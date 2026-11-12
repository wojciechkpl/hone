#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! D1-D4 of `docs/plans/honed-graceful-shutdown-spec.md`, at library level:
//! shutdown signals come from channels, not from the OS.
//!
//! API this file requires (new; the file fails to compile until it exists):
//!
//! ```ignore
//! // crates/hone-server/src/worker_pool.rs
//! impl WorkerPool {
//!     /// Like `spawn`, but when the value becomes `true` (only an explicit
//!     /// `true`, not the sender being dropped) the pool stops claiming. A
//!     /// task already running keeps going and its result is posted with
//!     /// `complete`; then the loop ends. An idle pool ends at once.
//!     pub fn spawn_with_shutdown(self, shutdown: tokio::sync::watch::Receiver<bool>)
//!         -> tokio::task::JoinHandle<()>;
//! }
//!
//! // crates/hone-server/src/shutdown.rs, re-exported at the crate root
//! pub struct WorkerShutdown {
//!     pub stop: tokio::sync::watch::Sender<bool>,      // the pool's channel
//!     pub handle: tokio::task::JoinHandle<()>,         // from spawn_with_shutdown
//! }
//! pub struct ShutdownOptions {
//!     pub grace: std::time::Duration,                  // the drain limit (D3)
//!     pub worker: Option<WorkerShutdown>,
//!     pub mcp: Option<hone_mcp::McpManager>,
//! }
//! impl Default for ShutdownOptions { /* grace 20 s, no worker, no mcp */ }
//! pub struct ShutdownReport {
//!     /// true: everything finished by itself inside the limit; false: the
//!     /// limit passed (or was 0) and something was dropped.
//!     pub drained: bool,
//! }
//! /// Serve `app` on `listener` until `shutdown` resolves. Then, in this order:
//! ///  1. stop accepting connections; send `true` on `worker.stop`;
//! ///  2. drain: wait for open responses and for `worker.handle`, together at
//! ///     most `grace` (0 = no drain). On the limit, abort the worker task and
//! ///     drop open responses (`drained = false`). Tools keep working meanwhile
//! ///     (no process group is touched);
//! ///  3. `mcp.shutdown().await` (graceful);
//! ///  4. `hone_tools::kill_running_commands()`;
//! ///  5. return.
//! /// The router is served with `into_make_service_with_connect_info::<SocketAddr>()`.
//! pub async fn serve_until_shutdown<F>(
//!     listener: tokio::net::TcpListener,
//!     app: axum::Router,
//!     shutdown: F,
//!     options: ShutdownOptions,
//! ) -> std::io::Result<ShutdownReport>
//! where F: std::future::Future<Output = ()> + Send + 'static;
//!
//! /// `HONE_SHUTDOWN_GRACE_SECS` parsing: `None` or an invalid value (not a
//! /// whole number of seconds >= 0) gives 20 s (the invalid case logs a
//! /// warning); `"0"` is zero.
//! pub fn parse_shutdown_grace(raw: Option<&str>) -> std::time::Duration;
//! ```
//!
//! The registry of process groups is process-wide and step 4 kills what is in
//! it, so every test takes `SERIAL`.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use axum::Json;
use axum::Router;
use axum::extract::ConnectInfo;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::response::sse::{Event, Sse};
use axum::routing::{get, post};
use hone_core::queue_client::QueueClient;
use hone_core::task_plan::SubTask;
use hone_server::{
    ExecutionOutcome, ShutdownOptions, TaskExecutor, WorkerPool, WorkerShutdown,
    parse_shutdown_grace, serve_until_shutdown,
};
use httpmock::prelude::*;
use tokio::sync::{Notify, oneshot, watch};

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ---- helpers ---------------------------------------------------------------

fn ps_alive(pid: &str) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

/// SIGKILLs every pid file's process when dropped: a failing test leaks nothing.
struct PidReaper(Vec<PathBuf>);

impl Drop for PidReaper {
    fn drop(&mut self) {
        for f in &self.0 {
            if let Ok(pid) = std::fs::read_to_string(f) {
                let _ = Command::new("kill")
                    .args(["-9", pid.trim()])
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}

async fn wait_for_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(s) = std::fs::read_to_string(path) {
            if !s.trim().is_empty() {
                return s.trim().to_owned();
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {path:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Handshakes. On the end of its input it writes `<dir>/stdin-closed` and exits;
/// on SIGTERM it writes `<dir>/got-term` and exits. argv[1] = directory.
const MCP_SERVER: &str = r#"
import json, os, signal, sys
d = sys.argv[1]
def term(*a):
    open(os.path.join(d, "got-term"), "w").write("x")
    os._exit(0)
signal.signal(signal.SIGTERM, term)
open(os.path.join(d, "server.pid"), "w").write(str(os.getpid()))
while True:
    line = sys.stdin.readline()
    if not line:
        open(os.path.join(d, "stdin-closed"), "w").write("x")
        os._exit(0)
    req = json.loads(line)
    if "id" not in req:
        continue
    result = {"tools": []} if req.get("method") == "tools/list" else {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

async fn mcp_manager(dir: &Path) -> hone_mcp::McpManager {
    let script = dir.join("server.py");
    std::fs::write(&script, MCP_SERVER).unwrap();
    let config: hone_config::mcp::McpConfig = serde_json::from_value(serde_json::json!({
        "mcpServers": { "s": {
            "command": "python3",
            "args": [script.to_str().unwrap(), dir.to_str().unwrap()]
        } }
    }))
    .unwrap();
    let m = tokio::time::timeout(
        Duration::from_secs(10),
        hone_mcp::McpManager::from_config(&config),
    )
    .await
    .expect("mcp startup hung");
    assert_eq!(m.server_count(), 1);
    m
}

async fn bind() -> (tokio::net::TcpListener, String) {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    (l, base)
}

type Serving = tokio::task::JoinHandle<std::io::Result<hone_server::ShutdownReport>>;

fn serve(
    listener: tokio::net::TcpListener,
    app: Router,
    options: ShutdownOptions,
) -> (oneshot::Sender<()>, Serving) {
    let (tx, rx) = oneshot::channel::<()>();
    let h = tokio::spawn(serve_until_shutdown(
        listener,
        app,
        async move {
            let _ = rx.await;
        },
        options,
    ));
    (tx, h)
}

/// Await `h` for at most `limit`; abort and fail otherwise.
async fn finishes_within<T>(h: &mut tokio::task::JoinHandle<T>, limit: Duration) -> T {
    match tokio::time::timeout(limit, &mut *h).await {
        Ok(r) => r.expect("task panicked"),
        Err(_) => {
            h.abort();
            panic!("did not finish within {limit:?}");
        }
    }
}

fn sleep_command(secs: u32, then: &str) -> Command {
    let mut c = Command::new("sh");
    c.args(["-c", &format!("sleep {secs}; {then}")]);
    c
}

// ---- D1: the worker finishes the task it is running -------------------------

struct SlowExecutor {
    started: Arc<Notify>,
    delay: Duration,
    calls: Arc<AtomicUsize>,
    finished: Arc<AtomicUsize>,
}

impl SlowExecutor {
    fn new(delay: Duration) -> Arc<Self> {
        Arc::new(Self {
            started: Arc::new(Notify::new()),
            delay,
            calls: Arc::new(AtomicUsize::new(0)),
            finished: Arc::new(AtomicUsize::new(0)),
        })
    }
}

#[async_trait]
impl TaskExecutor for SlowExecutor {
    async fn execute(&self, _task: &SubTask) -> ExecutionOutcome {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.started.notify_one();
        tokio::time::sleep(self.delay).await;
        self.finished.fetch_add(1, Ordering::SeqCst);
        ExecutionOutcome::completed("slow result")
    }
}

fn claimed_task_json(task_id: &str) -> serde_json::Value {
    let expires = (chrono::Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
    serde_json::json!({
        "task_id": task_id,
        "plan_id": "plan-test",
        "subtask": {
            "id": "s1", "description": "do the thing", "agent": "rust-expert",
            "context": "", "depends_on": [], "acceptance_tests": [],
            "budget_usd": null, "scope": [], "requires_capabilities": []
        },
        "lease_expires_at": expires,
        "attempts": 1
    })
}

struct Coordinator {
    server: &'static MockServer,
    results: httpmock::Mock<'static>,
}

impl Coordinator {
    /// A fake coordinator: heartbeats get 204; every claim gets `claim` (a
    /// task) or 204 (nothing); results for `task-abc` get 204. Its `MockServer`
    /// is leaked so the mock handle can live in the struct (test process only).
    async fn start(claim: Option<serde_json::Value>) -> Self {
        let server: &'static MockServer = Box::leak(Box::new(MockServer::start_async().await));
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/heartbeat");
            then.status(204);
        });
        match claim {
            Some(body) => {
                server.mock(|when, then| {
                    when.method(POST)
                        .path("/distributed/workers/worker-test/claim");
                    then.status(200)
                        .header("Content-Type", "application/json")
                        .json_body(body);
                });
            }
            None => {
                server.mock(|when, then| {
                    when.method(POST)
                        .path("/distributed/workers/worker-test/claim");
                    then.status(204);
                });
            }
        }
        let results = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/results/task-abc");
            then.status(204);
        });
        Self { server, results }
    }

    fn results_hits(&self) -> usize {
        self.results.hits()
    }

    fn pool(&self, executor: Arc<dyn TaskExecutor>) -> WorkerPool {
        let client = Arc::new(QueueClient::new(self.server.base_url(), "test-token"));
        WorkerPool::new(client, "worker-test", vec![], executor)
            .with_poll_interval(Duration::from_millis(10))
            .with_heartbeat_interval(Duration::from_millis(50))
            .with_lease_secs(60)
    }
}

#[tokio::test]
async fn test_d1_idle_pool_should_end_at_once_when_told_to_stop() {
    let _s = SERIAL.lock().await;
    let coord = Coordinator::start(None).await;
    let (stop, rx) = watch::channel(false);
    let mut h = coord
        .pool(SlowExecutor::new(Duration::ZERO))
        .spawn_with_shutdown(rx);
    tokio::time::sleep(Duration::from_millis(150)).await;
    stop.send(true).unwrap();
    finishes_within(&mut h, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn test_d1_running_task_should_finish_and_be_reported_then_the_pool_should_end() {
    let _s = SERIAL.lock().await;
    // Every claim returns a task: a pool that kept claiming would run it again.
    let coord = Coordinator::start(Some(claimed_task_json("task-abc"))).await;
    let executor = SlowExecutor::new(Duration::from_millis(600));
    let (stop, rx) = watch::channel(false);
    let mut h = coord.pool(executor.clone()).spawn_with_shutdown(rx);

    tokio::time::timeout(Duration::from_secs(5), executor.started.notified())
        .await
        .expect("the task never started");
    stop.send(true).unwrap();
    finishes_within(&mut h, Duration::from_secs(5)).await;

    assert_eq!(
        executor.calls.load(Ordering::SeqCst),
        1,
        "no new claim after stop"
    );
    assert_eq!(
        executor.finished.load(Ordering::SeqCst),
        1,
        "the running task was not cut short"
    );
    assert_eq!(
        coord.results_hits(),
        1,
        "its result is reported with complete"
    );
}

// ---- D2: requests drain with their commands -----------------------------------

#[tokio::test]
async fn test_d2_request_running_a_command_at_the_signal_should_finish_with_its_command() {
    let _s = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("command-done");
    let marker2 = marker.clone();
    let app = Router::new().route(
        "/slow",
        get(move || {
            let marker = marker2.clone();
            async move {
                let cmd = sleep_command(1, &format!("touch '{}'", marker.display()));
                let _ = hone_tools::run_command(cmd, None).await;
                "done"
            }
        }),
    );
    let (listener, base) = bind().await;
    let (signal, mut serving) = serve(listener, app, ShutdownOptions::default());

    let req = tokio::spawn(async move { reqwest::get(format!("{base}/slow")).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    signal.send(()).unwrap();

    let report = finishes_within(&mut serving, Duration::from_secs(10))
        .await
        .unwrap();
    let resp = tokio::time::timeout(Duration::from_secs(5), req)
        .await
        .expect("request hung")
        .unwrap()
        .expect("the request was cut");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "done");
    assert!(marker.exists(), "the command was killed at the signal");
    assert!(report.drained);
}

#[tokio::test]
async fn test_d2_new_connections_should_be_refused_after_the_signal() {
    let _s = SERIAL.lock().await;
    let app = Router::new().route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(2)).await;
            "done"
        }),
    );
    let (listener, base) = bind().await;
    let addr = base.trim_start_matches("http://").to_owned();
    let (signal, mut serving) = serve(listener, app, ShutdownOptions::default());
    let req = tokio::spawn(async move { reqwest::get(format!("{base}/slow")).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    signal.send(()).unwrap();

    let deadline = Instant::now() + Duration::from_secs(1);
    let mut refused = false;
    while Instant::now() < deadline {
        if tokio::net::TcpStream::connect(&addr).await.is_err() {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    finishes_within(&mut serving, Duration::from_secs(10))
        .await
        .unwrap();
    let _ = req.await;
    assert!(refused, "the listener must close when the drain starts");
}

// ---- D3: the drain has a limit ---------------------------------------------

#[test]
fn test_d3_grace_parsing_should_default_to_20_and_accept_whole_seconds() {
    let secs = |raw: Option<&str>| parse_shutdown_grace(raw);
    assert_eq!(secs(None), Duration::from_secs(20));
    assert_eq!(secs(Some("30")), Duration::from_secs(30));
    assert_eq!(secs(Some("0")), Duration::ZERO);
    for bad in ["", "abc", "-1", "1.5", "5s", " "] {
        assert_eq!(secs(Some(bad)), Duration::from_secs(20), "value {bad:?}");
    }
}

#[test]
fn test_d10_default_options_should_use_a_20_second_grace() {
    assert_eq!(ShutdownOptions::default().grace, Duration::from_secs(20));
}

#[tokio::test]
async fn test_d3_a_response_that_never_ends_should_be_dropped_when_the_limit_passes() {
    let _s = SERIAL.lock().await;
    let app = Router::new().route(
        "/events",
        get(|| async { Sse::new(futures::stream::pending::<Result<Event, Infallible>>()) }),
    );
    let (listener, base) = bind().await;
    let (signal, mut serving) = serve(
        listener,
        app,
        ShutdownOptions {
            grace: Duration::from_millis(300),
            ..Default::default()
        },
    );
    // Keep the stream open: the response has started and never ends.
    let resp = reqwest::get(format!("{base}/events")).await.unwrap();
    assert_eq!(resp.status(), 200);
    signal.send(()).unwrap();

    let start = Instant::now();
    let report = finishes_within(&mut serving, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(!report.drained, "the limit passed");
    assert!(
        start.elapsed() >= Duration::from_millis(250),
        "waited the limit"
    );
    drop(resp);
}

#[tokio::test]
async fn test_d3_zero_grace_should_not_drain_at_all() {
    let _s = SERIAL.lock().await;
    let app = Router::new().route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_secs(30)).await;
            "done"
        }),
    );
    let (listener, base) = bind().await;
    let (signal, mut serving) = serve(
        listener,
        app,
        ShutdownOptions {
            grace: Duration::ZERO,
            ..Default::default()
        },
    );
    let req = tokio::spawn(async move { reqwest::get(format!("{base}/slow")).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    signal.send(()).unwrap();
    let report = finishes_within(&mut serving, Duration::from_secs(3))
        .await
        .unwrap();
    assert!(!report.drained);
    req.abort();
}

#[tokio::test]
async fn test_d3_worker_task_still_running_at_the_limit_should_be_dropped_unreported() {
    let _s = SERIAL.lock().await;
    let coord = Coordinator::start(Some(claimed_task_json("task-abc"))).await;
    let executor = SlowExecutor::new(Duration::from_secs(60));
    let (stop, rx) = watch::channel(false);
    let handle = coord.pool(executor.clone()).spawn_with_shutdown(rx);
    tokio::time::timeout(Duration::from_secs(5), executor.started.notified())
        .await
        .expect("the task never started");

    let (listener, _base) = bind().await;
    let abort = handle.abort_handle();
    let (signal, mut serving) = serve(
        listener,
        Router::new(),
        ShutdownOptions {
            grace: Duration::from_millis(400),
            worker: Some(WorkerShutdown { stop, handle }),
            mcp: None,
        },
    );
    signal.send(()).unwrap();
    let start = Instant::now();
    let report = match tokio::time::timeout(Duration::from_secs(5), &mut serving).await {
        Ok(r) => r.unwrap().unwrap(),
        Err(_) => {
            abort.abort();
            serving.abort();
            panic!("serve_until_shutdown did not return within 5 s");
        }
    };
    let took = start.elapsed();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let finished = abort.is_finished();
    abort.abort();
    assert!(!report.drained);
    assert!(took < Duration::from_secs(3), "took {took:?}");
    assert!(finished, "the worker task is dropped at the limit");
    assert_eq!(
        coord.results_hits(),
        0,
        "a dropped task is left to the lease reaper"
    );
    assert_eq!(executor.finished.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_d3_everything_finishing_inside_the_limit_should_report_drained() {
    let _s = SERIAL.lock().await;
    let coord = Coordinator::start(Some(claimed_task_json("task-abc"))).await;
    let executor = SlowExecutor::new(Duration::from_millis(300));
    let (stop, rx) = watch::channel(false);
    let handle = coord.pool(executor.clone()).spawn_with_shutdown(rx);
    tokio::time::timeout(Duration::from_secs(5), executor.started.notified())
        .await
        .expect("the task never started");
    let (listener, _base) = bind().await;
    let (signal, mut serving) = serve(
        listener,
        Router::new(),
        ShutdownOptions {
            grace: Duration::from_secs(10),
            worker: Some(WorkerShutdown { stop, handle }),
            mcp: None,
        },
    );
    signal.send(()).unwrap();
    let start = Instant::now();
    let report = finishes_within(&mut serving, Duration::from_secs(8))
        .await
        .unwrap();
    assert!(report.drained);
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "does not wait the whole limit"
    );
    assert_eq!(
        coord.results_hits(),
        1,
        "the finished task was reported (D1 + D3)"
    );
}

// ---- D4: MCP servers shut down gracefully, then leftovers are stopped ---------

#[tokio::test]
async fn test_d4_mcp_server_should_work_during_the_drain_then_exit_by_its_input_closing() {
    let _s = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _reaper = PidReaper(vec![dir.path().join("server.pid")]);
    let mcp = mcp_manager(dir.path()).await;
    let pid = std::fs::read_to_string(dir.path().join("server.pid"))
        .unwrap()
        .trim()
        .to_owned();

    // A request that is still running when the signal comes records whether
    // the MCP server was still alive when it finished.
    let alive_at_end = Arc::new(AtomicBool::new(false));
    let (a2, p2) = (alive_at_end.clone(), pid.clone());
    let app = Router::new().route(
        "/slow",
        get(move || {
            let (a, p) = (a2.clone(), p2.clone());
            async move {
                tokio::time::sleep(Duration::from_millis(800)).await;
                a.store(ps_alive(&p), Ordering::SeqCst);
                "done"
            }
        }),
    );
    let (listener, base) = bind().await;
    let (signal, mut serving) = serve(
        listener,
        app,
        ShutdownOptions {
            mcp: Some(mcp),
            ..Default::default()
        },
    );
    let req = tokio::spawn(async move { reqwest::get(format!("{base}/slow")).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    signal.send(()).unwrap();
    finishes_within(&mut serving, Duration::from_secs(15))
        .await
        .unwrap();
    let _ = req.await;

    assert!(
        alive_at_end.load(Ordering::SeqCst),
        "the MCP server was stopped before the drain finished"
    );
    assert!(
        dir.path().join("stdin-closed").exists(),
        "the graceful path (input closed) ran"
    );
    assert!(
        !dir.path().join("got-term").exists(),
        "no SIGTERM before the graceful shutdown"
    );
    assert!(!ps_alive(&pid), "the server is gone when serve returns");
}

#[tokio::test]
async fn test_d4_zero_grace_should_still_shut_mcp_servers_down_gracefully() {
    let _s = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _reaper = PidReaper(vec![dir.path().join("server.pid")]);
    let mcp = mcp_manager(dir.path()).await;
    let pid = std::fs::read_to_string(dir.path().join("server.pid"))
        .unwrap()
        .trim()
        .to_owned();
    let (listener, _base) = bind().await;
    let (signal, mut serving) = serve(
        listener,
        Router::new(),
        ShutdownOptions {
            grace: Duration::ZERO,
            mcp: Some(mcp),
            ..Default::default()
        },
    );
    signal.send(()).unwrap();
    finishes_within(&mut serving, Duration::from_secs(10))
        .await
        .unwrap();
    assert!(dir.path().join("stdin-closed").exists());
    assert!(!dir.path().join("got-term").exists());
    assert!(!ps_alive(&pid));
}

#[tokio::test]
async fn test_d4_command_left_running_should_be_stopped_before_serve_returns() {
    let _s = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let pidfile = dir.path().join("cmd.pid");
    let _reaper = PidReaper(vec![pidfile.clone()]);
    // A command no request waits for: nothing but the final kill ends it.
    let script = format!("echo $$ > '{}'; exec sleep 300", pidfile.display());
    let bg = tokio::spawn(async move {
        let mut c = Command::new("sh");
        c.args(["-c", &script]);
        let _ = hone_tools::run_command(c, None).await;
    });
    let pid = wait_for_file(&pidfile).await;

    let (listener, _base) = bind().await;
    let (signal, mut serving) = serve(listener, Router::new(), ShutdownOptions::default());
    signal.send(()).unwrap();
    finishes_within(&mut serving, Duration::from_secs(10))
        .await
        .unwrap();
    let gone = !ps_alive(&pid);
    bg.abort();
    assert!(gone, "kill_running_commands runs after the drain");
}

// ---- D7: a co-located worker can report its result ---------------------------

/// The coordinator routes a worker's `QueueClient` uses, served by the same
/// app that `serve_until_shutdown` serves: heartbeat, claim (one task, then
/// nothing) and `complete` (counted).
fn coordinator_app() -> (Router, Arc<AtomicUsize>) {
    let complete_hits = Arc::new(AtomicUsize::new(0));
    let hits = complete_hits.clone();
    let claimed = Arc::new(AtomicBool::new(false));
    let app = Router::new()
        .route(
            "/distributed/workers/worker-test/heartbeat",
            post(|| async { StatusCode::NO_CONTENT }),
        )
        .route(
            "/distributed/workers/worker-test/claim",
            post(move || {
                let claimed = claimed.clone();
                async move {
                    if claimed.swap(true, Ordering::SeqCst) {
                        StatusCode::NO_CONTENT.into_response()
                    } else {
                        Json(claimed_task_json("task-abc")).into_response()
                    }
                }
            }),
        )
        .route(
            "/distributed/workers/worker-test/results/task-abc",
            post(move || {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    StatusCode::NO_CONTENT
                }
            }),
        );
    (app, complete_hits)
}

fn pool_for(base: &str, executor: Arc<dyn TaskExecutor>) -> WorkerPool {
    let client = Arc::new(QueueClient::new(base.to_owned(), "test-token"));
    WorkerPool::new(client, "worker-test", vec![], executor)
        .with_poll_interval(Duration::from_millis(10))
        .with_heartbeat_interval(Duration::from_millis(50))
        .with_lease_secs(60)
}

#[tokio::test]
async fn test_d7_worker_with_this_server_as_coordinator_should_report_its_result_after_the_signal()
{
    let _s = SERIAL.lock().await;
    let (app, complete_hits) = coordinator_app();
    let (listener, base) = bind().await;
    let executor = SlowExecutor::new(Duration::from_millis(700));
    let (stop, rx) = watch::channel(false);
    let handle = pool_for(&base, executor.clone()).spawn_with_shutdown(rx);
    let (signal, mut serving) = serve(
        listener,
        app,
        ShutdownOptions {
            grace: Duration::from_secs(10),
            worker: Some(WorkerShutdown { stop, handle }),
            mcp: None,
        },
    );
    tokio::time::timeout(Duration::from_secs(5), executor.started.notified())
        .await
        .expect("the task never started");
    signal.send(()).unwrap();

    let report = finishes_within(&mut serving, Duration::from_secs(10))
        .await
        .unwrap();
    assert_eq!(
        complete_hits.load(Ordering::SeqCst),
        1,
        "the listener closed before the worker could post `complete`"
    );
    assert_eq!(executor.finished.load(Ordering::SeqCst), 1);
    assert!(report.drained);
}

#[tokio::test]
async fn test_d7_listener_should_close_once_the_worker_has_finished_while_requests_still_drain() {
    let _s = SERIAL.lock().await;
    let (app, complete_hits) = coordinator_app();
    let app = app.route(
        "/slow",
        get(|| async {
            tokio::time::sleep(Duration::from_millis(3500)).await;
            "done"
        }),
    );
    let (listener, base) = bind().await;
    let addr = base.trim_start_matches("http://").to_owned();
    let executor = SlowExecutor::new(Duration::from_millis(300));
    let (stop, rx) = watch::channel(false);
    let handle = pool_for(&base, executor.clone()).spawn_with_shutdown(rx);
    let (signal, mut serving) = serve(
        listener,
        app,
        ShutdownOptions {
            grace: Duration::from_secs(15),
            worker: Some(WorkerShutdown { stop, handle }),
            mcp: None,
        },
    );
    let slow_url = format!("{base}/slow");
    let req = tokio::spawn(async move { reqwest::get(slow_url).await });
    tokio::time::timeout(Duration::from_secs(5), executor.started.notified())
        .await
        .expect("the task never started");
    signal.send(()).unwrap();

    // The worker reports; then the listener stops accepting while /slow runs.
    let deadline = Instant::now() + Duration::from_secs(3);
    while complete_hits.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "complete never arrived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut refused = false;
    while Instant::now() < deadline {
        if tokio::net::TcpStream::connect(&addr).await.is_err() {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let report = finishes_within(&mut serving, Duration::from_secs(15))
        .await
        .unwrap();
    let resp = req.await.unwrap().expect("the running request was cut");
    assert!(refused, "the listener must close once the worker is done");
    assert_eq!(resp.status(), 200);
    assert!(report.drained);
}

#[tokio::test]
async fn test_d7_limit_should_be_shared_by_the_worker_phase_and_the_http_phase() {
    let _s = SERIAL.lock().await;
    let coord = Coordinator::start(Some(claimed_task_json("task-abc"))).await;
    let executor = SlowExecutor::new(Duration::from_secs(60));
    let (stop, rx) = watch::channel(false);
    let handle = coord.pool(executor.clone()).spawn_with_shutdown(rx);
    tokio::time::timeout(Duration::from_secs(5), executor.started.notified())
        .await
        .expect("the task never started");
    let app = Router::new().route(
        "/events",
        get(|| async { Sse::new(futures::stream::pending::<Result<Event, Infallible>>()) }),
    );
    let (listener, base) = bind().await;
    let (signal, mut serving) = serve(
        listener,
        app,
        ShutdownOptions {
            grace: Duration::from_secs(1),
            worker: Some(WorkerShutdown { stop, handle }),
            mcp: None,
        },
    );
    let resp = reqwest::get(format!("{base}/events")).await.unwrap();
    signal.send(()).unwrap();
    let start = Instant::now();
    let report = finishes_within(&mut serving, Duration::from_secs(6))
        .await
        .unwrap();
    let took = start.elapsed();
    drop(resp);
    assert!(!report.drained);
    assert!(
        took < Duration::from_millis(1800),
        "one 1 s limit covers both phases, took {took:?}"
    );
}

// ---- D8: the limit really cuts; handlers see the client address ---------------

#[tokio::test]
async fn test_d8_open_stream_should_end_for_the_client_soon_after_the_limit() {
    let _s = SERIAL.lock().await;
    let app = Router::new().route(
        "/events",
        get(|| async { Sse::new(futures::stream::pending::<Result<Event, Infallible>>()) }),
    );
    let (listener, base) = bind().await;
    let (signal, serving) = serve(
        listener,
        app,
        ShutdownOptions {
            grace: Duration::from_millis(500),
            ..Default::default()
        },
    );
    let mut resp = reqwest::get(format!("{base}/events")).await.unwrap();
    assert_eq!(resp.status(), 200);
    signal.send(()).unwrap();

    // The test runtime keeps running: only the cut can end the stream.
    let start = Instant::now();
    let ended = tokio::time::timeout(Duration::from_millis(2500), async {
        // Read until the stream ends (EOF or an error).
        while let Ok(Some(_)) = resp.chunk().await {}
    })
    .await;
    let took = start.elapsed();
    serving.abort();
    assert!(
        ended.is_ok(),
        "the stream was still open {took:?} after the signal (limit 500 ms)"
    );
}

#[tokio::test]
async fn test_d8_handlers_should_get_the_peer_address_as_connect_info() {
    let _s = SERIAL.lock().await;
    let app = Router::new().route(
        "/whoami",
        get(|ConnectInfo(addr): ConnectInfo<SocketAddr>| async move { addr.ip().to_string() }),
    );
    let (listener, base) = bind().await;
    let (signal, mut serving) = serve(listener, app, ShutdownOptions::default());
    let body = reqwest::get(format!("{base}/whoami"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    signal.send(()).unwrap();
    finishes_within(&mut serving, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(body, "127.0.0.1");
}

/// Its drop is slow, so "the aborted worker task was awaited before the MCP
/// shutdown" is observable without racing: the MCP server sees its input close
/// only after the slow drop has finished.
struct DropStamp(
    Arc<std::sync::Mutex<Option<std::time::SystemTime>>>,
    Duration,
);

impl Drop for DropStamp {
    fn drop(&mut self) {
        std::thread::sleep(self.1);
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(std::time::SystemTime::now());
    }
}

struct StuckExecutor {
    started: Arc<Notify>,
    dropped_at: Arc<std::sync::Mutex<Option<std::time::SystemTime>>>,
    drop_delay: Duration,
}

#[async_trait]
impl TaskExecutor for StuckExecutor {
    async fn execute(&self, _task: &SubTask) -> ExecutionOutcome {
        let _stamp = DropStamp(self.dropped_at.clone(), self.drop_delay);
        self.started.notify_one();
        tokio::time::sleep(Duration::from_secs(60)).await;
        ExecutionOutcome::completed("never")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_d8_aborted_worker_task_should_be_gone_before_the_mcp_shutdown_starts() {
    let _s = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let _reaper = PidReaper(vec![dir.path().join("server.pid")]);
    let mcp = mcp_manager(dir.path()).await;
    let coord = Coordinator::start(Some(claimed_task_json("task-abc"))).await;
    let started = Arc::new(Notify::new());
    let dropped_at = Arc::new(std::sync::Mutex::new(None));
    let executor = Arc::new(StuckExecutor {
        started: started.clone(),
        dropped_at: dropped_at.clone(),
        drop_delay: Duration::from_millis(600),
    });
    let (stop, rx) = watch::channel(false);
    let handle = coord.pool(executor).spawn_with_shutdown(rx);
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("the task never started");

    let (listener, _base) = bind().await;
    let (signal, mut serving) = serve(
        listener,
        Router::new(),
        ShutdownOptions {
            grace: Duration::from_millis(300),
            worker: Some(WorkerShutdown { stop, handle }),
            mcp: Some(mcp),
        },
    );
    signal.send(()).unwrap();
    finishes_within(&mut serving, Duration::from_secs(15))
        .await
        .unwrap();

    let dropped = *dropped_at
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dropped = dropped.expect("the worker task was not dropped by the time serve returned");
    let closed = std::fs::metadata(dir.path().join("stdin-closed"))
        .expect("the MCP server was not shut down gracefully")
        .modified()
        .unwrap();
    assert!(
        closed >= dropped,
        "the MCP shutdown started while the aborted worker task was still being dropped"
    );
}

// ---- D9: reaching the limit after the worker finished cannot crash ------------

fn events_app() -> Router {
    Router::new().route(
        "/events",
        get(|| async { Sse::new(futures::stream::pending::<Result<Event, Infallible>>()) }),
    )
}

/// Reads `resp` until the stream ends or fails; returns when that happened.
fn watch_stream_end(mut resp: reqwest::Response) -> tokio::task::JoinHandle<Instant> {
    tokio::spawn(async move {
        loop {
            match resp.chunk().await {
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => return Instant::now(),
            }
        }
    })
}

/// The worker is done by the time the HTTP phase reaches the limit: the old
/// code polled the finished `JoinHandle` again and tokio panicked.
async fn limit_after_finished_worker(coord: Coordinator, executor: Arc<dyn TaskExecutor>) {
    let (stop, rx) = watch::channel(false);
    let handle = coord.pool(executor).spawn_with_shutdown(rx);
    tokio::time::sleep(Duration::from_millis(300)).await; // idle, or the task has run
    let (listener, base) = bind().await;
    let grace = Duration::from_millis(600);
    let (signal, mut serving) = serve(
        listener,
        events_app(),
        ShutdownOptions {
            grace,
            worker: Some(WorkerShutdown { stop, handle }),
            mcp: None,
        },
    );
    let resp = reqwest::get(format!("{base}/events")).await.unwrap();
    assert_eq!(resp.status(), 200);
    let ended = watch_stream_end(resp);
    signal.send(()).unwrap();
    let t0 = Instant::now();

    let report = finishes_within(&mut serving, Duration::from_secs(6))
        .await
        .expect("serve_until_shutdown returned an error");
    assert!(!report.drained, "the open stream hit the limit");
    let ended_at = tokio::time::timeout(Duration::from_secs(2), ended)
        .await
        .expect("the stream was never cut")
        .unwrap();
    let after = ended_at.duration_since(t0);
    assert!(
        after >= Duration::from_millis(450) && after < grace + Duration::from_millis(1000),
        "the stream ended {after:?} after the signal (limit {grace:?})"
    );
}

#[tokio::test]
async fn test_d9_limit_reached_after_an_idle_worker_ended_should_not_crash_and_should_cut() {
    let _s = SERIAL.lock().await;
    let coord = Coordinator::start(None).await;
    limit_after_finished_worker(coord, SlowExecutor::new(Duration::ZERO)).await;
}

#[tokio::test]
async fn test_d9_limit_reached_after_a_quick_task_finished_should_not_crash_and_should_cut() {
    let _s = SERIAL.lock().await;
    let coord = Coordinator::start(Some(claimed_task_json("task-abc"))).await;
    limit_after_finished_worker(coord, SlowExecutor::new(Duration::from_millis(50))).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_d9_connections_should_be_cut_at_the_limit_not_after_the_aborted_workers_slow_drop() {
    let _s = SERIAL.lock().await;
    let coord = Coordinator::start(Some(claimed_task_json("task-abc"))).await;
    let started = Arc::new(Notify::new());
    let executor = Arc::new(StuckExecutor {
        started: started.clone(),
        dropped_at: Arc::new(std::sync::Mutex::new(None)),
        drop_delay: Duration::from_millis(900),
    });
    let (stop, rx) = watch::channel(false);
    let handle = coord.pool(executor).spawn_with_shutdown(rx);
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .expect("the task never started");
    let (listener, base) = bind().await;
    let grace = Duration::from_millis(300);
    let (signal, mut serving) = serve(
        listener,
        events_app(),
        ShutdownOptions {
            grace,
            worker: Some(WorkerShutdown { stop, handle }),
            mcp: None,
        },
    );
    let resp = reqwest::get(format!("{base}/events")).await.unwrap();
    let ended = watch_stream_end(resp);
    signal.send(()).unwrap();
    let t0 = Instant::now();

    let ended_at = tokio::time::timeout(Duration::from_secs(6), ended)
        .await
        .expect("the stream was never cut")
        .unwrap();
    let after = ended_at.duration_since(t0);
    finishes_within(&mut serving, Duration::from_secs(8))
        .await
        .unwrap();
    // Cut at the limit (300 ms); waiting for the 900 ms drop first would end it
    // at about 1.2 s.
    assert!(
        after < Duration::from_millis(800),
        "the stream ended {after:?} after the signal: the cut waited for the worker"
    );
}
