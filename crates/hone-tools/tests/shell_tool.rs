use std::sync::Arc;

use hone_sandbox::NullSandbox;
use hone_tools::{ShellTool, Tool, ToolContext, ToolInput};
use serde_json::json;
use tempfile::TempDir;

fn make_ctx(dir: &TempDir) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        working_dir: dir.path().to_path_buf(),
    }
}

#[tokio::test]
async fn test_shell_captures_stdout() {
    let dir = TempDir::new().unwrap();
    let tool = ShellTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "command": "echo hello" }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    assert_eq!(output.value["stdout"], "hello\n");
}

#[tokio::test]
async fn test_shell_captures_stderr() {
    let dir = TempDir::new().unwrap();
    let tool = ShellTool;
    let ctx = make_ctx(&dir);
    // Write to stderr explicitly.
    let input = ToolInput::new(json!({ "command": "echo error_msg >&2" }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    assert_eq!(output.value["stderr"], "error_msg\n");
}

#[tokio::test]
async fn test_shell_exit_code() {
    let dir = TempDir::new().unwrap();
    let tool = ShellTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "command": "false" }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    let exit_code = output.value["exit_code"].as_i64().expect("exit_code should be integer");
    assert_ne!(exit_code, 0, "exit code of `false` should be non-zero");
}

#[tokio::test]
async fn test_shell_timeout() {
    let dir = TempDir::new().unwrap();
    let tool = ShellTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "command": "sleep 10", "timeout_ms": 100 }));

    let err = tool.call(input, &ctx).await.expect_err("should time out");
    assert!(
        matches!(err, hone_tools::ToolError::Timeout { .. }),
        "expected Timeout error, got: {err:?}"
    );
}
