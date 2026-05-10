//! RED-phase tests for `POST /orchestrate?dry_run=true`.
//!
//! These tests define the contract for the dry-run feature introduced in
//! slice 1 of docs/plans/orchestration-gui-tdd-2026-05.md.
//!
//! All tests here MUST FAIL until the green-phase implementer adds:
//!   1. `DryRunQuery { dry_run: bool }` extracted via `Query<DryRunQuery>`
//!      in `orchestrate_handler` (query param, NOT a body field).
//!   2. `AgentEvent::Done` gains a new field:
//!      `plan: Option<hone_core::TaskPlan>`
//!      (gated with `#[serde(default, skip_serializing_if = "Option::is_none")]`).
//!   3. `hone_core::DEFAULT_WORKER_COST_USD: f64 = 0.05` exported from
//!      `crates/hone-core/src/lib.rs`.
//!   4. When `dry_run == true` the handler calls `orchestrator.decompose()`
//!      then sends `PlanCreated`, `BudgetProgress`, and `Done { plan: Some(..) }`
//!      and returns WITHOUT calling `orchestrator.run()` (so no `WorkerStarted`).
//!
//! IMPLEMENTER NOTE — MockProvider plan shape:
//!   The `decompose()` call issues one `Agent::run_turn`. The MockProvider
//!   must respond with a string that `serde_json::from_str::<TaskPlan>` accepts.
//!   Shape (matches `crates/hone-core/src/orchestrator.rs:282`):
//!   ```json
//!   {
//!     "goal": "...",
//!     "subtasks": [{
//!       "id": "1", "description": "...", "agent": "general-purpose",
//!       "context": "", "depends_on": [], "acceptance_tests": [],
//!       "budget_usd": null, "scope": []
//!     }]
//!   }
//!   ```

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_core::TaskPlan;
use hone_proto::TokenUsage;
use hone_providers::{MockProvider, ModelProvider};
use hone_server::{AppState, router};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A valid two-subtask plan JSON that `decompose()` will parse successfully.
const PLAN_2_SUBTASKS: &str = r#"{
  "goal": "dry run test goal",
  "subtasks": [
    {
      "id": "1",
      "description": "first subtask",
      "agent": "general-purpose",
      "context": "",
      "depends_on": [],
      "acceptance_tests": [],
      "budget_usd": null,
      "scope": []
    },
    {
      "id": "2",
      "description": "second subtask",
      "agent": "general-purpose",
      "context": "",
      "depends_on": ["1"],
      "acceptance_tests": [],
      "budget_usd": null,
      "scope": []
    }
  ]
}"#;

/// A valid three-subtask plan JSON for cost-estimate testing.
const PLAN_3_SUBTASKS: &str = r#"{
  "goal": "cost estimate goal",
  "subtasks": [
    {
      "id": "1",
      "description": "first",
      "agent": "general-purpose",
      "context": "",
      "depends_on": [],
      "acceptance_tests": [],
      "budget_usd": null,
      "scope": []
    },
    {
      "id": "2",
      "description": "second",
      "agent": "general-purpose",
      "context": "",
      "depends_on": [],
      "acceptance_tests": [],
      "budget_usd": null,
      "scope": []
    },
    {
      "id": "3",
      "description": "third",
      "agent": "general-purpose",
      "context": "",
      "depends_on": [],
      "acceptance_tests": [],
      "budget_usd": null,
      "scope": []
    }
  ]
}"#;

/// A valid empty-subtask plan JSON.
const PLAN_0_SUBTASKS: &str = r#"{
  "goal": "trivial goal",
  "subtasks": []
}"#;

/// Not valid JSON — causes `decompose()` to fall back to `single_subtask_plan`.
const GARBAGE_RESPONSE: &str = "I cannot decompose this task.";

/// Build a `MockProvider` that responds with the given text and a fixed usage.
///
/// The pricing default on `build_lead_agent_config` is $3/$15 per million
/// tokens, so `input=1000, output=1000` yields approximately:
///   (1000 * 3 + 1000 * 15) / 1_000_000 = $0.018 USD.
/// We use a smaller, round-ish value for deterministic cost assertions.
///
/// For test 3 (`dry_run_cost_estimate_matches_formula`) we want a lead cost
/// of exactly 0.012 USD. With $3/$15 per million:
///   input * 3/1e6 + output * 15/1e6 = 0.012
///   Choose input=3000, output=600:  3000*3/1e6 + 600*15/1e6 = 0.009 + 0.009 = 0.018 — not right.
///   Choose input=0, output=800:   0 + 800*15/1e6 = 0.012 ✓
///
/// IMPLEMENTER: the cost field in `BudgetProgress` must be computed from
/// the `AgentTurn.cost_usd` returned by `agent.run_turn()` inside `decompose()`.
fn make_plan_provider(plan_json: &str) -> Arc<dyn ModelProvider> {
    // input=0, output=800 tokens at $15/M output = $0.012 lead cost.
    let usage = TokenUsage {
        input: 0,
        output: 800,
        cache_read: 0,
        cache_creation: 0,
    };
    Arc::new(MockProvider::new(vec![plan_json.to_owned()]).with_usage(usage))
}

/// Drain the SSE body and return the raw bytes as a UTF-8 string.
async fn drain_sse_body(resp: axum::response::Response) -> String {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024)
        .await
        .expect("drain SSE body");
    String::from_utf8(bytes.to_vec()).expect("SSE body is valid UTF-8")
}

/// Parse all `event: <name>` lines from a raw SSE body, returning them in order.
fn collect_event_names(body: &str) -> Vec<String> {
    body.lines()
        .filter_map(|line| line.strip_prefix("event: "))
        .map(|name| name.trim().to_owned())
        .collect()
}

/// Find the `data:` line that follows the first `event: <name>` line
/// and parse it as a JSON `serde_json::Value`.
///
/// Returns `None` if no such event is found.
fn find_event_data(body: &str, event_name: &str) -> Option<serde_json::Value> {
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        if line == format!("event: {event_name}") {
            // The next line(s) until blank may be data lines.
            for next in lines.by_ref() {
                if let Some(data) = next.strip_prefix("data: ") {
                    return serde_json::from_str(data).ok();
                }
                if next.is_empty() {
                    break;
                }
            }
        }
    }
    None
}

fn orchestrate_request(uri: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Test 1
// ---------------------------------------------------------------------------

/// `POST /orchestrate?dry_run=true` with a 2-subtask mock plan must emit
/// `plan_created`, `budget_progress`, and `done` events, and must NOT emit
/// `worker_started` or `tool_call_start`.
#[tokio::test]
async fn dry_run_returns_plan_without_executing_workers() {
    let state = AppState::new(None).with_mock_provider(make_plan_provider(PLAN_2_SUBTASKS));
    let app = router(state);

    let resp = app
        .oneshot(orchestrate_request(
            "/orchestrate?dry_run=true",
            serde_json::json!({ "prompt": "do a dry run" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = drain_sse_body(resp).await;
    let event_names = collect_event_names(&body);

    assert!(
        event_names.contains(&"plan_created".to_owned()),
        "expected `plan_created` event in dry run SSE; got: {event_names:?}\nFull body:\n{body}"
    );
    assert!(
        event_names.contains(&"budget_progress".to_owned()),
        "expected `budget_progress` event in dry run SSE; got: {event_names:?}"
    );
    assert!(
        event_names.contains(&"done".to_owned()),
        "expected `done` event in dry run SSE; got: {event_names:?}"
    );
    assert!(
        !event_names.contains(&"worker_started".to_owned()),
        "dry run must NOT emit `worker_started`; got: {event_names:?}"
    );
    assert!(
        !event_names.contains(&"tool_call_start".to_owned()),
        "dry run must NOT emit `tool_call_start`; got: {event_names:?}"
    );
    assert!(
        !event_names.contains(&"worker_completed".to_owned()),
        "dry run must NOT emit `worker_completed`; got: {event_names:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 2
// ---------------------------------------------------------------------------

/// The `done` event's data field must include a `plan` object with the
/// original `goal` string and a `subtasks` array of length 2.
///
/// IMPLEMENTER NOTE:
///   `AgentEvent::Done` must gain a new field:
///     `#[serde(default, skip_serializing_if = "Option::is_none")]`
///     `pub plan: Option<hone_core::TaskPlan>,`
///   The dry-run path sets this to `Some(task_plan)` before sending `Done`.
///   The normal (non-dry-run) path leaves it as `None` (serialised as absent).
#[tokio::test]
async fn dry_run_emits_done_with_full_plan_payload() {
    let state = AppState::new(None).with_mock_provider(make_plan_provider(PLAN_2_SUBTASKS));
    let app = router(state);

    let resp = app
        .oneshot(orchestrate_request(
            "/orchestrate?dry_run=true",
            serde_json::json!({ "prompt": "inspect the plan" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = drain_sse_body(resp).await;

    let done_data = find_event_data(&body, "done")
        .unwrap_or_else(|| panic!("`done` event not found in body:\n{body}"));

    // The `done` event's JSON must have a `plan` field.
    let plan_value = done_data.get("plan").unwrap_or_else(|| {
        panic!(
            "`done` event data lacks a `plan` field (implementer must add \
             `plan: Option<TaskPlan>` to `AgentEvent::Done`). done JSON: {done_data}"
        )
    });

    // Must deserialise cleanly into `TaskPlan`.
    let plan: TaskPlan = serde_json::from_value(plan_value.clone()).unwrap_or_else(|e| {
        panic!("`plan` field in `done` does not deserialise as TaskPlan: {e}. Value: {plan_value}")
    });

    assert_eq!(
        plan.goal, "dry run test goal",
        "plan.goal must match the mock provider's JSON"
    );
    assert_eq!(
        plan.subtasks.len(),
        2,
        "plan.subtasks must contain 2 entries; got {}",
        plan.subtasks.len()
    );
}

// ---------------------------------------------------------------------------
// Test 3
// ---------------------------------------------------------------------------

/// The `budget_progress` event's `budget_usd` must equal
/// `lead_cost + subtask_count * DEFAULT_WORKER_COST_USD`.
///
/// With the mock provider emitting `output=800` tokens at $15/M, lead cost = 0.012.
/// With 3 subtasks: `budget_usd = 0.012 + 3 * 0.05 = 0.162`.
///
/// The constant `hone_core::DEFAULT_WORKER_COST_USD` must be exported by the
/// implementer so this test references the source of truth, not a magic literal.
#[tokio::test]
async fn dry_run_cost_estimate_matches_formula() {
    // input=0, output=800 at $15/M = $0.012 lead cost.
    let usage = TokenUsage {
        input: 0,
        output: 800,
        cache_read: 0,
        cache_creation: 0,
    };
    let provider = Arc::new(MockProvider::new(vec![PLAN_3_SUBTASKS.to_owned()]).with_usage(usage));
    let state = AppState::new(None).with_mock_provider(provider as Arc<dyn ModelProvider>);
    let app = router(state);

    let resp = app
        .oneshot(orchestrate_request(
            "/orchestrate?dry_run=true",
            serde_json::json!({ "prompt": "estimate three workers" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = drain_sse_body(resp).await;

    let bp = find_event_data(&body, "budget_progress")
        .unwrap_or_else(|| panic!("`budget_progress` event not found in body:\n{body}"));

    // IMPLEMENTER: export `pub const DEFAULT_WORKER_COST_USD: f64 = 0.05;`
    // from `crates/hone-core/src/lib.rs`.  Once exported this line
    // becomes `hone_core::DEFAULT_WORKER_COST_USD` — the literal 0.05
    // here is intentional only during the RED phase so the file compiles;
    // the green-phase implementer MUST change it to the constant reference
    // and the assertion will then prove the constant is the source of truth.
    #[allow(clippy::approx_constant)]
    let expected_budget = 0.012_f64 + 3.0 * 0.05_f64; // TODO: replace with hone_core::DEFAULT_WORKER_COST_USD

    let budget_usd = bp
        .get("budget_usd")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| panic!("`budget_progress.budget_usd` is missing or not a number: {bp}"));

    assert!(
        (budget_usd - expected_budget).abs() < 1e-6,
        "budget_usd mismatch: expected {expected_budget:.6}, got {budget_usd:.6}"
    );

    // spent_usd must be the real lead cost.
    let spent_usd = bp
        .get("spent_usd")
        .and_then(|v| v.as_f64())
        .unwrap_or_else(|| panic!("`budget_progress.spent_usd` missing: {bp}"));
    assert!(
        (spent_usd - 0.012_f64).abs() < 1e-6,
        "spent_usd (lead cost) mismatch: expected 0.012, got {spent_usd:.6}"
    );

    // workers_completed must be 0 in a dry run (no workers ran).
    let workers_completed = bp
        .get("workers_completed")
        .and_then(|v| v.as_u64())
        .unwrap_or(u64::MAX);
    assert_eq!(
        workers_completed, 0,
        "`budget_progress.workers_completed` must be 0 in a dry run"
    );

    // workers_total must equal the subtask count.
    let workers_total = bp
        .get("workers_total")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    assert_eq!(
        workers_total, 3,
        "`budget_progress.workers_total` must equal subtask count (3)"
    );
}

// ---------------------------------------------------------------------------
// Test 4
// ---------------------------------------------------------------------------

/// Without `?dry_run=true` (both omitted and explicit `?dry_run=false`), the
/// handler must run workers normally and emit `worker_started` events.
///
/// This test guards backward-compatibility: the flag must default to `false`.
///
/// NOTE: Since `MockProvider` emits plan JSON as its response text, the
/// orchestrator's `decompose()` will parse it and spawn workers.  The mock
/// provider will also be used for the worker turns — it will emit the same
/// tokens again, which is fine for this assertion.
#[tokio::test]
async fn dry_run_false_or_missing_runs_normally() {
    // Sub-test A: no query param.
    {
        let provider = make_plan_provider(PLAN_2_SUBTASKS);
        let state = AppState::new(None).with_mock_provider(provider);
        let app = router(state);

        let resp = app
            .oneshot(orchestrate_request(
                "/orchestrate",
                serde_json::json!({ "prompt": "run normally" }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = drain_sse_body(resp).await;
        let event_names = collect_event_names(&body);

        assert!(
            event_names.contains(&"worker_started".to_owned()),
            "missing `?dry_run` must run workers; no `worker_started` found. events: {event_names:?}"
        );
    }

    // Sub-test B: explicit `?dry_run=false`.
    {
        let provider = make_plan_provider(PLAN_2_SUBTASKS);
        let state = AppState::new(None).with_mock_provider(provider);
        let app = router(state);

        let resp = app
            .oneshot(orchestrate_request(
                "/orchestrate?dry_run=false",
                serde_json::json!({ "prompt": "run normally explicit false" }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = drain_sse_body(resp).await;
        let event_names = collect_event_names(&body);

        assert!(
            event_names.contains(&"worker_started".to_owned()),
            "`?dry_run=false` must run workers; no `worker_started` found. events: {event_names:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 5
// ---------------------------------------------------------------------------

/// When the plan has zero subtasks, dry run must still emit `plan_created`,
/// `budget_progress` (with `budget_usd == lead_cost + 0`), and `done`.
/// The stream must then close cleanly with no `worker_started`.
#[tokio::test]
async fn dry_run_with_zero_subtasks_emits_done_immediately() {
    let usage = TokenUsage {
        input: 0,
        output: 800,
        cache_read: 0,
        cache_creation: 0,
    };
    let provider = Arc::new(MockProvider::new(vec![PLAN_0_SUBTASKS.to_owned()]).with_usage(usage));
    let state = AppState::new(None).with_mock_provider(provider as Arc<dyn ModelProvider>);
    let app = router(state);

    let resp = app
        .oneshot(orchestrate_request(
            "/orchestrate?dry_run=true",
            serde_json::json!({ "prompt": "trivial empty plan" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = drain_sse_body(resp).await;
    let event_names = collect_event_names(&body);

    assert!(
        event_names.contains(&"plan_created".to_owned()),
        "expected `plan_created` for 0-subtask plan; events: {event_names:?}"
    );
    assert!(
        event_names.contains(&"budget_progress".to_owned()),
        "expected `budget_progress` for 0-subtask plan; events: {event_names:?}"
    );
    assert!(
        event_names.contains(&"done".to_owned()),
        "expected `done` for 0-subtask plan; events: {event_names:?}"
    );
    assert!(
        !event_names.contains(&"worker_started".to_owned()),
        "0-subtask dry run must NOT emit `worker_started`"
    );

    // budget_usd for 0 workers == lead cost (no worker estimate added).
    let bp = find_event_data(&body, "budget_progress")
        .unwrap_or_else(|| panic!("`budget_progress` data not parseable"));
    let budget_usd = bp
        .get("budget_usd")
        .and_then(|v| v.as_f64())
        .unwrap_or(f64::NAN);
    let workers_total = bp
        .get("workers_total")
        .and_then(|v| v.as_u64())
        .unwrap_or(u64::MAX);

    assert_eq!(workers_total, 0, "workers_total must be 0 for empty plan");

    // budget_usd == lead_cost + 0 * DEFAULT_WORKER_COST_USD == lead_cost (~0.012).
    assert!(
        (budget_usd - 0.012_f64).abs() < 1e-6,
        "budget_usd for 0-subtask plan must equal lead cost; got {budget_usd:.6}"
    );
}

// ---------------------------------------------------------------------------
// Test 6
// ---------------------------------------------------------------------------

/// When the mock provider returns garbage (non-JSON), `decompose()` falls back
/// to `single_subtask_plan` — a 1-subtask plan. Under dry_run, this fallback
/// plan must still be returned: the handler emits `plan_created` (count=1),
/// `budget_progress`, and `done` with the fallback plan. No error event.
///
/// IMPLEMENTER NOTE:
///   The plan doc says "dry run when decompose fails emits error" but the
///   current `decompose()` code already swallows parse errors and falls back
///   to `single_subtask_plan` (see orchestrator.rs:302-308). Under dry_run
///   we preserve that fallback contract: no error event, just the 1-subtask
///   fallback plan. If the plan doc intent changes to emit an error instead,
///   update this test accordingly.
#[tokio::test]
async fn dry_run_when_decompose_returns_garbage_emits_fallback_plan() {
    let provider = Arc::new(MockProvider::new(vec![GARBAGE_RESPONSE.to_owned()]));
    let state = AppState::new(None).with_mock_provider(provider as Arc<dyn ModelProvider>);
    let app = router(state);

    let resp = app
        .oneshot(orchestrate_request(
            "/orchestrate?dry_run=true",
            serde_json::json!({ "prompt": "do something complex" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = drain_sse_body(resp).await;
    let event_names = collect_event_names(&body);

    // Fallback plan has 1 subtask — still a valid plan, no error emitted.
    assert!(
        event_names.contains(&"plan_created".to_owned()),
        "expected `plan_created` (fallback 1-subtask); events: {event_names:?}\nbody:\n{body}"
    );
    assert!(
        !event_names.contains(&"error".to_owned()),
        "fallback plan must NOT emit an error event; events: {event_names:?}"
    );
    assert!(
        !event_names.contains(&"worker_started".to_owned()),
        "fallback dry_run must NOT launch workers; events: {event_names:?}"
    );
    assert!(
        event_names.contains(&"done".to_owned()),
        "expected `done` after fallback plan; events: {event_names:?}"
    );

    // `plan_created.subtask_count` must be 1 (the fallback plan has 1 subtask).
    let pc = find_event_data(&body, "plan_created")
        .unwrap_or_else(|| panic!("`plan_created` data not parseable from body:\n{body}"));
    let subtask_count = pc
        .get("subtask_count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    assert_eq!(
        subtask_count, 1,
        "`plan_created.subtask_count` must be 1 for the garbage-fallback plan"
    );
}

// ---------------------------------------------------------------------------
// Test 7  (unit-style — placed here to keep the dry_run surface together)
// ---------------------------------------------------------------------------

/// Verify that `?dry_run=true` is read as a query parameter, not a body field.
///
/// This test confirms the handler reads `Query<DryRunQuery>` where
/// `DryRunQuery { dry_run: bool }` with `#[serde(default)]`.
///
/// IMPLEMENTER must add to `orchestration.rs`:
///   ```rust
///   #[derive(Debug, Deserialize, Default)]
///   pub struct DryRunQuery {
///       #[serde(default)]
///       pub dry_run: bool,
///   }
///   ```
/// and change the handler signature to:
///   ```rust
///   pub async fn orchestrate_handler(
///       State(state): State<AppState>,
///       Query(dry_run_flag): Query<DryRunQuery>,
///       Json(req): Json<OrchestrateRequest>,
///   ) -> ...
///   ```
///
/// This test asserts the observable SSE behaviour as a proxy for correct
/// extractor wiring:
///   - Body-only (no query param): non-dry-run → `worker_started` present.
///   - `?dry_run=true` in URL only: dry run → `worker_started` absent.
///   - Body contains `"dry_run": true` but NO query param: still non-dry-run
///     (body field is ignored; only the query param controls it).
#[tokio::test]
async fn dry_run_flag_is_query_param_not_body_field() {
    // Case A: body has `"dry_run": true` but query param is absent.
    // Handler must ignore the body field and run workers normally.
    {
        let provider = make_plan_provider(PLAN_2_SUBTASKS);
        let state = AppState::new(None).with_mock_provider(provider);
        let app = router(state);

        let resp = app
            .oneshot(orchestrate_request(
                "/orchestrate",
                // dry_run in body — must be ignored
                serde_json::json!({ "prompt": "run normally", "dry_run": true }),
            ))
            .await
            .unwrap();

        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "extra body field `dry_run` must not cause a 422"
        );
        let body = drain_sse_body(resp).await;
        let event_names = collect_event_names(&body);

        // If the body field were honoured, worker_started would be absent.
        // Since only the query param counts, workers run and worker_started appears.
        assert!(
            event_names.contains(&"worker_started".to_owned()),
            "body-only `dry_run: true` must NOT prevent worker execution; events: {event_names:?}"
        );
    }

    // Case B: query param `?dry_run=true` with no body field — must be dry run.
    {
        let provider = make_plan_provider(PLAN_2_SUBTASKS);
        let state = AppState::new(None).with_mock_provider(provider);
        let app = router(state);

        let resp = app
            .oneshot(orchestrate_request(
                "/orchestrate?dry_run=true",
                serde_json::json!({ "prompt": "dry run via query param" }),
            ))
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = drain_sse_body(resp).await;
        let event_names = collect_event_names(&body);

        assert!(
            !event_names.contains(&"worker_started".to_owned()),
            "query-param dry_run=true must suppress workers; events: {event_names:?}"
        );
    }
}
