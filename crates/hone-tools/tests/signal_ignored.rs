#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! W41 of `docs/plans/worker-tools-spec.md`: a signal that is ignored when the
//! process starts (SIGHUP under `nohup`, SIGINT for a background job) must stay
//! ignored: hone installs its handler for a signal only if it isn't ignored.
//!
//! Assumed API: `hone_tools::signal_is_ignored(signal: i32) -> bool`
//! (`sigaction` with a null new action, checking for `SIG_IGN`). The binary's
//! own wiring (it must call this before installing each handler) cannot be
//! unit tested; these tests pin the helper and the "check first, then install
//! the handler that calls `kill_commands_and_reraise`" pattern.
//!
//! Like `shell_signal.rs`, the test binary re-executes itself as the child.

use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::time::{Duration, Instant};

const MODE_VAR: &str = "HONE_W41_MODE";
const REPORT_VAR: &str = "HONE_W41_REPORT";

/// Child body. Modes: `report` writes which of SIGHUP/SIGINT/SIGTERM it sees
/// as ignored to a file; `handlers` installs hone's one-shot handlers the way
/// the binary does (only for signals that are not ignored) and waits.
#[test]
fn w41_child_entry() {
    let Ok(mode) = std::env::var(MODE_VAR) else {
        return;
    };
    if mode == "report" {
        let report: Vec<String> = [1, 2, 15]
            .iter()
            .map(|s| format!("{s}={}", hone_tools::signal_is_ignored(*s)))
            .collect();
        std::fs::write(std::env::var(REPORT_VAR).unwrap(), report.join(",")).unwrap();
        return;
    }
    // Decide BEFORE tokio installs its own handlers.
    let to_handle: Vec<i32> = [1, 2, 15]
        .into_iter()
        .filter(|s| !hone_tools::signal_is_ignored(*s))
        .collect();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        use tokio::signal::unix::{SignalKind, signal};
        for sig in to_handle {
            let mut stream = signal(SignalKind::from_raw(sig)).unwrap();
            tokio::spawn(async move {
                stream.recv().await;
                hone_tools::kill_commands_and_reraise(sig);
            });
        }
        tokio::time::sleep(Duration::from_millis(2000)).await;
    });
    // Survived: normal exit 0.
}

fn spawn_child(
    mode: &str,
    ignore: &[i32],
    report: Option<&std::path::Path>,
) -> std::process::Child {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "w41_child_entry", "--test-threads=1"])
        .env(MODE_VAR, mode)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Some(r) = report {
        cmd.env(REPORT_VAR, r);
    }
    let ignore = ignore.to_vec();
    // What `nohup` (SIGHUP) or a shell background job (SIGINT) does: the signal
    // is ignored across exec.
    // SAFETY: only async-signal-safe `signal` calls between fork and exec.
    unsafe {
        cmd.pre_exec(move || {
            for s in &ignore {
                libc::signal(*s, libc::SIG_IGN);
            }
            Ok(())
        });
    }
    cmd.spawn().unwrap()
}

fn wait(mut child: std::process::Child) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        if start.elapsed() > Duration::from_secs(10) {
            child.kill().ok();
            panic!("child hung");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn report_for(ignore: &[i32]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("report");
    let status = wait(spawn_child("report", ignore, Some(&file)));
    assert!(status.success(), "{status:?}");
    std::fs::read_to_string(&file).unwrap()
}

#[test]
fn test_w41_signal_is_ignored_should_report_sighup_ignored_under_nohup() {
    assert_eq!(report_for(&[libc::SIGHUP]), "1=true,2=false,15=false");
}

#[test]
fn test_w41_signal_is_ignored_should_report_sigint_ignored_for_a_background_job() {
    assert_eq!(report_for(&[libc::SIGINT]), "1=false,2=true,15=false");
}

#[test]
fn test_w41_signal_is_ignored_should_report_nothing_ignored_by_default() {
    assert_eq!(report_for(&[]), "1=false,2=false,15=false");
}

#[test]
fn test_w41_ignored_sighup_should_not_end_a_process_that_installs_handlers_the_hone_way() {
    let child = spawn_child("handlers", &[libc::SIGHUP], None);
    std::thread::sleep(Duration::from_millis(500));
    // SAFETY: plain kill(2) on our own child.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGHUP);
    }

    let status = wait(child);

    assert_eq!(
        status.signal(),
        None,
        "SIGHUP must stay ignored: {status:?}"
    );
    assert_eq!(status.code(), Some(0));
}

#[test]
fn test_w41_sigterm_should_still_end_that_process_by_the_signal() {
    let child = spawn_child("handlers", &[libc::SIGHUP], None);
    std::thread::sleep(Duration::from_millis(500));
    // SAFETY: plain kill(2) on our own child.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }

    let status = wait(child);

    assert_eq!(status.signal(), Some(libc::SIGTERM), "{status:?}");
}

#[test]
fn test_w41_sighup_should_end_a_process_that_did_not_start_ignoring_it() {
    let child = spawn_child("handlers", &[], None);
    std::thread::sleep(Duration::from_millis(500));
    // SAFETY: plain kill(2) on our own child.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGHUP);
    }

    let status = wait(child);

    assert_eq!(status.signal(), Some(libc::SIGHUP), "{status:?}");
}
