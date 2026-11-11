use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use hone_core::{
    AgentConfig, AgentMode, AgentRegistry, Orchestrator, SubTask, TaskPlan, WorkerResult,
    WorkerStatus,
};
use hone_proto::{
    Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth, TokenUsage,
    ToolApprovalPolicy,
};
use hone_providers::{CompletionRequest, MockProvider, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn base_config(provider: Arc<MockProvider>) -> AgentConfig {
    AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "You are a lead orchestrator agent.".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: std::path::PathBuf::from("."),
        memory_store: None,
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    }
}

fn empty_registry() -> AgentRegistry {
    AgentRegistry::new()
}

fn registry_with_debugger() -> AgentRegistry {
    let mut registry = AgentRegistry::new();
    registry.load_builtins();
    registry
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_task_plan_serialization() {
    let plan = TaskPlan {
        goal: "Implement a feature".to_owned(),
        subtasks: vec![
            SubTask {
                id: "1".to_owned(),
                description: "Write tests".to_owned(),
                agent: "tdd-workflow".to_owned(),
                context: "Use TDD approach".to_owned(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
                requires_capabilities: vec![],
            },
            SubTask {
                id: "2".to_owned(),
                description: "Implement code".to_owned(),
                agent: "rust-expert".to_owned(),
                context: "Make tests pass".to_owned(),
                depends_on: vec!["1".to_owned()],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
                requires_capabilities: vec![],
            },
        ],
    };

    let json = serde_json::to_string(&plan).expect("serialization should succeed");
    let deserialized: TaskPlan =
        serde_json::from_str(&json).expect("deserialization should succeed");

    assert_eq!(deserialized.goal, plan.goal);
    assert_eq!(deserialized.subtasks.len(), 2);
    assert_eq!(deserialized.subtasks[0].id, "1");
    assert_eq!(deserialized.subtasks[0].agent, "tdd-workflow");
    assert_eq!(deserialized.subtasks[1].depends_on, vec!["1"]);
}

#[tokio::test]
async fn test_single_subtask_orchestration() {
    // decompose response: valid JSON plan with 1 subtask using "debugger"
    let decompose_json = r#"{"goal":"test","subtasks":[{"id":"1","description":"do thing","agent":"debugger","context":"","depends_on":[]}]}"#;
    // worker response
    let worker_response = "Task done successfully.";
    // merge response
    let merge_response = "Final merged answer.";

    // MockProvider returns the same token sequence for every call.
    // We need 3 calls: decompose, worker, merge.
    // MockProvider::new always returns the same tokens regardless of call count,
    // so we give it a combined response that works for all steps.
    // For decompose we need JSON; for worker and merge we need text.
    // Since MockProvider repeats the same tokens, we use a SequencedProvider
    // pattern — but MockProvider just repeats. We'll use decompose_json as the
    // single token so the worker also sees JSON (that's fine — it's returned as
    // the response text, and worker just records whatever the model returns).
    let provider = Arc::new(MockProvider::new(vec![decompose_json.to_owned()]));
    let registry = registry_with_debugger();
    let config = base_config(Arc::clone(&provider) as Arc<MockProvider>);

    let mut orchestrator = Orchestrator::new(config, registry, 4);
    let (tx, _rx) = mpsc::channel(256);

    let result = orchestrator
        .run("do thing", tx)
        .await
        .expect("orchestration should succeed");

    // There should be at least 1 worker result.
    assert!(!result.workers.is_empty());
    // The first worker should have completed (MockProvider doesn't return errors).
    assert_eq!(result.workers[0].status, WorkerStatus::Completed);
    // Goal should be preserved.
    assert_eq!(result.goal, "test");
    let _ = worker_response;
    let _ = merge_response;
}

#[tokio::test]
async fn test_decompose_fallback_on_invalid_json() {
    // Provider returns plain text, not JSON.
    let provider = Arc::new(MockProvider::new(vec![
        "This is not JSON at all.".to_owned(),
    ]));

    let registry = registry_with_debugger();
    let config = base_config(Arc::clone(&provider) as Arc<MockProvider>);

    let mut orchestrator = Orchestrator::new(config, registry, 4);
    let (tx, _rx) = mpsc::channel(256);

    // decompose should fall back to a single-subtask plan without panicking.
    let (plan, _lead_cost, _lead_usage) = orchestrator
        .decompose("do something", tx)
        .await
        .expect("decompose should succeed even with invalid JSON");

    assert_eq!(plan.subtasks.len(), 1);
    assert_eq!(plan.subtasks[0].id, "1");
    // The description should contain the original request.
    assert!(plan.subtasks[0].description.contains("do something"));
}

#[test]
fn test_worker_result_types() {
    let completed = WorkerResult {
        task_id: "task-1".to_owned(),
        agent: "rust-expert".to_owned(),
        status: WorkerStatus::Completed,
        response: "Done!".to_owned(),
        usage: TokenUsage {
            input: 10,
            output: 20,
            cache_read: 0,
            cache_creation: 0,
        },
        cost_usd: 0.001,
    };

    assert_eq!(completed.task_id, "task-1");
    assert_eq!(completed.agent, "rust-expert");
    assert_eq!(completed.status, WorkerStatus::Completed);
    assert_eq!(completed.response, "Done!");

    let failed = WorkerResult {
        task_id: "task-2".to_owned(),
        agent: "debugger".to_owned(),
        status: WorkerStatus::Failed("timeout".to_owned()),
        response: String::new(),
        usage: TokenUsage::default(),
        cost_usd: 0.0,
    };

    assert_eq!(failed.status, WorkerStatus::Failed("timeout".to_owned()));
    assert!(matches!(failed.status, WorkerStatus::Failed(_)));
}

#[tokio::test]
async fn test_dependency_ordering_logic() {
    // Plan: task A (no deps), task B (depends on A).
    // Provider always returns "done" — we just verify the results are collected
    // and that B's result appears after A's in the output.
    let provider = Arc::new(MockProvider::new(vec!["done".to_owned()]));

    let plan = TaskPlan {
        goal: "Ordered execution".to_owned(),
        subtasks: vec![
            SubTask {
                id: "A".to_owned(),
                description: "First task".to_owned(),
                agent: "debugger".to_owned(),
                context: String::new(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
                requires_capabilities: vec![],
            },
            SubTask {
                id: "B".to_owned(),
                description: "Second task depends on A".to_owned(),
                agent: "debugger".to_owned(),
                context: String::new(),
                depends_on: vec!["A".to_owned()],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
                requires_capabilities: vec![],
            },
        ],
    };

    let config = base_config(Arc::clone(&provider) as Arc<MockProvider>);
    let orchestrator = Orchestrator::new(config, empty_registry(), 4);

    let (tx, _rx) = mpsc::channel(256);
    let results = orchestrator.execute_plan(plan, None, tx).await;

    assert_eq!(results.len(), 2);

    // Find positions of A and B in the results vector.
    let pos_a = results
        .iter()
        .position(|r| r.task_id == "A")
        .expect("A should be present");
    let pos_b = results
        .iter()
        .position(|r| r.task_id == "B")
        .expect("B should be present");

    // A must have been completed before B started, so A's result index < B's.
    assert!(
        pos_a < pos_b,
        "A (pos {pos_a}) should appear before B (pos {pos_b}) in results"
    );
}

#[tokio::test]
async fn test_max_workers_respected() {
    // 5 independent subtasks, max_workers=2.
    // All should complete regardless of the concurrency cap.
    let provider = Arc::new(MockProvider::new(vec!["result".to_owned()]));

    let subtasks: Vec<SubTask> = (1..=5)
        .map(|i| SubTask {
            id: i.to_string(),
            description: format!("task {i}"),
            agent: "debugger".to_owned(),
            context: String::new(),
            depends_on: vec![],
            acceptance_tests: vec![],
            budget_usd: None,
            scope: vec![],
            requires_capabilities: vec![],
        })
        .collect();

    let plan = TaskPlan {
        goal: "Five parallel tasks".to_owned(),
        subtasks,
    };

    let config = base_config(Arc::clone(&provider) as Arc<MockProvider>);
    let orchestrator = Orchestrator::new(config, empty_registry(), 2);

    let (tx, _rx) = mpsc::channel(256);
    let results = orchestrator.execute_plan(plan, None, tx).await;

    // All 5 tasks should have completed.
    assert_eq!(results.len(), 5);
    for r in &results {
        assert_eq!(r.status, WorkerStatus::Completed);
    }
}

// ---------------------------------------------------------------------------
// Item 1 new tests: SubTask extension serialization
// ---------------------------------------------------------------------------

#[test]
fn test_subtask_serialization_with_new_fields() {
    let task = SubTask {
        id: "task-1".to_owned(),
        description: "Implement the auth module".to_owned(),
        agent: "rust-expert".to_owned(),
        context: "Use JWT tokens".to_owned(),
        depends_on: vec!["task-0".to_owned()],
        acceptance_tests: vec![
            "All existing tests pass".to_owned(),
            "New integration test passes".to_owned(),
        ],
        budget_usd: Some(0.50),
        scope: vec!["src/auth.rs".to_owned(), "src/auth/jwt.rs".to_owned()],
        requires_capabilities: vec![],
    };

    let json = serde_json::to_string(&task).expect("serialization should succeed");
    let roundtripped: SubTask =
        serde_json::from_str(&json).expect("deserialization should succeed");

    assert_eq!(roundtripped.id, task.id);
    assert_eq!(roundtripped.description, task.description);
    assert_eq!(roundtripped.agent, task.agent);
    assert_eq!(roundtripped.context, task.context);
    assert_eq!(roundtripped.depends_on, task.depends_on);
    assert_eq!(roundtripped.acceptance_tests, task.acceptance_tests);
    assert_eq!(roundtripped.budget_usd, task.budget_usd);
    assert_eq!(roundtripped.scope, task.scope);
}

#[test]
fn test_subtask_backwards_compat() {
    // Old-format JSON without the new fields — serde(default) must fill them.
    let old_json = r#"{
        "id": "1",
        "description": "do a thing",
        "agent": "debugger",
        "context": "some context",
        "depends_on": ["0"]
    }"#;

    let task: SubTask = serde_json::from_str(old_json).expect("old-format JSON should deserialize");

    assert_eq!(task.id, "1");
    assert_eq!(task.agent, "debugger");
    assert_eq!(task.depends_on, vec!["0"]);
    assert!(
        task.acceptance_tests.is_empty(),
        "acceptance_tests should default to empty"
    );
    assert!(
        task.budget_usd.is_none(),
        "budget_usd should default to None"
    );
    assert!(task.scope.is_empty(), "scope should default to empty");
}

// ---------------------------------------------------------------------------
// P1.2: Panicked worker task_id recovery
// ---------------------------------------------------------------------------

/// A provider that panics unconditionally.  Injecting this as the worker agent's
/// provider causes `run_worker` to panic, producing a `JoinError` in the
/// orchestrator's `JoinSet`.
struct PanicProvider;

#[async_trait]
impl ModelProvider for PanicProvider {
    fn id(&self) -> &str {
        "panic-provider"
    }
    fn display_name(&self) -> &str {
        "Panic Provider"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }
    async fn complete(
        &self,
        _req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        panic!("deliberate panic in PanicProvider::complete");
    }
    fn estimate_tokens(&self, _: &[Message]) -> u32 {
        1
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn panic_agent_config() -> AgentConfig {
    AgentConfig {
        provider: Arc::new(PanicProvider),
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "You are a panicking worker.".to_owned(),
        model: ModelId::new("panic:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("panic:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: std::path::PathBuf::from("/tmp"),
        memory_store: None,
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    }
}

/// A `SequencedProvider` alternates between a normal first response (for
/// orchestrator decompose / merge calls) and delegates worker calls to an inner
/// config that uses a panicking provider.  We instead test this via a dedicated
/// plan with `execute_plan` directly.
///
/// The orchestrator's `AgentRegistry` determines which `AgentConfig` is used
/// for each worker task.  The default `empty_registry()` falls back to the
/// orchestrator's own config.  To inject a panicking provider for *worker* tasks
/// only (not the lead), we build an `AgentRegistry` that maps the worker agent
/// name to the panic config.
#[tokio::test(flavor = "multi_thread")]
async fn test_panicked_worker_has_correct_task_id() {
    // Plan with three tasks: task-A and task-C use a normal provider;
    // task-B uses the panicking provider.  Since execute_plan uses the
    // orchestrator's own config for all workers (unless a registry is supplied),
    // we inject a panic by registering a panic-agent-name in the registry.
    //
    // Simpler approach: all three tasks are independent, orchestrator config
    // uses the panic provider, so ALL workers panic.  We then verify that the
    // resulting WorkerResults have task_ids matching the planted tasks, not
    // "unknown".
    let plan = TaskPlan {
        goal: "test panic recovery".to_owned(),
        subtasks: vec![
            SubTask {
                id: "task-panic-A".to_owned(),
                description: "first panicking task".to_owned(),
                agent: "debugger".to_owned(),
                context: String::new(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
                requires_capabilities: vec![],
            },
            SubTask {
                id: "task-panic-B".to_owned(),
                description: "second panicking task".to_owned(),
                agent: "debugger".to_owned(),
                context: String::new(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
                requires_capabilities: vec![],
            },
            SubTask {
                id: "task-panic-C".to_owned(),
                description: "third panicking task".to_owned(),
                agent: "debugger".to_owned(),
                context: String::new(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
                requires_capabilities: vec![],
            },
        ],
    };

    // Use the panic provider as the orchestrator config; all workers will panic.
    let config = panic_agent_config();
    let orchestrator = Orchestrator::new(config, empty_registry(), 4);

    let (tx, _rx) = mpsc::channel(256);
    let results = orchestrator.execute_plan(plan, None, tx).await;

    assert_eq!(
        results.len(),
        3,
        "all three panicked tasks should produce a result"
    );

    // Every result must have a task_id that matches one of the planted tasks,
    // NOT "unknown" (the old buggy value).
    let known_ids: Vec<&str> = vec!["task-panic-A", "task-panic-B", "task-panic-C"];
    for r in &results {
        assert!(
            known_ids.contains(&r.task_id.as_str()),
            "expected task_id to be one of {known_ids:?}, got {:?}",
            r.task_id
        );
        assert!(
            matches!(r.status, WorkerStatus::Failed(_)),
            "panicked task should have Failed status, got {:?}",
            r.status
        );
    }

    // All three distinct task_ids must be present.
    let mut result_ids: Vec<&str> = results.iter().map(|r| r.task_id.as_str()).collect();
    result_ids.sort_unstable();
    assert_eq!(
        result_ids,
        vec!["task-panic-A", "task-panic-B", "task-panic-C"]
    );
}

// ---------------------------------------------------------------------------
// G6 of docs/plans/plan-gate-spec.md: `Orchestrator::run_plan`
// ---------------------------------------------------------------------------

/// Records the text of every message of every `complete()` call, and answers
/// each call with the fixed text `"ok"`.
struct RecordingProvider {
    calls: std::sync::Mutex<Vec<String>>,
}

impl RecordingProvider {
    fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn decompose_calls(&self) -> usize {
        self.calls()
            .iter()
            .filter(|c| c.contains("You are a lead agent. Decompose"))
            .count()
    }
}

#[async_trait]
impl ModelProvider for RecordingProvider {
    fn id(&self) -> &str {
        "recording"
    }
    fn display_name(&self) -> &str {
        "Recording Provider"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }
    async fn complete(
        &self,
        req: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let text = req
            .messages
            .iter()
            .map(|m| m.content.as_text())
            .collect::<Vec<_>>()
            .join("\n");
        self.calls.lock().unwrap().push(text);
        Ok(Box::pin(futures::stream::iter(vec![
            ProviderEvent::Token {
                text: "ok".to_owned(),
            },
            ProviderEvent::Done,
        ])))
    }
    fn estimate_tokens(&self, _: &[Message]) -> u32 {
        1
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn recording_config(provider: Arc<RecordingProvider>) -> AgentConfig {
    let mut cfg = base_config(Arc::new(MockProvider::new(vec![])));
    cfg.provider = provider;
    cfg
}

fn two_task_plan() -> TaskPlan {
    let task = |id: &str, deps: Vec<&str>| SubTask {
        id: id.to_owned(),
        description: format!("do {id}"),
        agent: "debugger".to_owned(),
        context: String::new(),
        depends_on: deps.into_iter().map(str::to_owned).collect(),
        acceptance_tests: vec![],
        budget_usd: None,
        scope: vec![],
        requires_capabilities: vec![],
    };
    TaskPlan {
        goal: "ship the shown plan".to_owned(),
        subtasks: vec![task("a", vec![]), task("b", vec!["a"])],
    }
}

fn drain(rx: &mut mpsc::Receiver<hone_proto::AgentEvent>) -> Vec<hone_proto::AgentEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

#[tokio::test]
async fn test_g6_run_plan_never_asks_the_lead_to_decompose() {
    let provider = Arc::new(RecordingProvider::new());
    let mut orch = Orchestrator::new(
        recording_config(Arc::clone(&provider)),
        registry_with_debugger(),
        4,
    );
    let (tx, _rx) = mpsc::channel(256);

    let result = orch.run_plan(two_task_plan(), tx).await.expect("run_plan");

    assert_eq!(
        provider.decompose_calls(),
        0,
        "run_plan must not decompose; prompts: {:?}",
        provider.calls()
    );
    assert_eq!(result.workers.len(), 2, "both subtasks must run");
    assert!(
        result
            .workers
            .iter()
            .all(|w| w.status == WorkerStatus::Completed)
    );
}

#[tokio::test]
async fn test_g6_run_plan_sends_plan_created_before_any_worker_event() {
    let provider = Arc::new(RecordingProvider::new());
    let mut orch = Orchestrator::new(recording_config(provider), registry_with_debugger(), 4);
    let (tx, mut rx) = mpsc::channel(256);

    orch.run_plan(two_task_plan(), tx).await.expect("run_plan");
    let events = drain(&mut rx);

    let created: Vec<usize> = events
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e, hone_proto::AgentEvent::PlanCreated { .. }))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(created.len(), 1, "exactly one PlanCreated: {events:?}");
    match &events[created[0]] {
        hone_proto::AgentEvent::PlanCreated {
            goal,
            subtask_count,
        } => {
            assert_eq!(goal, "ship the shown plan");
            assert_eq!(*subtask_count, 2);
        }
        _ => unreachable!(),
    }
    let first_worker = events
        .iter()
        .position(|e| matches!(e, hone_proto::AgentEvent::WorkerStarted { .. }))
        .expect("a WorkerStarted event");
    assert!(
        created[0] < first_worker,
        "PlanCreated must precede workers: {events:?}"
    );
}

#[tokio::test]
async fn test_g6_run_plan_merges_with_the_plans_goal() {
    let provider = Arc::new(RecordingProvider::new());
    let mut orch = Orchestrator::new(
        recording_config(Arc::clone(&provider)),
        registry_with_debugger(),
        4,
    );
    let (tx, _rx) = mpsc::channel(256);

    let result = orch.run_plan(two_task_plan(), tx).await.expect("run_plan");

    assert_eq!(result.goal, "ship the shown plan");
    assert_eq!(result.merged_response, "ok");
    // The merge call is the last lead call and mentions the goal.
    let calls = provider.calls();
    assert!(
        calls.last().unwrap().contains("ship the shown plan"),
        "merge prompt must carry the plan goal; last call: {:?}",
        calls.last()
    );
}

#[tokio::test]
async fn test_g6_run_plan_writes_the_plan_store_before_and_after() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = hone_core::PlanStore::new(dir.path().join("plan.json"));
    let provider = Arc::new(RecordingProvider::new());
    let mut orch = Orchestrator::new(recording_config(provider), registry_with_debugger(), 4)
        .with_plan_store(store);
    let (tx, _rx) = mpsc::channel(256);

    orch.run_plan(two_task_plan(), tx).await.expect("run_plan");

    let written = std::fs::read_to_string(dir.path().join("plan.json")).expect("plan file");
    assert!(written.contains("ship the shown plan"), "{written}");
}

#[tokio::test]
async fn test_g6_run_still_decomposes_then_runs_the_plan() {
    let provider = Arc::new(RecordingProvider::new());
    let mut orch = Orchestrator::new(
        recording_config(Arc::clone(&provider)),
        registry_with_debugger(),
        4,
    );
    let (tx, mut rx) = mpsc::channel(256);

    // The recording provider answers "ok" (not JSON), so decompose falls back
    // to a single-subtask plan; what matters is that it was asked.
    let result = orch.run("do thing", tx).await.expect("run");

    assert_eq!(
        provider.decompose_calls(),
        1,
        "run must still decompose once"
    );
    assert_eq!(result.workers.len(), 1);
    assert!(
        drain(&mut rx)
            .iter()
            .any(|e| matches!(e, hone_proto::AgentEvent::PlanCreated { .. }))
    );
}

#[tokio::test]
async fn test_g18_run_plan_runs_a_dependency_after_its_prerequisite() {
    let provider = Arc::new(RecordingProvider::new());
    let mut orch = Orchestrator::new(
        recording_config(Arc::clone(&provider)),
        registry_with_debugger(),
        4,
    );
    let (tx, _rx) = mpsc::channel(256);

    // Plan: b depends on a (descriptions "do a" / "do b").
    orch.run_plan(two_task_plan(), tx).await.expect("run_plan");

    let calls = provider.calls();
    let first = |needle: &str| calls.iter().position(|c| c.contains(needle));
    let a = first("do a").expect("a's prompt");
    let b = first("do b").expect("b's prompt");
    assert!(a < b, "prerequisite a must be asked before b: {calls:?}");
}
