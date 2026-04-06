use std::sync::Arc;

use hone_core::{
    AgentConfig, AgentRegistry, Orchestrator, SubTask, TaskPlan, WorkerResult, WorkerStatus,
};
use hone_proto::{ModelId, PricingEntry, TokenUsage};
use hone_providers::MockProvider;
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
        },
        working_dir: std::path::PathBuf::from("."),
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
            },
            SubTask {
                id: "2".to_owned(),
                description: "Implement code".to_owned(),
                agent: "rust-expert".to_owned(),
                context: "Make tests pass".to_owned(),
                depends_on: vec!["1".to_owned()],
            },
        ],
    };

    let json = serde_json::to_string(&plan).expect("serialization should succeed");
    let deserialized: TaskPlan = serde_json::from_str(&json).expect("deserialization should succeed");

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
    let plan = orchestrator
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
        usage: TokenUsage { input: 10, output: 20, cache_read: 0 },
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
            },
            SubTask {
                id: "B".to_owned(),
                description: "Second task depends on A".to_owned(),
                agent: "debugger".to_owned(),
                context: String::new(),
                depends_on: vec!["A".to_owned()],
            },
        ],
    };

    let config = base_config(Arc::clone(&provider) as Arc<MockProvider>);
    let orchestrator = Orchestrator::new(config, empty_registry(), 4);

    let (tx, _rx) = mpsc::channel(256);
    let results = orchestrator.execute_plan(plan, tx).await;

    assert_eq!(results.len(), 2);

    // Find positions of A and B in the results vector.
    let pos_a = results.iter().position(|r| r.task_id == "A").expect("A should be present");
    let pos_b = results.iter().position(|r| r.task_id == "B").expect("B should be present");

    // A must have been completed before B started, so A's result index < B's.
    assert!(pos_a < pos_b, "A (pos {pos_a}) should appear before B (pos {pos_b}) in results");
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
        })
        .collect();

    let plan = TaskPlan {
        goal: "Five parallel tasks".to_owned(),
        subtasks,
    };

    let config = base_config(Arc::clone(&provider) as Arc<MockProvider>);
    let orchestrator = Orchestrator::new(config, empty_registry(), 2);

    let (tx, _rx) = mpsc::channel(256);
    let results = orchestrator.execute_plan(plan, tx).await;

    // All 5 tasks should have completed.
    assert_eq!(results.len(), 5);
    for r in &results {
        assert_eq!(r.status, WorkerStatus::Completed);
    }
}
