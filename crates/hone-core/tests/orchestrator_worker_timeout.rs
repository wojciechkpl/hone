#![allow(clippy::expect_used, clippy::unwrap_used)]
//! W8 of `docs/plans/worker-tools-spec.md`: `Orchestrator::without_worker_timeout()`.
//! Own file because the API does not exist yet (compile-level red); the guards
//! for the unchanged default live in `orchestrator_worker_tools.rs`.

mod worker_tools_support;

use std::sync::Arc;
use std::time::Duration;

use hone_core::{Orchestrator, WorkerStatus};

use worker_tools_support::*;

#[tokio::test(start_paused = true)]
async fn test_w8_without_worker_timeout_when_worker_takes_an_hour_should_complete() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W8-LONG"]);
    *provider.worker_delay.lock().unwrap() = Some(Duration::from_secs(3600));
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, builtin_registry(), 4).without_worker_timeout();
    let out = run_plan_with_ui_limit(
        orch,
        plan(vec![subtask("1", "W8-LONG", "rust-expert")]),
        None,
        proceed_all(),
        Duration::from_secs(86_400),
    )
    .await;

    assert!(
        matches!(out.workers[0].status, WorkerStatus::Completed),
        "{:?}",
        out.workers[0].status
    );
    assert_eq!(out.workers[0].response, "done-W8-LONG");
}

#[tokio::test(start_paused = true)]
async fn test_w8_without_worker_timeout_should_override_an_earlier_with_worker_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let log = new_log();
    let provider = ScriptedProvider::new(&["W8-OVERRIDE"]);
    *provider.worker_delay.lock().unwrap() = Some(Duration::from_secs(60));
    let config = lead_config(Arc::clone(&provider), session_tools(&log), dir.path());

    let orch = Orchestrator::new(config, builtin_registry(), 4)
        .with_worker_timeout(5)
        .without_worker_timeout();
    let out = run_plan_with_ui_limit(
        orch,
        plan(vec![subtask("1", "W8-OVERRIDE", "rust-expert")]),
        None,
        proceed_all(),
        Duration::from_secs(86_400),
    )
    .await;

    assert!(matches!(out.workers[0].status, WorkerStatus::Completed));
}
