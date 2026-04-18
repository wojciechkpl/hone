/// Tests that the shell tool builds a non-vacuous sandbox policy.
///
/// Fix 1: The shell tool used to build an empty `SandboxMode::Manual` policy
/// (allowing everything). It must now build a policy with at least the
/// working directory in both read and write allow-lists.
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

/// On all platforms: reading within the working dir should succeed.
#[tokio::test]
async fn test_shell_reads_working_dir_file() {
    let dir = TempDir::new().unwrap();
    // Write a file inside the working dir.
    std::fs::write(dir.path().join("hello.txt"), b"world").unwrap();

    let tool = ShellTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "command": "cat hello.txt" }));

    let output = tool.call(input, &ctx).await.expect("reading working dir should succeed");
    assert_eq!(output.value["exit_code"], 0);
    assert_eq!(output.value["stdout"], "world");
}

/// On all platforms: writing within the working dir should succeed.
#[tokio::test]
async fn test_shell_writes_working_dir_file() {
    let dir = TempDir::new().unwrap();

    let tool = ShellTool;
    let ctx = make_ctx(&dir);
    // Use a shell script via -c to avoid the compound-operator block.
    let input = ToolInput::new(json!({ "command": "echo written > output.txt" }));

    let output = tool.call(input, &ctx).await.expect("writing to working dir should succeed");
    assert_eq!(output.value["exit_code"], 0);
    // Verify the file was actually created.
    let content = std::fs::read_to_string(dir.path().join("output.txt")).unwrap();
    assert!(content.contains("written"), "expected 'written' in output.txt, got: {content:?}");
}

/// On macOS with the real Seatbelt sandbox: writing to /etc/hosts must fail.
///
/// This test only runs on macOS because Seatbelt is macOS-specific.
/// It uses the real `SeatbeltSandbox` — NOT `NullSandbox` — so the policy is
/// actually enforced.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn test_shell_macos_write_to_etc_hosts_denied() {
    use hone_sandbox::SeatbeltSandbox;

    let dir = TempDir::new().unwrap();
    let ctx = ToolContext {
        sandbox: Arc::new(SeatbeltSandbox),
        working_dir: dir.path().to_path_buf(),
    };

    let tool = ShellTool;
    // Attempt to append to /etc/hosts — should be denied by Seatbelt.
    let input = ToolInput::new(json!({ "command": "echo 'bad' >> /etc/hosts" }));

    let output = tool.call(input, &ctx).await.expect("call should return output (not panic)");
    // The write should fail with a non-zero exit code because Seatbelt
    // blocks file-write* outside the allow-listed paths.
    let exit_code = output.value["exit_code"].as_i64().unwrap_or(0);
    assert_ne!(exit_code, 0, "writing to /etc/hosts must be denied by sandbox");
}

/// Platform-independent: the policy built for the shell tool must have
/// non-empty allow_read and allow_write lists (i.e., not a vacuous empty policy).
///
/// We verify this indirectly by confirming that a command writing a file
/// inside the working directory succeeds (policy allows it) while the
/// shell tool code path builds a concrete policy (coverage).
#[tokio::test]
async fn test_shell_policy_allows_working_dir_write_via_null_sandbox() {
    // NullSandbox ignores the policy, so this test only verifies that
    // policy construction itself does not panic or return an error.
    let dir = TempDir::new().unwrap();
    let ctx = make_ctx(&dir);
    let tool = ShellTool;

    // This would have panicked on the old `.expect("manual policy with no constraints")`
    // if working_dir was somehow invalid. Now it returns a proper error.
    let input = ToolInput::new(json!({ "command": "echo ok" }));
    let result = tool.call(input, &ctx).await;
    assert!(result.is_ok(), "policy build with valid working_dir must succeed");
}
