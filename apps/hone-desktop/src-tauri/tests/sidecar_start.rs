#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! A1-A3 of `docs/plans/desktop-sidecar-autostart-spec.md` at the public API of
//! `hone_desktop_core`: the autostart decision, and starting a `honed` (here a
//! fake script) so that a failure explains itself and leaves nothing running.
//!
//! API this file requires (new; it fails to compile until it exists):
//!
//! ```ignore
//! // commands.rs
//! /// A1. Pure. The ACTIVE PROFILE (`cfg.active_profile()`, which falls back to
//! /// the first one) is authoritative when there is one: sidecar -> true,
//! /// external -> false, whatever the saved mode says. With no profile, the
//! /// web view's saved mode decides: `Some("sidecar")` -> true; `Some("external")`,
//! /// any other string and `None` -> false.
//! pub fn should_autostart(saved_mode: Option<&str>, cfg: &DesktopConfig) -> bool;
//!
//! // supervisor.rs
//! impl SidecarSupervisor {
//!     /// `spawn_with` + health wait in one step. On any failure the child is
//!     /// killed and reaped BEFORE the error is returned (A3). It also notices a
//!     /// child that exits early: it fails at once (A2), without waiting for
//!     /// `health_timeout`.
//!     pub fn start<I, K, V>(
//!         honed_path: Option<PathBuf>,
//!         extra_env: I,
//!         health_timeout: Duration,
//!     ) -> Result<Self, SupervisorError>;
//! }
//! ```
//!
//! `SupervisorError`'s `Display` text is the contract for A2:
//! - the program is missing: contains `not found on PATH` and the program name;
//! - it exited early: contains `exit status: <n>` (the `ExitStatus` display) and
//!   the last lines it wrote to stderr;
//! - it never became healthy: still the `HealthTimeout` text ("did not become
//!   healthy").
//! `honed`'s stderr is piped and drained (a chatty child must not block).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use hone_desktop_core::commands::should_autostart;
use hone_desktop_core::config::{DaemonMode, DaemonProfile, DesktopConfig};
use hone_desktop_core::supervisor::SidecarSupervisor;

/// Writing a script and executing it from parallel threads races with fork
/// (ETXTBSY), so every test that runs a fake takes this.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn no_env() -> std::iter::Empty<(String, String)> {
    std::iter::empty()
}

fn script(dir: &Path, body: &str) -> PathBuf {
    let path = dir.join("honed");
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// SIGKILLs the pid the fake recorded in `<dir>/pid`, whatever the test did.
struct Reaper(PathBuf);

impl Drop for Reaper {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(self.0.join("pid")) {
            let _ = Command::new("kill")
                .args(["-9", pid.trim()])
                .stderr(Stdio::null())
                .status();
        }
    }
}

fn alive(pid: &str) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

/// Waits (bounded) for the recorded pid to be gone.
fn gone_within(dir: &Path, limit: Duration) -> bool {
    let pid = std::fs::read_to_string(dir.join("pid"))
        .expect("the fake never recorded its pid")
        .trim()
        .to_owned();
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if !alive(&pid) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    !alive(&pid)
}

/// A `honed` that never answers /health: records its pid, then sleeps.
const HANGING: &str = "#!/bin/sh\necho $$ > \"$(dirname \"$0\")/pid\"\nexec sleep 300\n";

/// A `honed` that complains on stderr and exits 2.
const EXITS_2: &str =
    "#!/bin/sh\necho 'starting' >&2\necho 'boom: port already in use' >&2\nexit 2\n";

/// A healthy `honed` that first writes ~300 KB to stderr (more than a pipe
/// holds), so a stderr nobody drains would block it before it serves.
const CHATTY_HEALTHY: &str = r#"#!/usr/bin/env python3
import http.server, os, sys
d = os.path.dirname(os.path.abspath(__file__))
open(os.path.join(d, "pid"), "w").write(str(os.getpid()))
for i in range(3000):
    sys.stderr.write("log line %d %s\n" % (i, "x" * 90))
sys.stderr.flush()
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.end_headers(); self.wfile.write(b"ok")
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
"#;

// ---- A1: the autostart decision ----------------------------------------------

fn cfg_with(mode: Option<DaemonMode>) -> DesktopConfig {
    let mut cfg = DesktopConfig::default();
    if let Some(mode) = mode {
        cfg.add_profile(DaemonProfile {
            name: "p".into(),
            mode,
        })
        .unwrap();
    }
    cfg
}

fn external() -> DaemonMode {
    DaemonMode::External {
        url: "http://10.0.0.5:3117".into(),
        token: None,
    }
}

#[test]
fn test_a1_should_autostart_for_every_saved_mode_and_active_profile() {
    // (saved mode in local storage, active profile, expected)
    let table: Vec<(Option<&str>, Option<DaemonMode>, bool)> = vec![
        // No profile: the saved mode decides.
        (Some("sidecar"), None, true),
        (Some("external"), None, false),
        (Some("something-else"), None, false),
        (None, None, false),
        // An active sidecar profile starts it, whatever local storage says.
        (Some("sidecar"), Some(DaemonMode::Sidecar), true),
        (Some("external"), Some(DaemonMode::Sidecar), true),
        (None, Some(DaemonMode::Sidecar), true),
        // An active external profile never starts anything.
        (Some("sidecar"), Some(external()), false),
        (Some("external"), Some(external()), false),
        (None, Some(external()), false),
    ];
    for (saved, profile, expected) in table {
        let cfg = cfg_with(profile.clone());
        assert_eq!(
            should_autostart(saved, &cfg),
            expected,
            "saved mode {saved:?}, active profile {profile:?}"
        );
    }
}

#[test]
fn test_a1_active_pointer_should_pick_the_profile_not_the_first_one() {
    let mut cfg = cfg_with(Some(DaemonMode::Sidecar));
    cfg.add_profile(DaemonProfile {
        name: "team".into(),
        mode: external(),
    })
    .unwrap();
    cfg.set_active("team").unwrap();
    assert!(!should_autostart(Some("sidecar"), &cfg));
    cfg.set_active("p").unwrap();
    assert!(should_autostart(None, &cfg));
}

// ---- A2: a failed start says why -----------------------------------------------

#[test]
fn test_a2_missing_binary_should_say_it_is_not_found_on_path() {
    let name = "honed-definitely-missing-xyz";
    let err = SidecarSupervisor::start(Some(PathBuf::from(name)), no_env(), Duration::from_secs(5))
        .err()
        .expect("starting a missing binary must fail")
        .to_string();
    assert!(err.contains("not found on PATH"), "got: {err}");
    assert!(err.contains(name), "names the program; got: {err}");
}

#[test]
fn test_a2_early_exit_should_report_status_and_stderr_within_a_second() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let honed = script(dir.path(), EXITS_2);

    let start = Instant::now();
    let err = SidecarSupervisor::start(Some(honed), no_env(), Duration::from_secs(5))
        .err()
        .expect("a honed that exits 2 must fail the start")
        .to_string();
    let took = start.elapsed();

    assert!(
        took < Duration::from_millis(1500),
        "took {took:?}, not the 5 s health wait"
    );
    assert!(err.contains("exit status: 2"), "got: {err}");
    assert!(err.contains("boom: port already in use"), "got: {err}");
}

#[test]
fn test_a2_stderr_should_be_drained_so_a_chatty_healthy_honed_still_starts() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _reaper = Reaper(dir.path().to_path_buf());
    let honed = script(dir.path(), CHATTY_HEALTHY);

    let sup = SidecarSupervisor::start(Some(honed), no_env(), Duration::from_secs(10))
        .unwrap_or_else(|e| panic!("a healthy chatty honed must start: {e}"));
    sup.shutdown().unwrap();
}

#[test]
fn test_a2_health_timeout_should_still_say_it_did_not_become_healthy() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _reaper = Reaper(dir.path().to_path_buf());
    let honed = script(dir.path(), HANGING);

    let err = SidecarSupervisor::start(Some(honed), no_env(), Duration::from_millis(600))
        .err()
        .expect("a honed that never answers must fail")
        .to_string();
    assert!(err.contains("did not become healthy"), "got: {err}");
}

// ---- A3: a failed start leaves nothing running -----------------------------------

#[test]
fn test_a3_failed_start_should_kill_and_reap_the_child_before_returning() {
    let _s = serial();
    let dir = tempfile::tempdir().unwrap();
    let _reaper = Reaper(dir.path().to_path_buf());
    let honed = script(dir.path(), HANGING);

    let result = SidecarSupervisor::start(Some(honed), no_env(), Duration::from_millis(600));
    assert!(result.is_err());
    // Killed before `start` returned: no waiting is needed beyond a short poll.
    assert!(
        gone_within(dir.path(), Duration::from_millis(500)),
        "the honed that never became healthy is still running"
    );
}
