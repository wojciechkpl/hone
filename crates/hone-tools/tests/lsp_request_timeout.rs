//! RED-PHASE regression test for **MAJOR-5**: `StdioLspBridgeInner::request()`
//! has no timeout; a hung LSP server permanently stalls any agent turn that
//! invokes `hover()` or `code_actions()`.
//!
//! Audit finding (d):
//!     `crates/hone-tools/src/lsp/stdio_bridge.rs:218-242`
//!     `request()` enters `loop { read_message(...).await }` waiting for a
//!     matching response id.  If the server crashes, hangs, or only emits
//!     non-matching notifications, the future never resolves.
//!     `did_open_and_wait` (line 345) DOES use `tokio::time::timeout`, so
//!     `diagnostics()` is bounded; but `hover()` and `code_actions()` —
//!     and the `initialize()` handshake itself — are not.
//!
//! Contract this test pins:
//!     `request()` (and therefore `spawn()` / `initialize()`) MUST return
//!     an `Err` within a bounded time when the server never sends a
//!     matching response.  The exact timeout is implementer's choice; this
//!     test gives 60 seconds of headroom.
//!
//! Why testing through `spawn()` is sufficient:
//!     `request()` is private; it is reached from `initialize()` (called by
//!     `spawn()`), `hover()`, and `code_actions()`.  The simplest reachable
//!     surface is `spawn()` against a child that holds stdout open without
//!     ever writing — `read_message` blocks on `read_line` and the loop
//!     never advances.  This is exactly the bug condition.
//!
//! Caveats:
//!     - SLOW TEST.  When the bug is present, the outer 60 s timeout fires.
//!       Once the bug is fixed, runtime drops to whatever inner timeout the
//!       implementer chose (audit suggested 30 s; could reasonably be 5–10 s).
//!     - We use `sh -c "exec sleep 600"` as the "LSP server".  `exec` causes
//!       the shell to be replaced by `sleep`, which never reads stdin and
//!       never writes stdout — exactly the silent-server scenario `request()`
//!       must defend against.
//!
//! Implementer's freedom (green phase):
//!     - Wrap the loop in `tokio::time::timeout`, OR use `tokio::select!`
//!       with a per-iteration deadline that resets on activity from the
//!       channel.  The latter is preferable because a chatty-but-responsive
//!       server emitting many unrelated notifications must not trigger a
//!       spurious timeout.
//!
//! Test name pinning the contract: `request_returns_err_when_server_never_responds`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::{Duration, Instant};

use hone_tools::StdioLspBridge;

/// Outer wall-clock deadline.  Generous enough to survive a 30 s inner
/// timeout plus startup overhead, tight enough that a forever-hang fails the
/// test in finite time.
const OUTER_TIMEOUT: Duration = Duration::from_secs(60);

#[tokio::test(flavor = "multi_thread")]
async fn request_returns_err_when_server_never_responds() {
    let started = Instant::now();

    // `exec sleep 600` replaces the shell with `sleep`, leaving stdout open
    // (the parent's pipe end stays connected to the child's fd 1) but never
    // writing — so `read_line` in `read_message` blocks indefinitely.  This
    // is the exact failure mode `request()` must guard against.
    let outcome = tokio::time::timeout(
        OUTER_TIMEOUT,
        StdioLspBridge::spawn("sh", &["-c", "exec sleep 600"]),
    )
    .await;

    let elapsed = started.elapsed();

    match outcome {
        Err(_elapsed_outer) => {
            panic!(
                "AUDIT-MAJOR-5: StdioLspBridge::spawn did not return within {} s. \
                 The `request()` loop in stdio_bridge.rs has no timeout, so a \
                 hung LSP server stalls the agent indefinitely.  Wrap the loop \
                 in `tokio::time::timeout` (per-iteration with reset on \
                 activity is preferable).",
                OUTER_TIMEOUT.as_secs()
            );
        }
        Ok(Ok(_bridge)) => {
            panic!(
                "AUDIT-MAJOR-5: spawn succeeded against a never-responding \
                 child after {elapsed:?}.  Either the test fixture failed to \
                 simulate a hung server, or the fix accidentally bypassed the \
                 LSP handshake entirely.  Investigate."
            );
        }
        Ok(Err(_e)) => {
            // SUCCESS path (after the fix): spawn returned an error within
            // the outer deadline.  The error message itself is implementer's
            // choice — we only pin "must error within {OUTER_TIMEOUT}".
        }
    }
}
