#![allow(clippy::unwrap_used, clippy::expect_used)]
//! M3 of `docs/plans/memory-recall-index-spec.md`, at agent level: the
//! "Relevant Memories" an `Agent` recalls for a turn never include `Outcome`
//! logs, while other kinds that match still do. No new API: it observes the
//! `MemoryRecalled` event of a turn.

use std::sync::Arc;

use hone_core::{Agent, AgentConfig, AgentMode, RetryConfig};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use hone_proto::{AgentEvent, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

fn agent_with_store(store: Arc<InMemoryStore>) -> Agent {
    agent_with_embedder(store, None)
}

fn agent_with_embedder(
    store: Arc<InMemoryStore>,
    embedder: Option<Arc<dyn hone_memory::Embedder>>,
) -> Agent {
    Agent::new(AgentConfig {
        provider: Arc::new(MockProvider::new(vec!["ok".to_owned()])),
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "You are a helpful assistant.".to_owned(),
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
        memory_store: Some(store as Arc<dyn MemoryStore>),
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        mode: AgentMode::Build,
        embedder,
        policy: None,
    })
}

async fn seed(store: &Arc<InMemoryStore>, id: &str, content: &str, kind: MemoryKind) {
    seed_with(store, id, content, kind, None).await;
}

async fn seed_with(
    store: &Arc<InMemoryStore>,
    id: &str,
    content: &str,
    kind: MemoryKind,
    embedding: Option<Vec<f32>>,
) {
    store
        .record(Memory {
            id: id.to_owned(),
            content: content.to_owned(),
            tags: vec![],
            weight: 1.0,
            created_at: 1_000_000,
            decay_at: None,
            kind,
            description: None,
            embedding,
        })
        .await
        .unwrap();
}

#[tokio::test]
async fn test_m3_turn_should_recall_matching_feedback_but_not_outcome_logs() {
    let store = Arc::new(InMemoryStore::new());
    seed(
        &store,
        "fb",
        "FEEDBACK-NOTE prefer rebasing the zorblax branch before merging",
        MemoryKind::Feedback,
    )
    .await;
    seed(
        &store,
        "out",
        "OUTCOME-LOG task: rebasing the zorblax branch before merging, result: done",
        MemoryKind::Outcome,
    )
    .await;
    let mut agent = agent_with_store(Arc::clone(&store));

    let (tx, mut rx) = mpsc::channel(64);
    agent
        .run_turn("rebasing the zorblax branch before merging", tx)
        .await
        .expect("turn");

    let mut recalled: Vec<(String, String)> = vec![];
    while let Ok(ev) = rx.try_recv() {
        if let AgentEvent::MemoryRecalled { entries } = ev {
            recalled.extend(entries);
        }
    }
    assert!(
        recalled.iter().any(|(_, s)| s.contains("FEEDBACK-NOTE")),
        "the matching feedback must still be recalled; got {recalled:?}"
    );
    assert!(
        !recalled.iter().any(|(_, s)| s.contains("OUTCOME-LOG")),
        "outcome logs must not come back as relevant memories; got {recalled:?}"
    );
}

// ---- M9: semantic recall keeps offering 5 candidates ---------------------------

/// Embeds every text to the same unit vector.
struct FixedEmbedder;

#[async_trait::async_trait]
impl hone_memory::Embedder for FixedEmbedder {
    fn dim(&self) -> usize {
        2
    }

    async fn embed(&self, _text: &str) -> Result<hone_memory::Embedding, hone_memory::EmbedError> {
        Ok(vec![1.0, 0.0])
    }
}

#[tokio::test]
async fn test_m9_semantic_recall_should_still_offer_five_candidates_after_dropping_outcomes() {
    let store = Arc::new(InMemoryStore::new());
    // Six outcome logs are the closest vectors; six feedback notes come next.
    // None shares a word with the prompt, so only the semantic path can match.
    for i in 0..6 {
        seed_with(
            &store,
            &format!("o{i}"),
            &format!("OUTCOME-LOG number {i} shipped"),
            MemoryKind::Outcome,
            Some(vec![1.0, 0.0]),
        )
        .await;
        seed_with(
            &store,
            &format!("f{i}"),
            &format!("FEEDBACK-NOTE number {i} observed"),
            MemoryKind::Feedback,
            Some(vec![0.8, 0.6]),
        )
        .await;
    }
    let mut agent = agent_with_embedder(Arc::clone(&store), Some(Arc::new(FixedEmbedder)));

    let (tx, mut rx) = mpsc::channel(64);
    agent.run_turn("quokka wrangling", tx).await.expect("turn");

    let mut recalled: Vec<(String, String)> = vec![];
    while let Ok(ev) = rx.try_recv() {
        if let AgentEvent::MemoryRecalled { entries } = ev {
            recalled.extend(entries);
        }
    }
    let notes = recalled.iter().filter(|(_, s)| s.contains("FEEDBACK-NOTE")).count();
    assert!(
        !recalled.iter().any(|(_, s)| s.contains("OUTCOME-LOG")),
        "no outcome logs; got {recalled:?}"
    );
    assert_eq!(notes, 5, "five non-outcome candidates remain; got {recalled:?}");
}
