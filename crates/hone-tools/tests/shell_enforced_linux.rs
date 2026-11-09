#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! End-to-end: with `HONE_SANDBOX_ENFORCE=1` the shell tool really runs under
//! Landlock. Linux-only; skipped without Landlock.
//!
//! Everything lives in ONE test because it mutates process env and this file
//! is its own test process.
#![cfg(target_os = "linux")]

use std::sync::Arc;

use hone_sandbox::{LandlockSandbox, SandboxPolicy};
use hone_tools::{ShellTool, Tool, ToolContext, ToolInput};
use serde_json::json;

const MARKER: &str = "TOP-SECRET-MARKER-7f3a";

#[tokio::test]
async fn test_shell_when_enforced_should_allow_dev_null_and_deny_reading_home_secret() {
    let lsm = std::fs::read_to_string("/sys/kernel/security/lsm").unwrap_or_default();
    if !lsm.contains("landlock") {
        eprintln!("skipping: landlock not enabled");
        return;
    }
    // Not /tmp: /tmp is writable (and readable) inside the sandbox.
    let home = tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let work = tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let secret = home.path().join("secret.txt");
    std::fs::write(&secret, MARKER).unwrap();

    // SAFETY: single test in this process; no other threads read env concurrently.
    unsafe {
        std::env::set_var("HONE_SANDBOX_ENFORCE", "1");
        std::env::set_var("HOME", home.path());
    }

    let ctx = ToolContext {
        sandbox: Arc::new(LandlockSandbox),
        sandbox_policy: Arc::new(SandboxPolicy::permissive()),
        working_dir: work.path().to_path_buf(),
    };

    let out = ShellTool
        .call(
            ToolInput::new(json!({ "command": "echo x >/dev/null" })),
            &ctx,
        )
        .await
        .expect("dev-null redirect should run");
    assert_eq!(out.value["exit_code"], 0, "{:?}", out.value);
    assert_eq!(out.value["stderr"], "", "{:?}", out.value);

    let cmd = format!("cat {}", secret.display());
    let res = ShellTool
        .call(ToolInput::new(json!({ "command": cmd })), &ctx)
        .await;
    match res {
        Err(_) => {}
        Ok(out) => {
            assert_ne!(out.value["exit_code"], 0, "{:?}", out.value);
            let stdout = out.value["stdout"].as_str().unwrap_or("");
            assert!(!stdout.contains(MARKER), "secret leaked: {stdout}");
        }
    }
}
