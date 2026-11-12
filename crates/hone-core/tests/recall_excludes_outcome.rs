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
        embedder: None,
        policy: None,
    })
}

async fn seed(store: &Arc<InMemoryStore>, id: &str, content: &str, kind: MemoryKind) {
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
            embedding: None,
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
