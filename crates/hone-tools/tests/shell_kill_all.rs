#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! W25 of `docs/plans/worker-tools-spec.md`: a process-wide registry of running
//! command groups, and a public function that kills them all (called when hone
//! exits). New API, so this file fails to compile until it exists.
//!
//! Assumed name and shape: `hone_tools::kill_running_commands()`, a plain
//! synchronous function (`mod shell` is private, so the root re-export).
//!
//! The registry is process-wide and the tests of this binary run in parallel:
//! every test takes `SERIAL` so one test's kill-all cannot hit another's command.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hone_sandbox::{NullSandbox, SandboxPolicy};
use hone_tools::{Tool, ToolContext, ToolInput};
use serde_json::json;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(SandboxPolicy::permissive()),
        working_dir: dir.to_path_buf(),
        read_filter: None,
    }
}

fn marker_script(dir: &Path, name: &str, marker: &Path) -> String {
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!("(sleep 1; touch '{}') &\nsleep 5\n", marker.display()),
    )
    .unwrap();
    format!("sh {}", path.display())
}

/// The killed call ended: either an error, or a non-zero exit (killed by a
/// signal reports no code).
fn assert_killed(result: Result<hone_tools::ToolOutput, hone_tools::ToolError>) {
    if let Ok(out) = result {
        assert_ne!(
            out.value["exit_code"], 0,
            "a killed command did not succeed: {out:?}"
        );
    }
}

fn spawn_call(
    dir: &Path,
    command: String,
) -> tokio::task::JoinHandle<Result<hone_tools::ToolOutput, hone_tools::ToolError>> {
    let c = ctx(dir);
    tokio::spawn(async move {
        hone_tools::ShellTool
            .call(ToolInput::new(json!({ "command": command })), &c)
            .await
    })
}

#[tokio::test]
async fn test_w25_kill_running_commands_should_end_the_call_and_kill_grandchildren() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("marker");
    let call = spawn_call(dir.path(), marker_script(dir.path(), "m.sh", &marker));
    tokio::time::sleep(Duration::from_millis(200)).await;

    hone_tools::kill_running_commands();

    // The call ends promptly (killed, not waiting out `sleep 5`).
    let result = tokio::time::timeout(Duration::from_secs(2), call)
        .await
        .expect("the call must end after kill_running_commands")
        .expect("task joins");
    assert_killed(result);
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!marker.exists(), "the grandchild survived kill-all");
}

#[tokio::test]
async fn test_w25_kill_running_commands_should_kill_every_running_command() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let m1 = dir.path().join("m1");
    let m2 = dir.path().join("m2");
    let a = spawn_call(dir.path(), marker_script(dir.path(), "m1.sh", &m1));
    let b = spawn_call(dir.path(), marker_script(dir.path(), "m2.sh", &m2));
    tokio::time::sleep(Duration::from_millis(200)).await;

    hone_tools::kill_running_commands();

    for call in [a, b] {
        let result = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .expect("each call ends")
            .expect("task joins");
        assert_killed(result);
    }
    tokio::time::sleep(Duration::from_millis(1500)).await;
    assert!(!m1.exists() && !m2.exists());
}

#[tokio::test]
async fn test_w25_kill_running_commands_when_nothing_runs_should_be_a_no_op() {
    let _g = SERIAL.lock().await;
    hone_tools::kill_running_commands();

    // A later command is unaffected: finished commands are not tracked.
    let dir = tempfile::tempdir().unwrap();
    let out = spawn_call(dir.path(), "echo hi".to_owned())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out.value["stdout"], "hi\n");
    hone_tools::kill_running_commands();
}

#[tokio::test]
async fn test_w25_command_started_after_a_kill_all_should_run_normally() {
    let _g = SERIAL.lock().await;
    hone_tools::kill_running_commands();
    let dir = tempfile::tempdir().unwrap();

    let out = spawn_call(dir.path(), "sleep 0.2".to_owned())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(out.value["stdout"], "");
    assert_eq!(out.value["exit_code"], 0);
}
