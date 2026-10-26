//! Red-phase integration tests for the `record_memory(MemoryCandidate)` path (R5+R6).
//!
//! Strategy: rather than driving `run_turn` (which requires a wired provider and
//! produces non-deterministic memory IDs), these tests call the new
//! `Agent::record_memory(candidate)` method directly after building the candidate
//! via `classify_turn`.  This exercises the full classify → format → dedup →
//! store pipeline without the provider machinery.
//!
//! These tests MUST fail to compile on `main` because:
//!   - `hone_core::memory_worthy` does not exist.
//!   - `Agent::record_memory` still takes `(user_input, reply)`, not a `MemoryCandidate`.
//!
//! Do not modify these tests during the green phase — they are the contract.

use std::sync::Arc;

use hone_core::memory_worthy::classify_turn;
use hone_core::{Agent, AgentConfig, AgentMode, RetryConfig};
use hone_memory::{InMemoryStore, MemoryKind, MemoryQuery, MemoryStore};
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;

// ---------------------------------------------------------------------------
// Helper: build a minimal AgentConfig with an InMemoryStore attached.
// Returns (Agent, Arc<dyn MemoryStore>) so tests can inspect the store directly.
// ---------------------------------------------------------------------------

fn make_agent_with_store() -> (Agent, Arc<dyn hone_memory::MemoryStore>) {
    let store: Arc<dyn hone_memory::MemoryStore> = Arc::new(InMemoryStore::new());

    let provider = Arc::new(MockProvider::new(vec!["ok".to_owned()]));

    let config = AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
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
        memory_store: Some(Arc::clone(&store)),
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
    };

    (Agent::new(config), store)
}

// ---------------------------------------------------------------------------
// Test 21 — noisy turn does not write memory
// ---------------------------------------------------------------------------

/// A turn whose user_input + reply trigger no classifier rule must not produce
/// any record in the store.  `record_memory` must silently do nothing.
#[tokio::test]
async fn noisy_turn_does_not_write_memory() {
    let (agent, store) = make_agent_with_store();

    // This is a noisy turn: plain factual Q&A, no triggers.
    let candidate = classify_turn("how do I list files in src/", "use ls or fd");

    // The classifier must return None — if it doesn't, the test setup is wrong
    // and should fail loudly.
    assert!(
        candidate.is_none(),
        "classify_turn should return None for a noisy turn; got {candidate:?}"
    );

    // Call record_memory with None (noisy path).
    // The new signature: `async fn record_memory(&self, candidate: Option<MemoryCandidate>)`
    // OR the caller is expected to skip the call when classify_turn returns None.
    // Either way, nothing should reach the store.
    //
    // If record_memory still takes (user_input, reply), this will fail to compile —
    // that is intentional (red phase).
    if let Some(c) = candidate {
        agent.record_memory(c).await;
    }

    // Store must remain empty.
    let recalled = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(
        recalled.len(),
        0,
        "noisy turn must not write anything to the store; got {} record(s)",
        recalled.len()
    );
}

// ---------------------------------------------------------------------------
// Test 22 — correction turn writes a Feedback memory
// ---------------------------------------------------------------------------

/// A turn with a correction pattern ("don't mock the DB") must produce exactly
/// one record in the store with `kind == Feedback`.
#[tokio::test]
async fn correction_turn_writes_feedback_memory() {
    let (agent, store) = make_agent_with_store();

    let candidate = classify_turn(
        "don't mock the database",
        "Understood, I'll use the real DB in tests.",
    )
    .expect("correction pattern must be classified as Some(_)");

    assert_eq!(
        candidate.kind,
        MemoryKind::Feedback,
        "correction must produce Feedback kind"
    );

    // Record it via the new single-argument signature.
    agent.record_memory(candidate).await;

    let recalled = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(
        recalled.len(),
        1,
        "correction turn must write exactly 1 memory; got {}",
        recalled.len()
    );
    assert_eq!(
        recalled[0].kind,
        MemoryKind::Feedback,
        "stored memory must have kind Feedback; got {:?}",
        recalled[0].kind
    );
}

// ---------------------------------------------------------------------------
// Test 23 — duplicate correction upserts instead of inserting twice
// ---------------------------------------------------------------------------

/// Running the same correction turn twice must result in exactly 1 record in
/// the store (dedup path), not 2 separate records.
///
/// The dedup threshold is 0.8 (from the plan). The second call's `created_at`
/// must reflect the second turn (greater than or equal to the first).
#[tokio::test]
async fn dup_correction_upserts_not_inserts() {
    let (agent, store) = make_agent_with_store();

    let candidate1 =
        classify_turn("don't mock the database", "Understood.").expect("correction must classify");

    let candidate2 = classify_turn("don't mock the database", "Got it.")
        .expect("correction must classify on second call too");

    // Record first time.
    agent.record_memory(candidate1).await;

    // Brief pause so the second `created_at` is guaranteed >= first.
    // (Timestamps are in seconds; two same-second calls are fine since
    //  the upsert semantic is "keep latest", not "reject if same second".)

    // Record second time — should upsert, not insert.
    agent.record_memory(candidate2).await;

    let recalled = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .unwrap();

    assert_eq!(
        recalled.len(),
        1,
        "duplicate correction must upsert (1 record), not insert twice (2 records); got {}",
        recalled.len()
    );
}
