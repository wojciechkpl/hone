//! Integration tests for the Linux Landlock sandbox backend.
//!
//! These tests are Linux-only (`#![cfg(target_os = "linux")]`).  On macOS and
//! other platforms the entire file is compiled away.
//!
//! Tests that call `restrict_child` spawn real child processes and assert on
//! their exit status.  Tests that call `apply` are destructive to the current
//! process and are intentionally left as documentation-only (see the comment in
//! `test_apply_is_destructive`).
//!
//! # Running
//!
//! ```text
//! cargo test -p hone-sandbox --test landlock
//! ```
//!
//! Tests are silently skipped on kernels that do not support Landlock (< 5.13
//! or `CONFIG_SECURITY_LANDLOCK` not set).

#![cfg(target_os = "linux")]

use hone_sandbox::{LandlockSandbox, Sandbox, SandboxMode, SandboxPolicyBuilder};
use std::path::PathBuf;
use std::process::Command;

/// Return `true` when the running kernel advertises Landlock support via the
/// ABI version pseudo-file added in Linux 5.13.
fn landlock_supported() -> bool {
    std::fs::read_to_string("/proc/sys/fs/landlock/abi")
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|v| v >= 1)
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Informational test: apply() on the current process
// ---------------------------------------------------------------------------

/// Calling `apply()` restricts the **current** process, which would break the
/// rest of the test runner.  This test just verifies the builder wiring by
/// checking that the call site compiles and returns a result without panicking.
/// It is intentionally not `#[test]` — it serves as a compile-time smoke check.
#[allow(dead_code)]
fn _apply_compiles_and_returns_result() {
    if !landlock_supported() {
        return;
    }
    let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
        .allow_read(PathBuf::from("/tmp"))
        .build()
        .expect("build policy");
    let sandbox = LandlockSandbox;
    // We do NOT call sandbox.apply(&policy) — that would restrict the test runner.
    let _ = (sandbox, policy); // suppress unused-variable warnings
}

// ---------------------------------------------------------------------------
// restrict_child tests
// ---------------------------------------------------------------------------

/// A sandboxed child that only has read access to `/tmp` must NOT be able to
/// read `/etc/passwd`.
#[test]
fn test_restrict_child_blocks_unauthorized_read() {
    if !landlock_supported() {
        eprintln!("Landlock not supported on this kernel, skipping");
        return;
    }

    let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
        .allow_read(PathBuf::from("/tmp")) // only /tmp is readable
        .build()
        .expect("build policy");

    let sandbox = LandlockSandbox;

    let mut cmd = Command::new("cat");
    cmd.arg("/etc/passwd");
    cmd.stdout(std::process::Stdio::null());
    cmd.stderr(std::process::Stdio::null());

    sandbox.restrict_child(&mut cmd, &policy).expect("restrict_child");

    let status = cmd.status().expect("spawn child");
    assert!(
        !status.success(),
        "cat /etc/passwd should be blocked by Landlock (got exit status {status})"
    );
}

/// A sandboxed child that has read access to `/tmp` MUST be able to read a
/// file inside `/tmp`.
#[test]
fn test_restrict_child_allows_authorized_read() {
    if !landlock_supported() {
        eprintln!("Landlock not supported on this kernel, skipping");
        return;
    }

    let test_file = "/tmp/hone_landlock_test_read.txt";
    std::fs::write(test_file, "hone landlock test").expect("write test file");

    let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
        .allow_read(PathBuf::from("/tmp"))
        .build()
        .expect("build policy");

    let sandbox = LandlockSandbox;

    let mut cmd = Command::new("cat");
    cmd.arg(test_file);
    cmd.stdout(std::process::Stdio::null());

    sandbox.restrict_child(&mut cmd, &policy).expect("restrict_child");

    let status = cmd.status().expect("spawn child");

    // Clean up before asserting so the file is always removed.
    let _ = std::fs::remove_file(test_file);

    assert!(
        status.success(),
        "cat of an allowed /tmp file should succeed (got exit status {status})"
    );
}

/// A child that has write access to `/tmp` must be able to create files there.
#[test]
fn test_restrict_child_allows_authorized_write() {
    if !landlock_supported() {
        eprintln!("Landlock not supported on this kernel, skipping");
        return;
    }

    let test_file = "/tmp/hone_landlock_test_write.txt";
    // Remove any leftover from a previous run.
    let _ = std::fs::remove_file(test_file);

    let policy = SandboxPolicyBuilder::new(SandboxMode::Manual)
        .allow_write(PathBuf::from("/tmp")) // write implies read
        .build()
        .expect("build policy");

    let sandbox = LandlockSandbox;

    // Use `sh -c` so we can redirect output to a file in a single child.
    let mut cmd = Command::new("sh");
    cmd.args(["-c", &format!("echo ok > {test_file}")]);

    sandbox.restrict_child(&mut cmd, &policy).expect("restrict_child");

    let status = cmd.status().expect("spawn child");

    let content = std::fs::read_to_string(test_file).ok();
    let _ = std::fs::remove_file(test_file);

    assert!(
        status.success(),
        "write to an allowed /tmp file should succeed (got exit status {status})"
    );
    assert_eq!(
        content.as_deref().map(str::trim),
        Some("ok"),
        "written content should match"
    );
}
