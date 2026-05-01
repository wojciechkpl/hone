//! RED-PHASE regression test for **MAJOR-6**: `did_open_and_wait` calls
//! `std::fs::read_to_string` synchronously inside an `async fn` while
//! holding a `tokio::sync::Mutex` guard, blocking the executor thread.
//!
//! Audit finding (e):
//!     `crates/hone-tools/src/lsp/stdio_bridge.rs:308-309`
//!         let content = std::fs::read_to_string(file)
//!     This is reached from `<StdioLspBridge as LspBridge>::diagnostics`
//!     (line 427) while `self.inner.lock().await` is held — so the entire
//!     tokio worker thread blocks for the duration of the disk read.
//!
//! Contract this test pins:
//!     The body of `did_open_and_wait` MUST NOT call `std::fs::read_to_string`
//!     (or any other synchronous filesystem function from `std::fs`).
//!     Instead it must use `tokio::fs::read_to_string` and `.await` the
//!     result.
//!
//! Why a static-source test:
//!     A behavioural test ("verify the executor doesn't stall under load")
//!     is flaky and platform-dependent.  This contract is a coding rule —
//!     "don't call std::fs from async paths" — and a substring assertion on
//!     the function body pins it directly without false positives or
//!     timing dependencies.
//!
//! These tests will FAIL against current `main` because the literal
//! `std::fs::read_to_string` appears inside `did_open_and_wait`.
//!
//! Implementer's freedom (green phase):
//!     - Replace the line with `tokio::fs::read_to_string(file).await`.
//!     - Propagate the resulting `io::Error` via the existing
//!       `ToolError::Other` mapping.

#![allow(clippy::expect_used, clippy::unwrap_used)]

const STDIO_BRIDGE_SRC: &str = include_str!("../src/lsp/stdio_bridge.rs");

/// Slice the source between the start of `async fn did_open_and_wait` and
/// the line of the next outer item (`async fn` or `}` at column 0 inside
/// `impl`).  We use a heuristic that's robust to formatting tweaks: scan
/// up to ~3 KiB after the function header.
fn did_open_and_wait_body() -> &'static str {
    let start = STDIO_BRIDGE_SRC
        .find("async fn did_open_and_wait")
        .expect(
            "did_open_and_wait function not found — has the function been \
             renamed?  Update this test to track the new name.",
        );
    let after = &STDIO_BRIDGE_SRC[start..];
    // Bound the slice we inspect.  3 KiB is comfortably more than the
    // current function body (~50 lines, ~1.5 KiB) and well short of the
    // next function definition.
    let end = after.len().min(3 * 1024);
    &after[..end]
}

#[test]
fn did_open_and_wait_does_not_call_std_fs_read_to_string() {
    let body = did_open_and_wait_body();
    assert!(
        !body.contains("std::fs::read_to_string"),
        "AUDIT-MAJOR-6: did_open_and_wait must use tokio::fs::read_to_string \
         (async, non-blocking) instead of std::fs::read_to_string (blocking).\n\
         Holding a tokio::sync::Mutex guard across blocking I/O stalls the \
         executor thread for the duration of the disk read."
    );
}

#[test]
fn did_open_and_wait_does_not_call_other_blocking_std_fs_helpers() {
    // Defensive guard against the same anti-pattern reappearing in a
    // different form — `std::fs::read`, `std::fs::File::open`, etc.
    let body = did_open_and_wait_body();
    let banned = [
        "std::fs::read",  // covers read, read_to_string, read_dir
        "std::fs::File",  // covers File::open, File::create
        "std::fs::write", // also blocking
    ];
    let mut hits: Vec<&str> = Vec::new();
    for needle in banned {
        if body.contains(needle) {
            hits.push(needle);
        }
    }
    assert!(
        hits.is_empty(),
        "AUDIT-MAJOR-6: did_open_and_wait contains blocking std::fs calls: \
         {hits:?}.  Use the tokio::fs equivalents in async code."
    );
}

#[test]
fn did_open_and_wait_uses_tokio_fs_for_reads() {
    // Positive contract: the implementer must actually replace the call
    // with the async equivalent, not delete the read entirely.  We check
    // that *some* tokio::fs call exists in the function body.
    let body = did_open_and_wait_body();
    assert!(
        body.contains("tokio::fs::"),
        "AUDIT-MAJOR-6: did_open_and_wait must use tokio::fs:: to read the \
         opened file's contents — the function still needs the file body to \
         send in the textDocument/didOpen notification."
    );
}
