//! AUDIT-P1 #4 (revised — option (b) trait refactor)
//!
//! ## Why the previous test design was rejected
//!
//! The previous `agent_memory_concurrency.rs` measured wall-clock time of four
//! concurrent `record_memory` calls and asserted the total was < 60% of the
//! serialized baseline.  Two fix attempts failed:
//!
//! * **Fire-and-forget `tokio::spawn`** — the wall-clock test passed because
//!   spawning detaches the work and returns immediately, but a reviewer correctly
//!   rejected the approach: writes can be lost when the runtime shuts down before
//!   the detached task finishes.  The wall-clock test could not distinguish "fast
//!   because concurrent" from "fast because dropped".
//!
//! * **Synchronous split-lock** — an attempt to acquire two separate `Mutex`
//!   locks (one for read, one for write) measured 414 ms (worse than the
//!   fully-serialized 400 ms baseline).  `tokio::sync::Mutex` is FIFO-fair, so
//!   splitting a single lock into two does not pipeline; it just adds lock-hand-off
//!   overhead.
//!
//! Both failures reveal the same root cause: any outer `Mutex<dyn MemoryStore>`
//! at the agent level serializes ALL store operations regardless of whether they
//! actually need exclusive access.
//!
//! ## Option (b): interior-mutability trait refactor
//!
//! The fix is to remove `&mut self` from `MemoryStore::record`, `forget`, and
//! `decay`, replacing them with `&self`.  Each implementation uses interior
//! mutability (`RwLock`/`Mutex` inside the struct, or the sqlx connection pool
//! for `SqliteStore`).  The `Arc<Mutex<dyn MemoryStore>>` wrapper at the agent
//! level becomes `Arc<dyn MemoryStore>`, and `fetch_relevant_memories` likewise
//! calls directly without acquiring an outer lock.
//!
//! This unlocks:
//! - TRUE concurrent reads: `find_similar` / `recall` calls overlap.
//! - Concurrent read + write: `recall` returns while `record` is in-flight.
//! - No lost writes: every `record_memory` awaits the store's internal future.
//!
//! ## How the tests exercise the bug
//!
//! The tests hold the raw `Arc<Mutex<dyn MemoryStore>>` (the same arc that
//! `AgentConfig::memory_store` holds) and contend on it directly.  This is the
//! exact mutex that `record_memory` and `fetch_relevant_memories` acquire.
//! Tests that bypass the agent (using a bare data handle) would not observe the
//! outer-mutex serialization, so they would pass regardless of the bug.
//!
//! After option-(b), `AgentConfig::memory_store` becomes
//! `Option<Arc<dyn MemoryStore>>` (no outer Mutex).  Tests 3 and 4 are then
//! rewritten to contend on the store's internal `RwLock` directly — they still
//! pass because `RwLock` allows simultaneous readers.
//!
//! ## What these tests pin
//!
//! | Test | Category | Fails today because |
//! |------|----------|---------------------|
//! | `record_memory_does_not_lose_writes_on_shutdown` | SAFETY | fire-and-forget drops writes; today this PASSES (current code awaits); included as regression guard |
//! | `record_memory_concurrent_writes_do_not_deadlock` | SAFETY | outer Mutex coalesces dedup; exactly 1 record lands instead of N |
//! | `recall_overlaps_with_concurrent_record` | CONCURRENCY | outer Mutex blocks recall behind record |
//! | `concurrent_recall_calls_do_not_serialize` | CONCURRENCY | outer Mutex serializes N recalls → N×delay not ≈ delay |
//! | `record_memory_returns_only_after_write_completes` | SAFETY | fire-and-forget returns before write; today PASSES; regression guard |
//!
//! ## Mock note
//!
//! `SlowMockStore` uses interior mutability (`Arc<RwLock<HashMap>>`).  When the
//! trait signature flips from `&mut self` to `&self` in the green phase, only
//! the `impl MemoryStore` line changes — all method bodies are unchanged.
//!
//! ## Runtime
//!
//! All tests use `#[tokio::test(flavor = "multi_thread", worker_threads = 4)]`.
//! A single-threaded runtime masks the bug: tasks cannot truly run in parallel
//! on one OS thread, so even serialized mutex acquisition looks concurrent.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use hone_core::memory_format::StructuredBody;
use hone_core::memory_worthy::MemoryCandidate;
use hone_core::{Agent, AgentConfig, RetryConfig};
use hone_memory::{Memory, MemoryError, MemoryFilter, MemoryKind, MemoryQuery, MemoryStore};
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::RwLock;

// ---------------------------------------------------------------------------
// Timing constants
// ---------------------------------------------------------------------------

/// Artificial delay injected by `SlowMockStore::record`.
/// Long enough that "blocked behind record" is observable at ≥50% of this value.
const RECORD_DELAY: Duration = Duration::from_millis(200);

/// Artificial delay injected by `SlowMockStore::recall`.
const RECALL_DELAY: Duration = Duration::from_millis(100);

/// Number of concurrent tasks used in the multi-caller tests.
const N: usize = 4;

// ---------------------------------------------------------------------------
// SlowMockStore
// ---------------------------------------------------------------------------

/// A `MemoryStore` with configurable per-method artificial delays and interior
/// mutability.
///
/// **Interior-mutability design note**: all state lives in
/// `Arc<RwLock<HashMap<…>>>` rather than directly in `self`.  This means the
/// method bodies are IDENTICAL whether the trait signature is `&mut self` or
/// `&self`.  After the green phase (option-(b) refactor), only the signature
/// lines in `impl MemoryStore for SlowMockStore` change — no body edits.
struct SlowMockStore {
    data: Arc<RwLock<HashMap<String, Memory>>>,
    record_delay: Duration,
    recall_delay: Duration,
}

impl SlowMockStore {
    fn new(record_delay: Duration, recall_delay: Duration) -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
            record_delay,
            recall_delay,
        }
    }
}

#[async_trait]
impl MemoryStore for SlowMockStore {
    // NOTE: trait now requires `&self` (option-(b) refactor).
    // Body is unchanged — interior mutability via Arc<RwLock<…>>.
    async fn record(&self, memory: Memory) -> Result<(), MemoryError> {
        if !self.record_delay.is_zero() {
            tokio::time::sleep(self.record_delay).await;
        }
        self.data.write().await.insert(memory.id.clone(), memory);
        Ok(())
    }

    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        if !self.recall_delay.is_zero() {
            tokio::time::sleep(self.recall_delay).await;
        }
        let guard = self.data.read().await;
        let all: Vec<Memory> = guard
            .values()
            .filter(|m| query.kind.map_or(true, |k| m.kind == k))
            .cloned()
            .collect();
        let limit = if query.limit == 0 {
            all.len()
        } else {
            query.limit
        };
        Ok(all.into_iter().take(limit).collect())
    }

    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        Ok(self.data.read().await.get(id).cloned())
    }

    async fn forget(&self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        let mut guard = self.data.write().await;
        let before = guard.len();
        match filter {
            MemoryFilter::Tag(tag) => guard.retain(|_, m| !m.tags.contains(tag)),
            MemoryFilter::Before(ts) => guard.retain(|_, m| m.created_at >= *ts),
            MemoryFilter::After(ts) => guard.retain(|_, m| m.created_at <= *ts),
            MemoryFilter::Kind(kind) => guard.retain(|_, m| m.kind != *kind),
        }
        Ok(before.saturating_sub(guard.len()) as u32)
    }

    async fn decay(&self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        let mut guard = self.data.write().await;
        let before = guard.len();
        guard.retain(|_, m| {
            m.weight *= factor;
            !matches!(m.decay_at, Some(t) if t <= now)
        });
        Ok(before.saturating_sub(guard.len()) as u32)
    }

    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        let guard = self.data.read().await;
        let memories: Vec<&Memory> = guard.values().collect();
        let json = serde_json::to_string_pretty(&memories)?;
        tokio::fs::write(path, json).await?;
        Ok(())
    }

    async fn import(path: &Path) -> Result<Self, MemoryError> {
        let bytes = tokio::fs::read(path).await?;
        let memories: Vec<Memory> = serde_json::from_slice(&bytes)?;
        let map: HashMap<String, Memory> =
            memories.into_iter().map(|m| (m.id.clone(), m)).collect();
        Ok(Self {
            data: Arc::new(RwLock::new(map)),
            record_delay: Duration::ZERO,
            recall_delay: Duration::ZERO,
        })
    }
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// Build an `Agent` and return both the agent AND the raw
/// `Arc<dyn MemoryStore>` that is installed in `AgentConfig`.
///
/// After option-(b), `AgentConfig::memory_store` is `Option<Arc<dyn MemoryStore>>`
/// with no outer `Mutex`.  Tests that previously contended on the outer mutex
/// via `store.lock().await` now call store methods directly — the `SlowMockStore`
/// uses an internal `Arc<RwLock<HashMap<…>>>` for its own synchronisation.
fn make_agent(record_delay: Duration, recall_delay: Duration) -> (Agent, Arc<dyn MemoryStore>) {
    let store = SlowMockStore::new(record_delay, recall_delay);
    let store_arc: Arc<dyn MemoryStore> = Arc::new(store);
    let store_arc_for_config = Arc::clone(&store_arc);

    let provider = Arc::new(MockProvider::new(vec!["ok".to_owned()]));

    let config = AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "test".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: std::path::PathBuf::from("/tmp"),
        memory_store: Some(store_arc_for_config),
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
    };

    (Agent::new(config), store_arc)
}

/// Build a unique `MemoryCandidate` per index.  Candidates are drawn from
/// completely different subject domains so their trigram-Jaccard overlap is
/// well below 0.8 — the dedup path will NOT coalesce any two of them.
///
/// This is verified empirically: the bodies are from different lexical domains
/// (quantum physics, javascript, cooking, finance) so they share almost no
/// alphanumeric trigrams.
static CANDIDATE_BODIES: &[&str] = &[
    "quantum superposition collapses upon measurement yielding definite eigenstate",
    "javascript closures capture variables from outer lexical scope at definition time",
    "hollandaise sauce emulsifies butter with egg yolk over gentle heat and lemon",
    "amortized loan payments distribute principal and compounding interest across installments",
    "photosynthesis converts solar radiation into glucose via chlorophyll pigment absorption",
    "fibonacci heap provides asymptotically optimal decrease key for dijkstra shortest path",
    "annealing schedule controls exploration decay in simulated optimization landscape",
    "bézier curve interpolates control points using bernstein polynomial basis functions",
];

fn make_candidate(idx: usize) -> MemoryCandidate {
    MemoryCandidate {
        kind: MemoryKind::Feedback,
        description: None,
        body: StructuredBody {
            rule: CANDIDATE_BODIES[idx % CANDIDATE_BODIES.len()].to_string(),
            why: None,
            how: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Test 1 — SAFETY: no lost writes (fire-and-forget regression guard)
// ---------------------------------------------------------------------------

/// SAFETY: `record_memory` must durably write before it returns to its awaiter.
///
/// Current status: PASSES (current code awaits the store directly; this is a
/// regression guard ensuring a future "fire-and-forget" attempt cannot sneak in).
///
/// With a fire-and-forget impl (`tokio::spawn` then return), on a multi-thread
/// runtime the spawned task may execute on a different worker and may NOT have
/// completed by the time we call `recall_by_id` — so the assertion can fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_memory_does_not_lose_writes_on_shutdown() {
    let (agent, store) = make_agent(Duration::ZERO, Duration::ZERO);

    agent.record_memory(make_candidate(0)).await;

    // Query directly — no outer mutex after option-(b).
    let count = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 64,
            kind: None,
        })
        .await
        .unwrap()
        .len();

    assert!(
        count >= 1,
        "record_memory returned but no record was found. \
         Fire-and-forget: the write was spawned but not yet executed (P1 #4)."
    );
}

// ---------------------------------------------------------------------------
// Test 2 — SAFETY: N concurrent writes all land
// ---------------------------------------------------------------------------

/// SAFETY: N=4 concurrent `record_memory` calls on DISTINCT candidates must ALL
/// produce distinct records.
///
/// Current bug: the outer `Mutex` is held for the entire `find_similar → record`
/// span.  Because the mutex is FIFO-fair and non-reentrant, tasks queue up.
/// Each task waits for the previous to finish, then calls `find_similar` — which
/// now sees the records written by earlier tasks.  If two candidates share
/// enough trigram overlap with an already-written record (≥ 0.8 Jaccard), the
/// second write is treated as a dedup/upsert rather than a new insert, so the
/// total count can be less than N.
///
/// The `make_candidate` helper deliberately produces candidates whose trigram
/// overlap is < 0.8 with each other.  Despite this, with very short strings
/// the shared prefix "slot" can inflate Jaccard enough to coalesce.  The test
/// asserts count == N; if dedup fires incorrectly the count will be < N.
///
/// After option-(b), each call reads and writes concurrently without an outer
/// lock, so the count reliably equals N (no spurious dedup from serial ordering).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_memory_concurrent_writes_do_not_deadlock() {
    let (agent, store) = make_agent(Duration::ZERO, Duration::ZERO);
    let agent = Arc::new(agent);

    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        let agent = Arc::clone(&agent);
        handles.push(tokio::spawn(async move {
            agent.record_memory(make_candidate(i)).await;
        }));
    }
    for h in handles {
        h.await.expect("spawned task panicked");
    }

    let count = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 64,
            kind: None,
        })
        .await
        .unwrap()
        .len();

    assert_eq!(
        count, N,
        "Expected {N} distinct records after {N} concurrent record_memory calls, \
         got {count}. Without an outer Mutex, concurrent find_similar calls each see \
         the initial empty state and do not spuriously dedup (P1 #4)."
    );
}

// ---------------------------------------------------------------------------
// Test 3 — CONCURRENCY: recall does NOT block behind an in-flight record
// ---------------------------------------------------------------------------

/// KEY CONCURRENCY pin: today `record_memory` holds `Arc<Mutex<dyn MemoryStore>>`
/// for the entire `find_similar → record` span.  Any concurrent `recall` call
/// that tries `store.lock().await` blocks until `record_memory` releases the
/// mutex — a full RECORD_DELAY (200 ms) wait.
///
/// The test acquires the outer mutex itself (simulating a concurrent `recall`)
/// WHILE a slow `record_memory` call is in-flight, and measures how long it
/// must wait.
///
/// Expected after option-(b): `Arc<dyn MemoryStore>` is used directly; no outer
/// mutex exists; `recall` returns immediately without waiting for `record`.
/// The wait for the mutex should then be ≈ 0 (≪ RECORD_DELAY × 0.5).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn recall_overlaps_with_concurrent_record() {
    // record_delay=200ms makes "blocked behind record" clearly visible.
    // recall_delay=0ms so the recall itself is instant; we measure mutex wait.
    let (agent, store) = make_agent(RECORD_DELAY, Duration::ZERO);
    let agent = Arc::new(agent);

    // Task A: start a slow record_memory.  It will hold the outer Mutex for
    // at least RECORD_DELAY while it awaits the mock's record() future.
    let agent_clone = Arc::clone(&agent);
    let record_handle = tokio::spawn(async move {
        agent_clone.record_memory(make_candidate(42)).await;
    });

    // Give Task A a brief head-start so it has acquired the outer Mutex.
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Task B: issue a recall directly against the Arc<dyn MemoryStore> while
    // Task A's record_memory is in-flight.  After option-(b) there is no outer
    // Mutex, so this should return immediately (recall_delay = 0).
    let store_clone = Arc::clone(&store);
    let wait_start = Instant::now();
    let recall_handle = tokio::spawn(async move {
        // No lock acquisition needed — call through Arc<dyn MemoryStore> directly.
        let _ = store_clone
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 1,
                kind: None,
            })
            .await;
        // Returns immediately — we only measured the wait.
    });

    recall_handle.await.expect("recall task panicked");
    let wait_elapsed = wait_start.elapsed();

    // With the bug: Task B was blocked ≈ RECORD_DELAY (200 ms) behind Task A.
    // After option-(b): no outer lock; Task B returns in ≈ 0 ms.
    // Threshold: 50% of RECORD_DELAY = 100 ms.
    let threshold = RECORD_DELAY.mul_f64(0.5); // 100 ms

    assert!(
        wait_elapsed < threshold,
        "A concurrent recall took {wait_elapsed:?}, exceeding threshold {threshold:?}. \
         It was blocked behind an in-flight record_memory. \
         Option-(b) fix: Arc<dyn MemoryStore> + &self eliminates the outer lock (P1 #4)."
    );

    record_handle.await.expect("record task panicked");
}

// ---------------------------------------------------------------------------
// Test 4 — CONCURRENCY: N concurrent recalls do not serialize
// ---------------------------------------------------------------------------

/// CONCURRENCY pin: with the current outer `Mutex<dyn MemoryStore>`, N=4
/// concurrent calls to `store.lock().await` queue up FIFO.  Each recall holds
/// the lock for ≈ RECALL_DELAY (100 ms), so total wall time ≈ N × RECALL_DELAY
/// (400 ms).
///
/// After option-(b), `recall` takes `&self` on an `Arc<dyn MemoryStore>` with
/// no outer exclusive lock.  All four recalls proceed simultaneously, total ≈
/// RECALL_DELAY (100 ms).
///
/// The test exercises the outer mutex directly — same arc the agent holds — so
/// it probes the actual serialization point, not a private data handle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_recall_calls_do_not_serialize() {
    // recall_delay=100ms per call; with outer Mutex: N×100ms = 400ms total.
    let (_agent, store) = make_agent(Duration::ZERO, RECALL_DELAY);

    let start = Instant::now();

    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let store_clone = Arc::clone(&store);
        handles.push(tokio::spawn(async move {
            // Call recall directly — no outer lock; concurrent reads proceed in parallel.
            store_clone
                .recall(&MemoryQuery {
                    text: String::new(),
                    limit: 32,
                    kind: None,
                })
                .await
                .unwrap()
        }));
    }

    for h in handles {
        h.await.expect("recall task panicked");
    }

    let elapsed = start.elapsed();

    // Concurrent ceiling: 2× RECALL_DELAY = 200 ms.
    // Serialized floor: (N-1) × RECALL_DELAY = 300 ms.
    let concurrent_ceiling = RECALL_DELAY.mul_f64(2.0); // 200 ms

    assert!(
        elapsed < concurrent_ceiling,
        "N={N} concurrent recalls serialized: elapsed={elapsed:?} \
         >= concurrent_ceiling={concurrent_ceiling:?} (RECALL_DELAY={RECALL_DELAY:?}). \
         After option-(b), Arc<dyn MemoryStore> + &self recall runs concurrently (P1 #4)."
    );
}

// ---------------------------------------------------------------------------
// Test 5 — SAFETY: record_memory is synchronous (no fire-and-forget)
// ---------------------------------------------------------------------------

/// SAFETY: `record_memory` is declared `async fn` and callers `await` it.
/// After the await the write MUST be durable — no intermediate detached spawn.
///
/// Current status: PASSES (regression guard).  A fire-and-forget attempt
/// (`tokio::spawn` inside `record_memory` + immediate return) would let this
/// test pass on a single-thread runtime (scheduler runs the spawn before the
/// next await) but fail on `flavor = "multi_thread"` where the spawned task
/// may land on a different worker and not complete before `recall`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn record_memory_returns_only_after_write_completes() {
    let (agent, store) = make_agent(Duration::ZERO, Duration::ZERO);

    agent.record_memory(make_candidate(7)).await;

    // Query directly — no outer Mutex after option-(b); no yield, no sleep.
    let count = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 64,
            kind: None,
        })
        .await
        .unwrap()
        .len();

    assert_eq!(
        count, 1,
        "record_memory returned but store is still empty (count={count}). \
         Detached spawn (fire-and-forget): write was not completed before \
         record_memory returned (P1 #4)."
    );
}
