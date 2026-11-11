#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! W48 of `docs/plans/worker-tools-spec.md`: Esc kills the main agent's running
//! command. When the caller of a turn goes away (the TUI aborts the task
//! awaiting `run_turn` and drops its event receiver), the agent loop drops the
//! tool dispatch future, so the command's process group is killed (W23) and the
//! actor is free again instead of waiting for the command.
//!
//! Real time and a real `shell` tool are unavoidable here.

mod worker_tools_support;

use std::path::Path;
use std::time::Duration;

use hone_core::{AgentHandle, Orchestrator};
use hone_proto::{AgentEvent, ToolApprovalPolicy};
use serde_json::json;
use tokio::sync::mpsc;

use worker_tools_support::*;

fn shell_registry() -> hone_tools::ToolRegistry {
    hone_tools::ToolRegistry::builder()
        .register(Box::new(hone_tools::ShellTool))
        .build()
}

/// Script: backgrounds a grandchild that writes `marker` after 1 s, then the
/// foreground sleeps 4 s. Returns the shell command that runs it.
fn slow_command(dir: &Path, marker: &Path) -> String {
    let script = dir.join("s.sh");
    std::fs::write(
        &script,
        format!("(sleep 1; touch '{}') &\nsleep 4\n", marker.display()),
    )
    .unwrap();
    format!("sh {}", script.display())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_w48_esc_during_a_main_agent_shell_call_should_free_the_actor_and_kill_the_command() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let mut config = lead_config(ScriptedProvider::new(&[]), shell_registry(), dir.path());
    config.provider = CallsOnce::new(vec![tool_call(
        "s1",
        "shell",
        json!({ "command": slow_command(dir.path(), &marker), "timeout_ms": 20000 }),
    )]);
    config.approval_policy = ToolApprovalPolicy::Auto;
    let handle = AgentHandle::spawn(config);

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let h = handle.clone();
    let task = tokio::spawn(async move { h.run_turn("go", tx).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(e) = rx.recv().await {
            if matches!(e, AgentEvent::ToolCallStart { .. }) {
                return;
            }
        }
        panic!("events closed before the shell call started");
    })
    .await
    .expect("the shell call should start");
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Esc: abort the waiting task and drop the receiver.
    task.abort();
    drop(rx);

    // The command runs 4 s; the actor must answer far sooner.
    let freed = tokio::time::timeout(Duration::from_secs(1), handle.get_messages()).await;
    assert!(
        freed.is_ok(),
        "the actor must not wait for the command after Esc"
    );
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(
        !marker.exists(),
        "the command's grandchild must have been killed"
    );
}

/// Guard: dropping an orchestrated run mid-tool already kills a worker's
/// command (workers are dropped, not actor-driven).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_w48_dropping_an_orchestrated_run_mid_tool_should_kill_the_workers_command() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let provider = ScriptedProvider::new(&["W48-WORKER"]);
    provider.script(
        "W48-WORKER",
        vec![tool_call(
            "w1",
            "shell",
            json!({ "command": slow_command(dir.path(), &marker), "timeout_ms": 20000 }),
        )],
    );
    let config = lead_config(provider, shell_registry(), dir.path());
    let mut orch = Orchestrator::new(config, builtin_registry(), 4);
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let plan = plan(vec![subtask("1", "W48-WORKER", "rust-expert")]);
    let run = tokio::spawn(async move { orch.run_plan(plan, tx).await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(e) = rx.recv().await {
            if matches!(e, AgentEvent::ToolCallStart { .. }) {
                return;
            }
        }
        panic!("events closed before the worker's shell call started");
    })
    .await
    .expect("the worker's shell call should start");
    tokio::time::sleep(Duration::from_millis(200)).await;

    run.abort();
    drop(rx);

    tokio::time::sleep(Duration::from_millis(1800)).await;
    assert!(
        !marker.exists(),
        "the worker's command must have been killed"
    );
}
