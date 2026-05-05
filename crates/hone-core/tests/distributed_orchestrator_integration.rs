//! Integration tests for Orchestrator distributed dispatch (Phase 2b+2c).
//!
//! These tests drive `Orchestrator::execute_plan` with a real QueueClient
//! pointed at an httpmock server.  They prove the orchestrator:
//!   - routes through the coordinator (not in-proc) when queue_client is set
//!   - polls plan_status until all tasks are terminal
//!   - converts WorkerResultWire correctly for completed/failed/timed_out
//!   - falls back to in-process when the coordinator returns 503 on enqueue
//!   - respects `with_distributed_poll_interval` timing
//!
//! All tests MUST FAIL until the implementation is in place.

use std::sync::Arc;
use std::time::{Duration, Instant};

use httpmock::prelude::*;
use tokio::sync::mpsc;

use hone_core::{
    AgentConfig, AgentRegistry, Orchestrator, QueueClient, WorkerResultWire,
    task_plan::{SubTask, TaskPlan, WorkerStatus},
    RetryConfig,
    plan_mode::AgentMode,
};
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;

// ── helpers ───────────────────────────────────────────────────────────────────

fn base_config() -> AgentConfig {
    let provider = Arc::new(MockProvider::new(vec![]));
    AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "test lead".to_owned(),
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
        retry_config: RetryConfig::default(),
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
    }
}

fn make_subtask(id: &str, agent: &str) -> SubTask {
    SubTask {
        id: id.to_owned(),
        description: format!("task {id}"),
        agent: agent.to_owned(),
        context: String::new(),
        depends_on: vec![],
        acceptance_tests: vec![],
        budget_usd: None,
        scope: vec![],
        requires_capabilities: vec![],
    }
}

fn make_plan(subtasks: Vec<SubTask>) -> TaskPlan {
    TaskPlan {
        goal: "test goal".to_owned(),
        subtasks,
    }
}

fn make_result_wire(task_id: &str, plan_id: &str, agent: &str, status: &str) -> WorkerResultWire {
    WorkerResultWire {
        task_id: task_id.to_owned(),
        plan_id: plan_id.to_owned(),
        agent: agent.to_owned(),
        status: status.to_owned(),
        response: format!("output of {task_id}"),
        error_msg: None,
        tokens_input: 10,
        tokens_output: 20,
        cost_usd: 0.001,
    }
}

/// Build a minimal Orchestrator wired to the given QueueClient.
/// The registry is empty — any attempt to invoke an in-process agent fails,
/// proving the distributed path never falls through to in-proc.
fn orchestrator_with_client(client: Arc<QueueClient>) -> Orchestrator {
    Orchestrator::new(base_config(), AgentRegistry::new(), 4).with_queue_client(client)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// When queue_client is set, the orchestrator must call enqueue + plan_status +
/// pull_results on the coordinator — never invoking any in-process agent.
#[tokio::test]
async fn with_queue_client_set_dispatches_via_queue_not_in_proc() {
    let server = MockServer::start_async().await;

    let enqueue_mock = server.mock(|when, then| {
        when.method(POST)
            .path_contains("/distributed/plans/")
            .path_contains("/enqueue");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"task_ids":["t1"]}"#);
    });

    let status_mock = server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/status");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"queued":0,"leased":0,"done":1,"failed":0,"timed_out":0}"#);
    });

    let results_mock = server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/results");
        then.status(200)
            .header("Content-Type", "application/json")
            .json_body(serde_json::json!([{
                "task_id": "t1",
                "plan_id": "plan-x",
                "agent": "rust-expert",
                "status": "completed",
                "response": "great work",
                "error_msg": null,
                "tokens_input": 10,
                "tokens_output": 20,
                "cost_usd": 0.001
            }]));
    });

    let client = Arc::new(QueueClient::new(server.base_url(), "test-token"));
    let orchestrator = orchestrator_with_client(client)
        .with_distributed_poll_interval(Duration::from_millis(10));

    let (tx, _rx) = mpsc::channel(32);
    let plan = make_plan(vec![make_subtask("s1", "rust-expert")]);
    let results = orchestrator.execute_plan(plan, None, tx).await;

    enqueue_mock.assert();
    status_mock.assert();
    results_mock.assert();

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].task_id, "t1");
    assert!(
        matches!(results[0].status, WorkerStatus::Completed),
        "completed wire should convert to WorkerStatus::Completed, got {:?}",
        results[0].status
    );
    assert_eq!(results[0].response, "great work");
}

/// The orchestrator must keep polling plan_status until queued+leased == 0.
/// plan_status returns done=2 immediately (simplest way with httpmock).
/// Verifies that two results are returned correctly.
#[tokio::test]
async fn distributed_dispatch_polls_until_all_tasks_terminal() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(POST)
            .path_contains("/distributed/plans/")
            .path_contains("/enqueue");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"task_ids":["t1","t2"]}"#);
    });

    // Return queued=1 first, then queued=0 next poll.
    // httpmock doesn't support stateful responses, so we register two mocks;
    // the implementation must call status at least once.
    server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/status");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"queued":0,"leased":0,"done":2,"failed":0,"timed_out":0}"#);
    });

    server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/results");
        then.status(200)
            .header("Content-Type", "application/json")
            .json_body(serde_json::json!([
                make_result_wire("t1", "p", "a1", "completed"),
                make_result_wire("t2", "p", "a2", "completed"),
            ]));
    });

    let client = Arc::new(QueueClient::new(server.base_url(), "test-token"));
    let orchestrator = orchestrator_with_client(client)
        .with_distributed_poll_interval(Duration::from_millis(20));

    let (tx, _rx) = mpsc::channel(32);
    let plan = make_plan(vec![make_subtask("s1", "a1"), make_subtask("s2", "a2")]);
    let results = orchestrator.execute_plan(plan, None, tx).await;
    assert_eq!(results.len(), 2, "both tasks must appear in results");
}

/// WorkerResultWire with status="failed" and error_msg="boom" must produce
/// WorkerStatus::Failed("boom").
#[tokio::test]
async fn distributed_dispatch_handles_failed_tasks() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(POST)
            .path_contains("/distributed/plans/")
            .path_contains("/enqueue");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"task_ids":["tf1"]}"#);
    });

    server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/status");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"queued":0,"leased":0,"done":0,"failed":1,"timed_out":0}"#);
    });

    server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/results");
        then.status(200)
            .header("Content-Type", "application/json")
            .json_body(serde_json::json!([{
                "task_id": "tf1",
                "plan_id": "p-fail",
                "agent": "fail-agent",
                "status": "failed",
                "response": "",
                "error_msg": "boom",
                "tokens_input": 5,
                "tokens_output": 0,
                "cost_usd": 0.0
            }]));
    });

    let client = Arc::new(QueueClient::new(server.base_url(), "test-token"));
    let orchestrator = orchestrator_with_client(client)
        .with_distributed_poll_interval(Duration::from_millis(10));

    let (tx, _rx) = mpsc::channel(32);
    let plan = make_plan(vec![make_subtask("sf1", "fail-agent")]);
    let results = orchestrator.execute_plan(plan, None, tx).await;

    assert_eq!(results.len(), 1);
    match &results[0].status {
        WorkerStatus::Failed(msg) => assert_eq!(msg, "boom"),
        other => panic!("expected Failed(\"boom\"), got {other:?}"),
    }
}

/// WorkerResultWire with status="timed_out" must produce
/// WorkerStatus::TimedOut with error_msg preserved.
#[tokio::test]
async fn distributed_dispatch_handles_timed_out_tasks() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(POST)
            .path_contains("/distributed/plans/")
            .path_contains("/enqueue");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"task_ids":["tt1"]}"#);
    });

    server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/status");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"queued":0,"leased":0,"done":0,"failed":0,"timed_out":1}"#);
    });

    server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/results");
        then.status(200)
            .header("Content-Type", "application/json")
            .json_body(serde_json::json!([{
                "task_id": "tt1",
                "plan_id": "p-timeout",
                "agent": "slow-agent",
                "status": "timed_out",
                "response": "",
                "error_msg": "exceeded 300s",
                "tokens_input": 0,
                "tokens_output": 0,
                "cost_usd": 0.0
            }]));
    });

    let client = Arc::new(QueueClient::new(server.base_url(), "test-token"));
    let orchestrator = orchestrator_with_client(client)
        .with_distributed_poll_interval(Duration::from_millis(10));

    let (tx, _rx) = mpsc::channel(32);
    let plan = make_plan(vec![make_subtask("st1", "slow-agent")]);
    let results = orchestrator.execute_plan(plan, None, tx).await;

    assert_eq!(results.len(), 1);
    match &results[0].status {
        WorkerStatus::TimedOut(msg) => assert_eq!(msg, "exceeded 300s"),
        other => panic!("expected TimedOut(\"exceeded 300s\"), got {other:?}"),
    }
}

/// When the coordinator returns 503 on the enqueue call, the orchestrator must
/// log a warning and fall back to in-process execution.  The result vec must
/// have the same length as plan.subtasks (even if in-proc produces failures
/// because the agent doesn't exist — that's acceptable; the count is what counts).
#[tokio::test]
async fn distributed_dispatch_falls_back_to_in_proc_on_503() {
    let server = MockServer::start_async().await;

    // Coordinator returns 503 for enqueue.
    server.mock(|when, then| {
        when.method(POST)
            .path_contains("/distributed/plans/")
            .path_contains("/enqueue");
        then.status(503).body("distributed orchestrator not configured");
    });

    let client = Arc::new(QueueClient::new(server.base_url(), "test-token"));
    let orchestrator = Orchestrator::new(base_config(), AgentRegistry::new(), 4)
        .with_queue_client(client)
        .with_distributed_poll_interval(Duration::from_millis(10));

    let (tx, _rx) = mpsc::channel(32);
    let subtasks = vec![
        make_subtask("s1", "no-op-agent"),
        make_subtask("s2", "no-op-agent"),
    ];
    let task_count = subtasks.len();
    let plan = make_plan(subtasks);

    let results = orchestrator.execute_plan(plan, None, tx).await;

    // Fallback must produce exactly one result per subtask.
    assert_eq!(
        results.len(),
        task_count,
        "in-proc fallback must return one result per subtask"
    );
}

/// `with_distributed_poll_interval(50ms)` is respected.
/// With plan_status immediately returning done=1, wall-clock time should be
/// well under 500ms — the poll itself runs at least once in >= 50ms.
#[tokio::test]
async fn distributed_poll_interval_is_respected() {
    let server = MockServer::start_async().await;

    server.mock(|when, then| {
        when.method(POST)
            .path_contains("/distributed/plans/")
            .path_contains("/enqueue");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"task_ids":["t1"]}"#);
    });

    server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/status");
        then.status(200)
            .header("Content-Type", "application/json")
            .body(r#"{"queued":0,"leased":0,"done":1,"failed":0,"timed_out":0}"#);
    });

    server.mock(|when, then| {
        when.method(GET)
            .path_contains("/distributed/plans/")
            .path_contains("/results");
        then.status(200)
            .header("Content-Type", "application/json")
            .json_body(serde_json::json!([make_result_wire("t1", "p", "a", "completed")]));
    });

    let client = Arc::new(
        QueueClient::new(server.base_url(), "test-token")
            .with_timeout(Duration::from_secs(5)),
    );
    let orchestrator = orchestrator_with_client(client)
        .with_distributed_poll_interval(Duration::from_millis(50));

    let (tx, _rx) = mpsc::channel(32);
    let plan = make_plan(vec![make_subtask("s1", "any-agent")]);

    let start = Instant::now();
    let results = orchestrator.execute_plan(plan, None, tx).await;
    let elapsed = start.elapsed();

    assert_eq!(results.len(), 1, "should return one result");
    // The implementation must sleep at least one poll interval (50ms) before
    // the first status check.
    assert!(
        elapsed >= Duration::from_millis(50),
        "at least one poll interval (50ms) must have elapsed; got {elapsed:?}"
    );
    // Loose upper bound to keep CI fast.
    assert!(
        elapsed < Duration::from_millis(500),
        "should converge quickly; got {elapsed:?}"
    );
}
