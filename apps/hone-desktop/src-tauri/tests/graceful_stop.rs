#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! G1 and G2 of `docs/plans/desktop-graceful-stop-spec.md`: stopping the managed
//! `honed` is gentle, then firm, and `honed` is started with a short drain.
//!
//! G1, on every stop path (`shutdown(self)`, `Drop`, the kill after a failed
//! `start`): SIGTERM, wait up to about 3 s for the exit; if it is still running
//! a second SIGTERM (`honed`'s force-stop), wait up to about 2 s; only then
//! SIGKILL. The child is always reaped. A child that returns early ends the stop
//! at once (no fixed sleep). A child that already exited is never signalled.
//!
//! G2: `SidecarSupervisor::spawn_with` / `start` give the child
//! `HONE_SHUTDOWN_GRACE_SECS=1` unless `extra_env` carries that variable (the app
//! passes its own environment there), which wins.
//!
//! No new names are needed: the behaviour changes inside `shutdown`, `Drop`,
//! `start` and `spawn_with`.
//!
//! The fake `honed` is a Python script (a shell `trap` would not run while a
//! foreground `sleep` is running). Its behaviour comes from `extra_env`:
//! `FAKE_MODE` = `polite` (exits on the first SIGTERM), `second` (exits on the
//! second) or `deaf` (never exits on SIGTERM); `FAKE_HEALTH=0` = never serves
//! `/health`. It writes `<dir>/pid`, `<dir>/env` (its environment) and one line
//! `TERM <unix time>` per SIGTERM to `<dir>/signals`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hone_desktop_core::supervisor::SidecarSupervisor;

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

const FAKE: &str = r#"#!/usr/bin/env python3
import http.server, os, signal, sys, time
d = os.path.dirname(os.path.abspath(__file__))
mode = os.environ.get("FAKE_MODE", "polite")
count = [0]
def on_term(*a):
    count[0] += 1
    open(os.path.join(d, "signals"), "a").write("TERM %f\n" % time.time())
    if mode == "polite" or (mode == "second" and count[0] >= 2):
        os._exit(0)
signal.signal(signal.SIGTERM, on_term)
open(os.path.join(d, "env"), "w").write("\n".join("%s=%s" % kv for kv in os.environ.items()))
open(os.path.join(d, "pid"), "w").write(str(os.getpid()))
if os.environ.get("FAKE_HEALTH") == "0":
    while True:
        time.sleep(1)
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b"ok")
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
"#;

fn script(dir: &Path) -> PathBuf {
    let path = dir.join("honed");
    std::fs::write(&path, FAKE).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn env_of(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

/// SIGKILLs the recorded pid at the end, whatever the test did.
struct Reaper(PathBuf);

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Some(pid) = pid(&self.0) {
            let _ = Command::new("kill")
                .args(["-9", &pid])
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn pid(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("pid"))
        .ok()
        .map(|s| s.trim().to_owned())
}

/// The process no longer exists at all: not running and not an unreaped zombie
/// (`kill -0` still succeeds for a zombie).
fn reaped(pid: &str) -> bool {
    !Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// Timestamps of the SIGTERMs the fake received.
fn terms(dir: &Path) -> Vec<f64> {
    std::fs::read_to_string(dir.join("signals"))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| l.strip_prefix("TERM ")?.trim().parse().ok())
        .collect()
}

fn start(dir: &Path, mode: &str) -> SidecarSupervisor {
    SidecarSupervisor::start(
        Some(script(dir)),
        env_of(&[("FAKE_MODE", mode)]),
        Duration::from_secs(10),
    )
    .unwrap_or_else(|e| panic!("the fake honed must start: {e}"))
}

// ---- G1: shutdown ---------------------------------------------------------------

#[test]
fn test_g1_shutdown_of_a_honed_that_exits_on_sigterm_should_return_at_once_after_one_term() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _r = Reaper(dir.path().to_path_buf());
    let sup = start(dir.path(), "polite");

    let t = Instant::now();
    sup.shutdown().unwrap();
    let took = t.elapsed();

    assert_eq!(
        terms(dir.path()).len(),
        1,
        "exactly one SIGTERM, no SIGKILL first"
    );
    assert!(
        took < Duration::from_secs(1),
        "no fixed sleep; took {took:?}"
    );
    assert!(reaped(&pid(dir.path()).unwrap()));
}

#[test]
fn test_g1_shutdown_should_send_a_second_term_after_about_three_seconds() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _r = Reaper(dir.path().to_path_buf());
    let sup = start(dir.path(), "second");

    let t = Instant::now();
    sup.shutdown().unwrap();
    let took = t.elapsed();

    let terms = terms(dir.path());
    assert_eq!(terms.len(), 2, "TERM, then a second TERM: {terms:?}");
    let gap = terms[1] - terms[0];
    assert!(
        (2.5..4.0).contains(&gap),
        "second TERM {gap:.2} s after the first"
    );
    assert!(
        took < Duration::from_millis(4500),
        "returns once it exits; took {took:?}"
    );
    assert!(reaped(&pid(dir.path()).unwrap()));
}

#[test]
fn test_g1_shutdown_of_a_deaf_honed_should_kill_it_after_about_five_seconds() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _r = Reaper(dir.path().to_path_buf());
    let sup = start(dir.path(), "deaf");

    let t = Instant::now();
    sup.shutdown().unwrap();
    let took = t.elapsed();

    let terms = terms(dir.path());
    assert_eq!(terms.len(), 2, "two SIGTERMs before the SIGKILL: {terms:?}");
    assert!(
        (4.5..7.5).contains(&took.as_secs_f64()),
        "3 s + 2 s then SIGKILL; took {took:?}"
    );
    assert!(reaped(&pid(dir.path()).unwrap()), "always reaped");
}

#[test]
fn test_g1_shutdown_of_an_already_exited_child_should_not_wait_or_fail() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _r = Reaper(dir.path().to_path_buf());
    let mut sup = start(dir.path(), "deaf");
    let p = pid(dir.path()).unwrap();
    let _ = Command::new("kill").args(["-9", &p]).status();
    let deadline = Instant::now() + Duration::from_secs(3);
    while sup.is_alive() {
        assert!(Instant::now() < deadline, "the killed child never ended");
        std::thread::sleep(Duration::from_millis(20));
    }

    let t = Instant::now();
    sup.shutdown().unwrap();

    assert!(
        t.elapsed() < Duration::from_millis(500),
        "nothing to wait for"
    );
    assert!(terms(dir.path()).is_empty(), "a dead child gets no signal");
}

// ---- G1: Drop ----------------------------------------------------------------------

#[test]
fn test_g1_dropping_the_supervisor_should_send_sigterm_first() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _r = Reaper(dir.path().to_path_buf());
    let sup = start(dir.path(), "polite");

    let t = Instant::now();
    drop(sup);

    assert_eq!(terms(dir.path()).len(), 1, "Drop stops it gently too");
    assert!(t.elapsed() < Duration::from_secs(1));
    assert!(reaped(&pid(dir.path()).unwrap()));
}

#[test]
fn test_g1_dropping_the_supervisor_of_a_deaf_honed_should_still_end_it() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _r = Reaper(dir.path().to_path_buf());
    let sup = start(dir.path(), "deaf");

    drop(sup);

    assert_eq!(terms(dir.path()).len(), 2);
    assert!(reaped(&pid(dir.path()).unwrap()));
}

// ---- G1: the failed-start path -----------------------------------------------------

#[test]
fn test_g1_failed_start_should_sigterm_the_child_before_anything_harsher() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _r = Reaper(dir.path().to_path_buf());

    let t = Instant::now();
    let result = SidecarSupervisor::start(
        Some(script(dir.path())),
        env_of(&[("FAKE_MODE", "polite"), ("FAKE_HEALTH", "0")]),
        Duration::from_millis(800),
    );
    let took = t.elapsed();

    assert!(result.is_err(), "it never answers /health");
    assert_eq!(
        terms(dir.path()).len(),
        1,
        "the child got a SIGTERM (a direct SIGKILL records none)"
    );
    assert!(took < Duration::from_secs(3), "took {took:?}");
    assert!(reaped(&pid(dir.path()).unwrap()));
}

// ---- G2: the drain is short ---------------------------------------------------------

fn child_env(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("env")).unwrap()
}

#[test]
fn test_g2_honed_should_get_a_one_second_drain_by_default() {
    if std::env::var_os("HONE_SHUTDOWN_GRACE_SECS").is_some() {
        return; // the child would inherit the developer's own value
    }
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _r = Reaper(dir.path().to_path_buf());
    let sup = start(dir.path(), "polite");

    let env = child_env(dir.path());
    sup.shutdown().unwrap();

    assert!(
        env.lines().any(|l| l == "HONE_SHUTDOWN_GRACE_SECS=1"),
        "child env: {env}"
    );
}

#[test]
fn test_g2_the_apps_own_value_should_be_passed_through() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _r = Reaper(dir.path().to_path_buf());
    let sup = SidecarSupervisor::start(
        Some(script(dir.path())),
        env_of(&[("FAKE_MODE", "polite"), ("HONE_SHUTDOWN_GRACE_SECS", "7")]),
        Duration::from_secs(10),
    )
    .unwrap();

    let env = child_env(dir.path());
    sup.shutdown().unwrap();

    assert!(
        env.lines().any(|l| l == "HONE_SHUTDOWN_GRACE_SECS=7"),
        "child env: {env}"
    );
    assert!(!env.lines().any(|l| l == "HONE_SHUTDOWN_GRACE_SECS=1"));
}
