#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Regression: a nonexistent path in the policy must be skipped by the
//! Landlock backend (read, write and exec rules) instead of failing the whole
//! ruleset inside the child's `pre_exec`.
//!
//! Linux-only; skipped at runtime when Landlock is not enabled.
#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

use hone_sandbox::{LandlockSandbox, Sandbox, SandboxMode, SandboxPolicy};

fn landlock_available() -> bool {
    std::fs::read_to_string("/sys/kernel/security/lsm")
        .map(|s| s.contains("landlock"))
        .unwrap_or(false)
}

fn base_policy() -> SandboxPolicy {
    let mut p = SandboxPolicy::permissive();
    p.mode = SandboxMode::Autonomous;
    p.allow_read = ["/bin", "/usr", "/lib", "/lib64", "/etc"]
        .iter()
        .map(PathBuf::from)
        .filter(|x| x.exists())
        .collect();
    p.allow_write = vec![];
    p.allow_exec = vec![];
    p
}

fn run_ok(policy: &SandboxPolicy) {
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo ok"]);
    LandlockSandbox.restrict_child(&mut cmd, policy).unwrap();
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}

#[test]
fn test_landlock_when_allow_read_has_missing_path_should_skip_it() {
    if !landlock_available() {
        eprintln!("skipping: landlock not enabled");
        return;
    }
    let mut p = base_policy();
    p.allow_read
        .push(PathBuf::from("/definitely/not/here-read"));
    run_ok(&p);
}

#[test]
fn test_landlock_when_allow_write_has_missing_path_should_skip_it() {
    if !landlock_available() {
        eprintln!("skipping: landlock not enabled");
        return;
    }
    let mut p = base_policy();
    p.allow_write
        .push(PathBuf::from("/definitely/not/here-write"));
    run_ok(&p);
}

#[test]
fn test_landlock_when_allow_exec_has_missing_path_should_skip_it() {
    if !landlock_available() {
        eprintln!("skipping: landlock not enabled");
        return;
    }
    let mut p = base_policy();
    p.allow_exec
        .push(PathBuf::from("/definitely/not/here-exec"));
    run_ok(&p);
}

#[test]
fn test_landlock_when_allow_write_has_device_file_should_enforce_and_allow_redirect() {
    if !landlock_available() {
        eprintln!("skipping: landlock not enabled");
        return;
    }
    let work = tempfile::Builder::new()
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let mut p = base_policy();
    p.allow_write = vec![work.path().to_path_buf(), PathBuf::from("/dev/null")];
    let mut cmd = Command::new("sh");
    cmd.args(["-c", "echo x >/dev/null; echo ok"])
        .current_dir(work.path());
    LandlockSandbox.restrict_child(&mut cmd, &p).unwrap();
    let out = cmd.output().expect("spawn");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "ok");
}
