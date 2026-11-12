//! Graceful shutdown of the daemon: drain with tools working, then stop the
//! MCP servers gracefully, then everything still running.
//!
//! See `docs/plans/honed-graceful-shutdown-spec.md` (rows D1-D4).

use std::collections::HashMap;
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use axum::extract::connect_info::ConnectInfo;
use axum::serve::{IncomingStream, Listener};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tower::ServiceExt as _;

/// Default drain limit, in seconds.
const DEFAULT_GRACE_SECS: u64 = 25;

/// A worker pool to stop (see `WorkerPool::spawn_with_shutdown`).
pub struct WorkerShutdown {
    /// The pool's shutdown channel: `true` stops claiming.
    pub stop: watch::Sender<bool>,
    /// The pool's task.
    pub handle: JoinHandle<()>,
}

/// What [`serve_until_shutdown`] stops after the signal.
pub struct ShutdownOptions {
    /// The drain limit; zero means no drain.
    pub grace: Duration,
    pub worker: Option<WorkerShutdown>,
    /// MCP servers to shut down gracefully after the drain.
    pub mcp: Option<hone_mcp::McpManager>,
}

impl Default for ShutdownOptions {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(DEFAULT_GRACE_SECS),
            worker: None,
            mcp: None,
        }
    }
}

/// How the shutdown went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShutdownReport {
    /// `true`: everything finished by itself inside the limit.
    pub drained: bool,
}

/// Parse `HONE_SHUTDOWN_GRACE_SECS`: whole seconds >= 0. `None` or an invalid
/// value gives 25 s (the invalid case logs a warning).
pub fn parse_shutdown_grace(raw: Option<&str>) -> Duration {
    let default = Duration::from_secs(DEFAULT_GRACE_SECS);
    let Some(raw) = raw else {
        return default;
    };
    match raw.parse::<u64>() {
        Ok(secs) => Duration::from_secs(secs),
        Err(_) => {
            tracing::warn!(
                "HONE_SHUTDOWN_GRACE_SECS={raw:?} is not a whole number of seconds; using {DEFAULT_GRACE_SECS}"
            );
            default
        }
    }
}

// ---- connections that can be cut at the drain limit --------------------------

/// Open connections, so the drain limit can really close them: axum runs each
/// connection in its own task, which outlives the serve future.
#[derive(Default)]
struct Connections {
    next: u64,
    /// A duplicate handle of each open socket.
    open: HashMap<u64, std::net::TcpStream>,
}

type SharedConnections = Arc<Mutex<Connections>>;

fn lock(c: &SharedConnections) -> std::sync::MutexGuard<'_, Connections> {
    c.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Shut every open socket down (both directions): their tasks see the error
/// and drop the connection and the requests running on it.
fn cut_all(c: &SharedConnections) {
    for sock in lock(c).open.values() {
        let _ = sock.shutdown(std::net::Shutdown::Both);
    }
}

struct TrackedIo {
    inner: TcpStream,
    id: Option<u64>,
    conns: SharedConnections,
}

impl Drop for TrackedIo {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            lock(&self.conns).open.remove(&id);
        }
    }
}

impl AsyncRead for TrackedIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for TrackedIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}

struct TrackedListener {
    inner: TcpListener,
    conns: SharedConnections,
}

impl Listener for TrackedListener {
    type Io = TrackedIo;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        let (stream, addr) = Listener::accept(&mut self.inner).await;
        let id = register(&self.conns, &stream);
        (
            TrackedIo {
                inner: stream,
                id,
                conns: self.conns.clone(),
            },
            addr,
        )
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        self.inner.local_addr()
    }
}

#[cfg(unix)]
fn register(conns: &SharedConnections, stream: &TcpStream) -> Option<u64> {
    use std::os::fd::AsFd;
    let dup = stream.as_fd().try_clone_to_owned().ok()?;
    let mut c = lock(conns);
    let id = c.next;
    c.next += 1;
    c.open.insert(id, std::net::TcpStream::from(dup));
    Some(id)
}

#[cfg(not(unix))]
fn register(_conns: &SharedConnections, _stream: &TcpStream) -> Option<u64> {
    None
}

/// Abort the worker task and wait (at most a second) until it is really gone,
/// so nothing of it runs while the MCP servers shut down.
async fn abort_and_wait(w: &mut WorkerShutdown) {
    w.handle.abort();
    let _ = tokio::time::timeout(Duration::from_secs(1), &mut w.handle).await;
}

// ---- serve_until_shutdown ----------------------------------------------------

/// Serve `app` on `listener` until `shutdown` resolves. Then, in this order:
///
/// 1. tell the worker pool to stop claiming. Without a worker, stop accepting
///    connections too; with one, keep accepting until its task is done (a
///    co-located worker reports its result to this server), then stop;
/// 2. drain: wait for the worker task and open responses, in all at most
///    `options.grace` (zero: no drain). At the limit the worker task is
///    aborted (and awaited, briefly) and open connections are cut. No
///    process group is touched meanwhile, so tools keep working;
/// 3. shut the MCP servers down gracefully;
/// 4. stop every process group still registered
///    ([`hone_tools::kill_running_commands`]).
pub async fn serve_until_shutdown<F>(
    listener: TcpListener,
    app: Router,
    shutdown: F,
    options: ShutdownOptions,
) -> io::Result<ShutdownReport>
where
    F: Future<Output = ()> + Send + 'static,
{
    let ShutdownOptions {
        grace,
        mut worker,
        mcp,
    } = options;
    let conns = SharedConnections::default();
    let listener = TrackedListener {
        inner: listener,
        conns: conns.clone(),
    };
    let make_service = tower::service_fn(move |stream: IncomingStream<'_, TrackedListener>| {
        let addr = *stream.remote_addr();
        let app = app.clone();
        async move {
            Ok::<_, std::convert::Infallible>(app.map_request(
                move |mut req: axum::extract::Request| {
                    req.extensions_mut().insert(ConnectInfo(addr));
                    req
                },
            ))
        }
    });

    let (stop_serving, stopped) = tokio::sync::oneshot::channel::<()>();
    let mut stop_serving = Some(stop_serving);
    let mut serve = Box::pin(
        axum::serve(listener, make_service)
            .with_graceful_shutdown(async move {
                let _ = stopped.await;
            })
            .into_future(),
    );
    let mut shutdown = Box::pin(shutdown);

    // Serving. The serve future only ends early on an error.
    let early = tokio::select! {
        r = &mut serve => Some(r),
        () = &mut shutdown => None,
    };
    // One limit covers every phase below.
    let deadline = tokio::time::Instant::now() + grace;
    if let Some(w) = &worker {
        let _ = w.stop.send(true);
    }

    let mut result = Ok(ShutdownReport { drained: true });
    match early {
        Some(r) => {
            // The server ended by itself: stop the worker, within the limit.
            if let Err(e) = r {
                result = Err(e);
            }
            if let Some(w) = worker.as_mut() {
                let finished = !grace.is_zero()
                    && tokio::time::timeout_at(deadline, &mut w.handle)
                        .await
                        .is_ok();
                if !finished {
                    abort_and_wait(w).await;
                    if let Ok(r) = &mut result {
                        r.drained = false;
                    }
                }
            }
        }
        None => {
            let mut served: Option<io::Result<()>> = None;
            let drained = !grace.is_zero() && {
                let drain = async {
                    // Phase 1, only with a worker: it may need this server (a
                    // co-located coordinator) to report its result, so keep
                    // accepting until it is done.
                    if let Some(w) = worker.as_mut() {
                        tokio::select! {
                            _ = &mut w.handle => {}
                            r = &mut serve => {
                                served = Some(r);
                                let _ = (&mut w.handle).await;
                            }
                        }
                    }
                    // Phase 2: stop accepting, let open responses finish.
                    if served.is_none() {
                        if let Some(tx) = stop_serving.take() {
                            let _ = tx.send(());
                        }
                        served = Some((&mut serve).await);
                    }
                };
                tokio::time::timeout_at(deadline, drain).await.is_ok()
            };
            if let Some(Err(e)) = served {
                result = Err(e);
            }
            if !drained {
                tracing::warn!("shutdown: drain limit reached, dropping open work");
                if let Some(w) = worker.as_mut() {
                    abort_and_wait(w).await;
                }
                drop(serve);
                cut_all(&conns);
                if let Ok(r) = &mut result {
                    r.drained = false;
                }
            }
        }
    }

    if let Some(mcp) = mcp {
        mcp.shutdown().await;
    }
    // The kill waits out a SIGTERM grace in a sleep loop: keep it off the
    // async threads.
    let _ = tokio::task::spawn_blocking(hone_tools::kill_running_commands).await;
    result
}
