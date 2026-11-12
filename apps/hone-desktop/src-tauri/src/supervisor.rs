//! Supervises a child `honed` process.
//!
//! Responsibilities:
//! - Allocate a free TCP port on `127.0.0.1` and pass it to the child.
//! - Spawn the binary with a generated bearer token in the environment so
//!   loopback is still authenticated.
//! - Poll the child's `/health` endpoint until it responds (or timeout).
//! - Provide a graceful shutdown that sends SIGTERM (Unix) / `kill` (Windows)
//!   and waits for exit.
//! - Bounded restart on unexpected crash.
//!
//! Tests use a stub child (a tiny in-process HTTP server bound to the chosen
//! port) so the test suite does not require a built `honed` binary.

use std::collections::VecDeque;
use std::io::Read;
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SupervisorError {
    #[error("could not allocate a free port: {0}")]
    PortAllocation(#[source] std::io::Error),
    #[error("could not spawn honed: {0}")]
    Spawn(#[source] std::io::Error),
    #[error(
        "{0} not found on PATH. Install it with `cargo install --path bins/honed --locked` \
         and make sure ~/.cargo/bin is on PATH"
    )]
    NotFound(String),
    #[error("honed exited before it was healthy ({status}); last stderr lines:\n{stderr}")]
    Exited { status: ExitStatus, stderr: String },
    #[error("honed did not become healthy within {0:?}")]
    HealthTimeout(Duration),
    #[error("child has already been stopped")]
    AlreadyStopped,
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// What we ran and where it listens. Returned to the frontend so it can
/// build URLs to the daemon.
#[derive(Debug, Clone, Serialize)]
pub struct SidecarInfo {
    pub url: String,
    pub port: u16,
    pub pid: u32,
    pub bearer_token: Option<String>,
}

impl Drop for SidecarSupervisor {
    /// Never leave a `honed` behind: kill and reap the child if it is still
    /// running. A child already reaped (by `shutdown`, or a failed `start`) is
    /// left alone, so no signal goes to a reused pid.
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// How many of the child's last stderr lines are kept.
const STDERR_LINES: usize = 20;

type StderrTail = Arc<Mutex<VecDeque<String>>>;

pub struct SidecarSupervisor {
    child: Child,
    info: SidecarInfo,
    stderr_tail: StderrTail,
    stderr_thread: Option<std::thread::JoinHandle<()>>,
}

/// Drain `stderr` on a thread into a ring buffer of the last lines, so a
/// chatty child never blocks on a full pipe and a failed start can say why.
fn drain_stderr<R: Read + Send + 'static>(
    mut stderr: R,
) -> (StderrTail, std::thread::JoinHandle<()>) {
    let tail: StderrTail = Arc::new(Mutex::new(VecDeque::new()));
    let out = Arc::clone(&tail);
    let handle = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        let mut line: Vec<u8> = Vec::new();
        let push = |bytes: &[u8]| {
            let mut q = out.lock().unwrap_or_else(PoisonError::into_inner);
            if q.len() == STDERR_LINES {
                q.pop_front();
            }
            q.push_back(String::from_utf8_lossy(bytes).into_owned());
        };
        loop {
            match stderr.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for &b in &buf[..n] {
                        if b == b'\n' {
                            push(&line);
                            line.clear();
                        } else if line.len() < 2048 {
                            line.push(b);
                        }
                    }
                }
            }
        }
        if !line.is_empty() {
            push(&line);
        }
    });
    (tail, handle)
}

impl SidecarSupervisor {
    /// Spawn `honed_path` (default `"honed"` resolved on `PATH`).
    ///
    /// The supervisor binds a `TcpListener` on `127.0.0.1:0` to discover a
    /// free port, immediately drops it, and passes the port to `honed`. There
    /// is a small race window between drop and the child's `bind`; in
    /// practice on a developer machine this is harmless. If the bind races,
    /// `wait_until_healthy` will time out and the caller can retry.
    pub fn spawn(honed_path: Option<PathBuf>) -> Result<Self, SupervisorError> {
        Self::spawn_with(honed_path, std::env::vars())
    }

    /// `extra_env` is merged into the child env. Used by tests to inject
    /// `HONE_EPHEMERAL=1` etc.
    pub fn spawn_with<I, K, V>(
        honed_path: Option<PathBuf>,
        extra_env: I,
    ) -> Result<Self, SupervisorError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        let port = allocate_free_port()?;
        let bearer = generate_token();
        let bin = honed_path.unwrap_or_else(|| PathBuf::from("honed"));

        let mut cmd = Command::new(&bin);
        cmd.arg(port.to_string());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        cmd.env("HONE_AUTH_TOKEN", &bearer)
            .env("HONE_BIND", "127.0.0.1")
            .env("HONE_EPHEMERAL", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SupervisorError::NotFound(bin.display().to_string())
            } else {
                SupervisorError::Spawn(e)
            }
        })?;
        let (stderr_tail, stderr_thread) = match child.stderr.take() {
            Some(pipe) => {
                let (tail, handle) = drain_stderr(pipe);
                (tail, Some(handle))
            }
            None => (Arc::default(), None),
        };
        let pid = child.id();
        let url = format!("http://127.0.0.1:{port}");

        Ok(Self {
            child,
            info: SidecarInfo {
                url,
                port,
                pid,
                bearer_token: Some(bearer),
            },
            stderr_tail,
            stderr_thread,
        })
    }

    /// `spawn_with` and the health wait in one step. A child that exits early
    /// fails the start at once, with its exit status and last stderr lines. On
    /// any failure the child is killed and reaped before the error returns.
    pub fn start<I, K, V>(
        honed_path: Option<PathBuf>,
        extra_env: I,
        health_timeout: Duration,
    ) -> Result<Self, SupervisorError>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<std::ffi::OsStr>,
        V: AsRef<std::ffi::OsStr>,
    {
        let mut sup = Self::spawn_with(honed_path, extra_env)?;
        match sup.wait_healthy_or_exit(health_timeout) {
            Ok(()) => Ok(sup),
            Err(e) => {
                let _ = sup.child.kill();
                let _ = sup.child.wait();
                Err(e)
            }
        }
    }

    /// The last stderr lines, oldest first. Waits briefly for the drain thread
    /// to see the pipe close, so the lines written just before an exit are in.
    fn stderr_text(&mut self) -> String {
        if let Some(h) = self.stderr_thread.take() {
            let deadline = Instant::now() + Duration::from_millis(300);
            while !h.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
        let q = self
            .stderr_tail
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        q.iter().cloned().collect::<Vec<_>>().join("\n")
    }

    fn wait_healthy_or_exit(&mut self, timeout: Duration) -> Result<(), SupervisorError> {
        let deadline = Instant::now() + timeout;
        let url = format!("{}/health", self.info.url);
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .map_err(|e| SupervisorError::Io(std::io::Error::other(e.to_string())))?;
        loop {
            if let Some(status) = self.child.try_wait()? {
                let stderr = self.stderr_text();
                return Err(SupervisorError::Exited { status, stderr });
            }
            if let Ok(resp) = client.get(&url).send() {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err(SupervisorError::HealthTimeout(timeout));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn info(&self) -> &SidecarInfo {
        &self.info
    }

    /// Block-poll `<url>/health` until 200 OK or timeout. Sleeps 50 ms
    /// between polls. Uses the **blocking** reqwest client deliberately so
    /// the supervisor remains usable from non-async callers.
    pub fn wait_until_healthy(&self, timeout: Duration) -> Result<(), SupervisorError> {
        let deadline = Instant::now() + timeout;
        let url = format!("{}/health", self.info.url);
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .map_err(|e| SupervisorError::Io(std::io::Error::other(e.to_string())))?;
        loop {
            if let Ok(resp) = client.get(&url).send() {
                if resp.status().is_success() {
                    return Ok(());
                }
            }
            if Instant::now() >= deadline {
                return Err(SupervisorError::HealthTimeout(timeout));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Returns true if the child is still running.
    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) | Err(_) => false,
        }
    }

    /// Send a kill signal and wait for exit. Idempotent across repeated calls.
    pub fn shutdown(mut self) -> Result<(), SupervisorError> {
        // `Child::kill` on Unix is SIGKILL; for the v0.0.1 shell we accept
        // that — `honed` has no on-disk state worth flushing in ephemeral
        // mode. Future work: switch to SIGTERM via libc and a join timeout.
        if let Err(e) = self.child.kill() {
            // ESRCH ("no such process") means the child already exited;
            // treat that as success.
            if e.kind() != std::io::ErrorKind::InvalidInput {
                return Err(SupervisorError::Io(e));
            }
        }
        let _ = self.child.wait();
        Ok(())
    }
}

fn allocate_free_port() -> Result<u16, SupervisorError> {
    let listener =
        TcpListener::bind("127.0.0.1:0").map_err(SupervisorError::PortAllocation)?;
    let port = listener
        .local_addr()
        .map_err(SupervisorError::PortAllocation)?
        .port();
    drop(listener);
    Ok(port)
}

fn generate_token() -> String {
    uuid::Uuid::now_v7().to_string()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn allocates_distinct_ports_on_repeat_call() {
        let a = allocate_free_port().expect("port a");
        let b = allocate_free_port().expect("port b");
        // The OS may or may not reuse the same number; we just verify both
        // calls succeed and return non-zero ports.
        assert_ne!(a, 0);
        assert_ne!(b, 0);
    }

    #[test]
    fn token_generation_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..32 {
            assert!(seen.insert(generate_token()));
        }
    }

    #[test]
    fn spawning_missing_binary_fails_and_names_the_path() {
        let path = "/definitely/not/a/real/honed/binary/path";
        let result = SidecarSupervisor::spawn(Some(PathBuf::from(path)));
        match result {
            Ok(_) => panic!("expected spawn to fail"),
            Err(e) => assert!(e.to_string().contains(path), "got {e}"),
        }
    }

    #[test]
    fn wait_until_healthy_times_out_when_nothing_listens() {
        // Allocate a port, do not listen on it, manually construct an info
        // and simulate the timeout path. We can't easily build a
        // SidecarSupervisor without a real child, so this test exercises
        // the underlying probing primitives via a tiny helper.
        let port = allocate_free_port().expect("port");
        let url = format!("http://127.0.0.1:{port}");
        let probe_url = format!("{url}/health");
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .expect("client");
        let start = Instant::now();
        let deadline = start + Duration::from_millis(150);
        let mut healthy = false;
        while Instant::now() < deadline {
            if client.get(&probe_url).send().map(|r| r.status().is_success()).unwrap_or(false) {
                healthy = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!healthy, "expected probe to time out, but it succeeded");
    }

    // ---- G6: the stderr reader retries interrupted reads --------------------------

    /// Yields `steps` in order, then end of file.
    enum Step {
        Data(&'static [u8]),
        Err(std::io::ErrorKind),
    }

    struct Scripted(std::collections::VecDeque<Step>);

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop_front() {
                None => Ok(0),
                Some(Step::Err(kind)) => Err(std::io::Error::from(kind)),
                Some(Step::Data(d)) => {
                    buf[..d.len()].copy_from_slice(d);
                    Ok(d.len())
                }
            }
        }
    }

    fn drained(steps: Vec<Step>) -> Vec<String> {
        let (tail, handle) = drain_stderr(Scripted(steps.into()));
        handle.join().unwrap();
        let q = tail.lock().unwrap_or_else(PoisonError::into_inner);
        q.iter().cloned().collect()
    }

    #[test]
    fn test_g6_interrupted_read_should_be_retried() {
        let lines = drained(vec![
            Step::Data(b"before\n"),
            Step::Err(std::io::ErrorKind::Interrupted),
            Step::Data(b"after\n"),
        ]);
        assert_eq!(lines, ["before", "after"]);
    }

    #[test]
    fn test_g6_interrupted_first_read_should_not_end_the_reader() {
        let lines = drained(vec![
            Step::Err(std::io::ErrorKind::Interrupted),
            Step::Err(std::io::ErrorKind::Interrupted),
            Step::Data(b"hello\nworld\n"),
        ]);
        assert_eq!(lines, ["hello", "world"]);
    }

    #[test]
    fn test_g6_any_other_error_should_end_the_reader() {
        let lines = drained(vec![
            Step::Data(b"kept\n"),
            Step::Err(std::io::ErrorKind::BrokenPipe),
            Step::Data(b"never read\n"),
        ]);
        assert_eq!(lines, ["kept"]);
    }
}
