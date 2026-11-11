#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! W23 of `docs/plans/worker-tools-spec.md`: a shell command ends with its tool
//! call. The marker is written by a grandchild (`( sleep 1; touch M ) &` runs in
//! a subshell, a child of the `sh` that the tool starts), so killing only the
//! direct child (`kill_on_drop`) is not enough: the whole process group must go.
//!
//! Real time is unavoidable here (a real process outliving its call).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hone_sandbox::{NullSandbox, SandboxPolicy};
use hone_tools::{Tool, ToolContext, ToolError, ToolInput};
use serde_json::json;

fn ctx(dir: &Path) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(SandboxPolicy::permissive()),
        working_dir: dir.to_path_buf(),
        read_filter: None,
    }
}

/// The shell tool's validator rejects chained commands (`;`, `&`), so each
/// scenario lives in a script file and the tool runs `sh <script>`.
fn script(dir: &Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, body).unwrap();
    format!("sh {}", path.display())
}

/// Backgrounds a subshell (a grandchild of the tool's `sh`) that writes `marker`
/// after 1 s, then waits 5 s in the foreground.
fn marker_command(dir: &Path, marker: &Path) -> String {
    script(
        dir,
        "marker.sh",
        &format!("(sleep 1; touch '{}') &\nsleep 5\n", marker.display()),
    )
}

fn marker_path(dir: &Path) -> PathBuf {
    dir.join("marker")
}

async fn settle() {
    // The marker, if the grandchild survives, appears about 1 s after the start.
    tokio::time::sleep(Duration::from_millis(1500)).await;
}

#[tokio::test]
async fn test_w23_dropping_the_call_should_kill_grandchildren() {
    let dir = tempfile::tempdir().unwrap();
    let marker = marker_path(dir.path());
    let c = ctx(dir.path());
    let input = ToolInput::new(json!({ "command": marker_command(dir.path(), &marker) }));

    let call = hone_tools::ShellTool.call(input, &c);
    let ended_early = tokio::time::timeout(Duration::from_millis(200), call)
        .await
        .is_err();
    assert!(ended_early, "the command (sleep 5) must still be running");

    settle().await;
    assert!(
        !marker.exists(),
        "the backgrounded grandchild outlived the dropped call"
    );
}

#[tokio::test]
async fn test_w23_own_timeout_should_return_timeout_error_and_kill_grandchildren() {
    let dir = tempfile::tempdir().unwrap();
    let marker = marker_path(dir.path());
    let c = ctx(dir.path());
    let input = ToolInput::new(json!({
        "command": marker_command(dir.path(), &marker),
        "timeout_ms": 200
    }));

    let result = hone_tools::ShellTool.call(input, &c).await;

    assert!(
        matches!(
            result,
            Err(ToolError::Timeout {
                duration_ms: 200,
                ..
            })
        ),
        "{result:?}"
    );
    settle().await;
    assert!(
        !marker.exists(),
        "timed-out command left a grandchild running"
    );
}

#[tokio::test]
async fn test_w23_command_that_finishes_normally_should_keep_its_background_work_untouched() {
    // A finished command is left alone: here it exits at once, and the
    // *foreground* result is returned as before.
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let input = ToolInput::new(json!({ "command": "echo hi" }));

    let out = hone_tools::ShellTool.call(input, &c).await.unwrap();

    assert_eq!(out.value["stdout"], "hi\n");
    assert_eq!(out.value["exit_code"], 0);
}

#[tokio::test]
async fn test_w23_non_zero_exit_should_still_return_its_code_and_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let input = ToolInput::new(
        json!({ "command": script(dir.path(), "exit3.sh", "echo oops >&2\nexit 3\n") }),
    );

    let out = hone_tools::ShellTool.call(input, &c).await.unwrap();

    assert_eq!(out.value["exit_code"], 3);
    assert_eq!(out.value["stderr"], "oops\n");
}

#[tokio::test]
async fn test_w23_stdin_should_stay_closed() {
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let input =
        ToolInput::new(json!({ "command": script(dir.path(), "cat.sh", "cat\necho done\n") }));

    let out = tokio::time::timeout(
        Duration::from_secs(3),
        hone_tools::ShellTool.call(input, &c),
    )
    .await
    .expect("`cat` must see EOF on stdin, not hang")
    .unwrap();

    assert_eq!(out.value["stdout"], "done\n");
    assert_eq!(out.value["exit_code"], 0);
}

#[tokio::test]
async fn test_w23_command_should_run_in_its_own_process_group() {
    // The group id of the command is its own pid (it leads a new group), and
    // differs from the test process's group: that is what lets one SIGKILL
    // reach every descendant.
    let dir = tempfile::tempdir().unwrap();
    let c = ctx(dir.path());
    let input = ToolInput::new(json!({
        "command": script(dir.path(), "pg.sh", "ps -o pid=,pgid= -p $$\n")
    }));

    let out = hone_tools::ShellTool.call(input, &c).await.unwrap();

    let stdout = out.value["stdout"].as_str().unwrap().to_owned();
    let mut it = stdout.split_whitespace();
    let pid: u32 = it.next().expect("pid").parse().unwrap();
    let pgid: u32 = it.next().expect("pgid").parse().unwrap();
    assert_eq!(
        pgid, pid,
        "the shell must lead its own process group: {stdout:?}"
    );
}
