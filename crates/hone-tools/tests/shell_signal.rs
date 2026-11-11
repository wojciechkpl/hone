#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! W37 of `docs/plans/worker-tools-spec.md`: on SIGINT/SIGTERM/SIGHUP a one-shot
//! run kills its running commands, then restores the signal's default action
//! and re-raises it, so the process ends *by* the signal (not `exit(130)`).
//!
//! Assumed API: `hone_tools::kill_commands_and_reraise(signal: i32) -> !`
//! (calls `kill_running_commands()`, resets the handler to default, raises the
//! signal; `exit(128 + signal)` only as a fallback). It ends the calling
//! process, so it is tested in a child: this test binary re-executes itself
//! running only `w37_child_entry`, selected by environment variables.

use std::os::unix::process::ExitStatusExt;
use std::time::{Duration, Instant};

const SIGNAL_VAR: &str = "HONE_W37_SIGNAL";
const FLAG_FILE_VAR: &str = "HONE_W37_FLAG_FILE";

/// The child's body. A no-op when run normally by `cargo test`.
#[test]
fn w37_child_entry() {
    let Ok(signal) = std::env::var(SIGNAL_VAR) else {
        return;
    };
    let signal: i32 = signal.parse().unwrap();
    let flag_file = std::env::var(FLAG_FILE_VAR).unwrap();

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(format!("(sleep 1; touch '{flag_file}') & sleep 5"));
        let _running = tokio::spawn(hone_tools::run_command(cmd, None));
        tokio::time::sleep(Duration::from_millis(300)).await;
        hone_tools::kill_commands_and_reraise(signal);
    });
    // Only reached if the handler returned: the parent then sees a normal exit.
}

fn run_child(
    signal: i32,
) -> (
    std::process::ExitStatus,
    std::path::PathBuf,
    tempfile::TempDir,
) {
    let dir = tempfile::tempdir().unwrap();
    let flag_file = dir.path().join("flag");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "w37_child_entry",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(SIGNAL_VAR, signal.to_string())
        .env(FLAG_FILE_VAR, &flag_file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().ok();
            panic!("the child did not end after the signal handler ran");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    (status, flag_file, dir)
}

fn assert_dies_by(signal: i32) {
    let (status, flag_file, _dir) = run_child(signal);

    assert_eq!(
        status.signal(),
        Some(signal),
        "the process must end BY the signal (status {status:?})"
    );
    assert_eq!(status.code(), None, "not a normal exit with 128 + n");
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        !flag_file.exists(),
        "the running command's grandchild must have been killed first"
    );
}

#[test]
fn test_w37_sigint_should_kill_commands_and_end_the_process_by_sigint() {
    assert_dies_by(2);
}

#[test]
fn test_w37_sigterm_should_kill_commands_and_end_the_process_by_sigterm() {
    assert_dies_by(15);
}

#[test]
fn test_w37_sighup_should_kill_commands_and_end_the_process_by_sighup() {
    assert_dies_by(1);
}
