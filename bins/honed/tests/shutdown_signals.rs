#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! D1-D5 wiring of `docs/plans/honed-graceful-shutdown-spec.md` in the real
//! `honed` binary, in plain HTTP mode (no worker, `HONE_EPHEMERAL=1`, loopback,
//! a free port). The logic itself is tested at library level in
//! `crates/hone-server/tests/graceful_shutdown.rs`; here: the first signal
//! drains with the `HONE_SHUTDOWN_GRACE_SECS` limit and exits 0, a second signal
//! ends `honed` *by* the signal, and a signal that was ignored at start stays
//! ignored.
//!
//! A request is held open with the results stream (SSE) of a queued plan, which
//! lasts for as long as its socket stays open (the distributed store is enabled
//! for that; no worker claims the task). Every wait is bounded and every child is
//! killed on drop.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Resets SIGINT/SIGTERM/SIGHUP to their defaults (SIGHUP to "ignored" when
/// argv[1] is `ignore-hup`, as under `nohup`), then execs honed (same pid), so
/// the test does not depend on how `cargo test` was started.
const RESET_SIGNALS_AND_EXEC: &str = "import os, signal, sys\n\
    for s in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):\n\
    \x20   signal.signal(s, signal.SIG_DFL)\n\
    if sys.argv[1] == 'ignore-hup':\n\
    \x20   signal.signal(signal.SIGHUP, signal.SIG_IGN)\n\
    os.execv(sys.argv[2], sys.argv[2:])";

struct Honed {
    child: Child,
    port: u16,
    _tmp: tempfile::TempDir,
    out: std::path::PathBuf,
}

impl Drop for Honed {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn start(grace: Option<&str>, ignore_hup: bool) -> Honed {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("honed.log");
    let port = free_port();
    let log = std::fs::File::create(&out).unwrap();
    let mut cmd = Command::new("python3");
    cmd.arg("-c")
        .arg(RESET_SIGNALS_AND_EXEC)
        .arg(if ignore_hup { "ignore-hup" } else { "default" })
        .arg(env!("CARGO_BIN_EXE_honed"))
        .arg(port.to_string())
        .env_clear()
        .env("HOME", tmp.path())
        .env("PATH", "/usr/bin:/bin:/usr/local/bin")
        .env("HONE_EPHEMERAL", "1")
        .env("HONE_BIND", "127.0.0.1")
        .env("NO_COLOR", "1")
        .env("RUST_LOG", "info")
        .env("HONE_DISTRIBUTED_DB", tmp.path().join("dist.db"))
        .current_dir(tmp.path())
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log);
    if let Some(g) = grace {
        cmd.env("HONE_SHUTDOWN_GRACE_SECS", g);
    }
    let child = cmd.spawn().expect("spawn honed");
    let mut h = Honed {
        child,
        port,
        _tmp: tmp,
        out,
    };
    let deadline = Instant::now() + Duration::from_secs(20);
    while TcpStream::connect(("127.0.0.1", port)).is_err() {
        if let Some(s) = h.child.try_wait().unwrap() {
            panic!("honed exited early: {s:?}; log: {}", h.log());
        }
        assert!(Instant::now() < deadline, "honed never listened");
        std::thread::sleep(Duration::from_millis(50));
    }
    h
}

impl Honed {
    fn log(&self) -> String {
        std::fs::read_to_string(&self.out).unwrap_or_default()
    }

    fn signal(&self, name: &str) {
        let st = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.child.id().to_string())
            .status()
            .unwrap();
        assert!(st.success());
    }

    /// An open request that never completes: the results stream (SSE) of a
    /// plan with a queued task nobody claims. Returns once the response has
    /// started, with the socket still open.
    fn hold_request_open(&self) -> TcpStream {
        let task = r#"{"subtasks":[{"id":"s1","description":"d","agent":"a","context":"","depends_on":[],"acceptance_tests":[],"budget_usd":null,"scope":[],"requires_capabilities":[]}]}"#;
        let mut enq = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        enq.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(
            enq,
            "POST /distributed/plans/p1/enqueue HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\
             Content-Type: application/json\r\nContent-Length: {}\r\n\r\n{task}",
            task.len()
        )
        .unwrap();
        let mut reply = String::new();
        let _ = enq.read_to_string(&mut reply);
        assert!(reply.starts_with("HTTP/1.1 200"), "enqueue failed: {reply}");

        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        write!(
            s,
            "GET /distributed/plans/p1/events HTTP/1.1\r\nHost: x\r\nAccept: text/event-stream\r\n\r\n"
        )
        .unwrap();
        let mut got = Vec::new();
        let mut buf = [0u8; 512];
        while !got.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = s.read(&mut buf).expect("no response headers");
            assert!(n > 0, "closed before the response started");
            got.extend_from_slice(&buf[..n]);
        }
        assert!(
            String::from_utf8_lossy(&got).starts_with("HTTP/1.1 200"),
            "stream refused: {}",
            String::from_utf8_lossy(&got)
        );
        s
    }

    fn wait_exit(&mut self, limit: Duration) -> Option<ExitStatus> {
        let start = Instant::now();
        loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                return Some(s);
            }
            if start.elapsed() > limit {
                return None;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    fn still_running(&mut self) -> bool {
        self.child.try_wait().unwrap().is_none()
    }
}

#[test]
fn test_d4_first_sigterm_when_idle_should_log_stopped_and_exit_zero() {
    let mut h = start(None, false);
    h.signal("TERM");
    let st = h.wait_exit(Duration::from_secs(10)).expect("did not exit");
    assert_eq!(st.code(), Some(0), "log: {}", h.log());
    assert!(h.log().contains("honed stopped"), "log: {}", h.log());
}

#[test]
fn test_d3_open_request_should_be_dropped_when_the_grace_limit_passes() {
    let mut h = start(Some("1"), false);
    let _held = h.hold_request_open();
    h.signal("TERM");
    let st = h.wait_exit(Duration::from_secs(8)).unwrap_or_else(|| {
        panic!(
            "no exit 8 s after SIGTERM with a 1 s limit; log: {}",
            h.log()
        )
    });
    assert_eq!(
        st.code(),
        Some(0),
        "exits 0, not by a signal; log: {}",
        h.log()
    );
}

#[test]
fn test_d3_zero_grace_should_not_wait_for_open_requests() {
    let mut h = start(Some("0"), false);
    let _held = h.hold_request_open();
    h.signal("TERM");
    let st = h
        .wait_exit(Duration::from_secs(4))
        .unwrap_or_else(|| panic!("0 means no drain, but honed kept waiting; log: {}", h.log()));
    assert_eq!(st.code(), Some(0), "log: {}", h.log());
}

#[test]
fn test_d3_invalid_grace_value_should_warn_and_still_shut_down() {
    let mut h = start(Some("soon"), false);
    h.signal("TERM");
    let st = h.wait_exit(Duration::from_secs(10)).expect("did not exit");
    assert_eq!(st.code(), Some(0), "log: {}", h.log());
    assert!(
        h.log().contains("HONE_SHUTDOWN_GRACE_SECS"),
        "a warning names the variable; log: {}",
        h.log()
    );
}

#[test]
fn test_d5_second_sigterm_during_the_drain_should_end_honed_by_the_signal() {
    let mut h = start(None, false); // default limit 20 s: only the 2nd signal ends it
    let _held = h.hold_request_open();
    h.signal("TERM");
    std::thread::sleep(Duration::from_millis(700));
    assert!(h.still_running(), "the open request keeps the drain going");
    h.signal("TERM");
    let st = h
        .wait_exit(Duration::from_secs(5))
        .unwrap_or_else(|| panic!("second SIGTERM did not stop honed; log: {}", h.log()));
    assert_eq!(
        st.signal(),
        Some(15),
        "ends by the signal; {st:?}; log: {}",
        h.log()
    );
}

#[test]
fn test_d5_signal_ignored_at_start_should_stay_ignored_for_the_second_signal() {
    let mut h = start(None, true); // SIGHUP ignored, as under nohup
    let _held = h.hold_request_open();
    h.signal("TERM");
    std::thread::sleep(Duration::from_millis(500));
    h.signal("HUP");
    std::thread::sleep(Duration::from_millis(700));
    assert!(h.still_running(), "an ignored SIGHUP must not stop honed");
    h.signal("TERM");
    let st = h
        .wait_exit(Duration::from_secs(5))
        .unwrap_or_else(|| panic!("second SIGTERM did not stop honed; log: {}", h.log()));
    assert_eq!(st.signal(), Some(15), "{st:?}; log: {}", h.log());
}
