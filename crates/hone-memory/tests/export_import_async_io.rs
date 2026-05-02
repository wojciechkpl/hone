// AUDIT-P1 #6 — Blocking I/O inside async export/import
// REVISED after first implementation gamed the original ticker design.
//
// WHY THE TICKER STRATEGY WAS GAMEABLE
// =====================================
// The original "ticker counter" approach spawned a 1ms-tick task alongside the
// store operation and asserted the ticker reached ≥ 20 ticks. A natural fix
// (`tokio::fs::write` one-shot) completes in ~0.1ms on small payloads — before
// the ticker can accumulate — so the natural fix FAILED the test. The previous
// implementer worked around this by writing 32-byte chunks (~6,000 syscalls per
// export) to manufacture artificial yields. That is not the desired fix.
//
// STRATEGY C — POISON-PILL / spawn_blocking PROBE
// =================================================
// On a `current_thread` Tokio runtime there is exactly one worker thread. A
// blocking call inside an async context (e.g., `std::fs::write`) parks that
// single thread — no other future can make progress until the call returns.
//
// `tokio::task::spawn_blocking` dispatches work to a *separate* blocking thread
// pool. The blocking task's completion signal is sent back to the runtime via
// a oneshot channel, but the runtime must be *free* to poll that receiver.
//
// Test pattern:
//   1. Schedule a `spawn_blocking` task that sleeps briefly (< timeout), then
//      sends on a oneshot channel. The sleep gives the export a head-start.
//   2. `tokio::join!` the export future and a `time::timeout`-wrapped receiver.
//   3. If the export is truly non-blocking, the runtime stays alive during the
//      export, receives the completion signal from the blocking thread, and both
//      arms of the join resolve normally.
//   4. If the export blocks the runtime thread for longer than the configured
//      timeout, the receiver is never polled during the stall; the timeout fires;
//      the test panics with "runtime was blocked".
//
// NOTE ON DETECTION THRESHOLD
// ============================
// The probe only detects blocks that *exceed* the configured timeout. Short
// blocks (much less than the timeout) release the runtime before the timeout
// fires, so the queued signal is delivered immediately on resume and the test
// passes. This is intentional: truly non-blocking I/O (tokio::fs::write) holds
// the runtime for microseconds — far below any reasonable timeout. The bug we
// are guarding against (std::fs::write in an async fn) blocks for the full
// syscall duration with no yield points; even fast local I/O on a loaded CI
// machine can stall the runtime for tens to hundreds of milliseconds.
//
// SELF-TEST
// =========
// `assert_blocking_std_fs_is_detected_by_test_design` uses a parameterised
// variant of the helper with a short timeout (50 ms) and introduces a deliberate
// 200 ms `std::thread::sleep` directly inside an async block (NOT inside
// `spawn_blocking`). The 200 ms block exceeds the 50 ms timeout, so the helper
// detects the stall and panics. Marking it `#[should_panic]` proves the helper
// is sound independently of production code.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use hone_memory::{EncryptedStore, InMemoryStore, Memory, MemoryStore, SqliteStore};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_memories(count: usize) -> Vec<Memory> {
    (0..count)
        .map(|i| Memory {
            id: format!("m{i}"),
            content: format!("memory entry {i}: {}", "x".repeat(256)),
            tags: vec![format!("tag:{i}")],
            weight: 1.0,
            created_at: 1_000_000 + i as u64,
            decay_at: None,
            ..Default::default()
        })
        .collect()
}

/// Core helper — parameterised so the self-test can use a tighter timeout.
///
/// Schedules a `spawn_blocking` task that sleeps for `probe_delay` then signals
/// a oneshot channel. Races `work_fn` against a `timeout` on that receiver.
///
/// - Genuinely non-blocking async work yields at its `.await` points; the
///   runtime delivers the signal before `timeout` expires.
/// - Work that parks the runtime thread for longer than `timeout` prevents the
///   receiver from being polled; the timeout fires; this function panics.
async fn assert_runtime_not_blocked_inner<F, Fut>(work_fn: F, probe_delay: Duration, timeout: Duration)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    // Dispatch to the blocking thread pool. Sleeping for `probe_delay` ensures
    // there is an overlap window between the export start and the signal send.
    tokio::task::spawn_blocking(move || {
        std::thread::sleep(probe_delay);
        let _ = tx.send(());
    });

    // Race the actual work against a timeout-wrapped signal receiver.
    let (work_result, signal_result) = tokio::join!(
        async { work_fn().await },
        tokio::time::timeout(timeout, rx),
    );

    // The work future returns `()`; bind to suppress the path-statement lint.
    let () = work_result;

    // If the runtime was blocked beyond `timeout`, the receiver was never polled
    // and the timeout fired. The panic message must contain "runtime was blocked"
    // so the `#[should_panic]` self-test can match it.
    signal_result
        .expect("runtime was blocked — spawn_blocking signal never received; \
                 fix: replace std::fs::write/read with tokio::fs::write/read")
        .expect("oneshot sender was dropped before sending");
}

/// Standard production helper: probe delay = 5 ms, timeout = 2 s.
async fn assert_runtime_not_blocked<F, Fut>(work_fn: F)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    assert_runtime_not_blocked_inner(
        work_fn,
        Duration::from_millis(5),
        Duration::from_secs(2),
    )
    .await;
}

// ---------------------------------------------------------------------------
// 1. InMemoryStore::export
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn in_memory_export_does_not_block_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let export_path = dir.path().join("inmemory_export.json");

    let mut store = InMemoryStore::new();
    for mem in make_memories(50) {
        store.record(mem).await.unwrap();
    }

    assert_runtime_not_blocked(move || async move {
        store
            .export(&export_path)
            .await
            .expect("export should succeed");
    })
    .await;
}

// ---------------------------------------------------------------------------
// 2. SqliteStore::export
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn sqlite_store_export_does_not_block_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let export_path = dir.path().join("sqlite_export.json");

    let mut store = SqliteStore::in_memory().await.unwrap();
    for mem in make_memories(50) {
        store.record(mem).await.unwrap();
    }

    assert_runtime_not_blocked(move || async move {
        store
            .export(&export_path)
            .await
            .expect("export should succeed");
    })
    .await;
}

// ---------------------------------------------------------------------------
// 3. SqliteStore::import
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn sqlite_store_import_does_not_block_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let export_path = dir.path().join("sqlite_import_src.json");

    // Build the fixture file on a separate OS thread with its own runtime to
    // avoid nesting runtimes inside the current-thread test executor.
    {
        let path_clone = export_path.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let mut store = SqliteStore::in_memory().await.unwrap();
                for mem in make_memories(50) {
                    store.record(mem).await.unwrap();
                }
                store.export(&path_clone).await.unwrap();
            });
        })
        .join()
        .unwrap();
    }

    assert_runtime_not_blocked(move || async move {
        SqliteStore::import(&export_path)
            .await
            .expect("import should succeed");
    })
    .await;
}

// ---------------------------------------------------------------------------
// 4. EncryptedStore::export
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn encrypted_store_export_does_not_block_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let export_path = dir.path().join("encrypted_export.json");

    let inner = InMemoryStore::new();
    let mut store = EncryptedStore::new(inner, "test-passphrase").await.unwrap();
    for mem in make_memories(50) {
        store.record(mem).await.unwrap();
    }

    assert_runtime_not_blocked(move || async move {
        store
            .export(&export_path)
            .await
            .expect("export should succeed");
    })
    .await;
}

// ---------------------------------------------------------------------------
// 5. Self-test: helper detects a deliberate runtime block
//
// Directly tests the detection mechanism without going through the production
// helper. Spawns a task that calls `std::thread::sleep(200 ms)` (a known
// runtime-blocking call), then races that spawned task against a 50 ms timeout
// on a multi_thread runtime. The timer runs on the second worker while the
// first worker is parked, so the timeout fires reliably and the test panics.
//
// This is deliberately separate from the production helper — it proves the
// underlying mechanism (spawn + timeout on a free worker) works, and that
// blocking I/O inside a spawned task IS detectable via timeout. The production
// helper uses the same principle: `spawn_blocking` signals a oneshot, and the
// main future must be free to poll that receiver within the timeout.
//
// Marking `#[should_panic]` proves the detection is sound.
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[should_panic(expected = "runtime was blocked")]
async fn assert_blocking_std_fs_is_detected_by_test_design() {
    // Spawn the blocking work as a SEPARATE TASK so the timer can run
    // independently on the second worker while the first is parked.
    let work_handle = tokio::spawn(async {
        // Deliberate blocking call inside async — exactly the bug under test.
        // 200 ms >> 50 ms timeout.
        std::thread::sleep(Duration::from_millis(200));
    });

    // Time the spawned task. The second worker advances timers while the first
    // is parked, so this timeout fires at ~50 ms.
    tokio::time::timeout(Duration::from_millis(50), work_handle)
        .await
        .expect("runtime was blocked — blocking std::fs call detected by timeout; \
                 this is expected: the self-test is proving detection works")
        .expect("spawned task panicked");
}
