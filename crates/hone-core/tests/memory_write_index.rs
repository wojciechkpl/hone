#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W13 of `docs/plans/memory-write-tool-spec.md` (review round 1), memory-index half:
//! tool-written memories reach the memory index only for their own project.
//!
//! ASSUMED API (new): `hone_core::memory_index::render_index_in(store: &dyn MemoryStore,
//! budget: IndexBudget, working_dir: &Path) -> Result<String, MemoryError>`: like
//! `render_index`, but a `source:tool` memory is included only when its `project:<id>` tag
//! matches the project of `working_dir` (the canonical directory, or the git top-level).
//! Other memories are always included. Separate file because it does not compile yet.

mod receipts_support;

use std::sync::Arc;

use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};
use hone_core::Agent;
use hone_core::memory_index::render_index_in;
use hone_core::strategy::{StrategyDeps, StrategyRegistry};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use hone_proto::AgentEvent;
use receipts_support::*;
use serde_json::json;
use tokio::sync::mpsc;

async fn write_from(dir: &std::path::Path, store: Arc<dyn MemoryStore>, fact: &str) {
    let user = ConfigLayer::from_toml("/u/config.toml", "[memory]\nwriter = \"tool\"\n").unwrap();
    let sel = StrategySelection::from_sources(
        Some(&user),
        None,
        &|_| None,
        &Flags {
            no_inject: true,
            ..Default::default()
        },
    )
    .unwrap();
    let strategies = Arc::new(
        StrategyRegistry::with_builtins()
            .build(&sel, &StrategyDeps::default())
            .unwrap(),
    );
    let provider = Script::new(vec![vec![call(
        "c1",
        "memory_write",
        json!({ "kind": "reference", "content": fact }),
    )]]);
    let mut agent = Agent::new(config(
        provider,
        Opts {
            memory: Some(store),
            working_dir: Some(dir.to_path_buf()),
            ..Default::default()
        },
    ))
    .with_strategies(strategies);
    let (tx, _rx) = mpsc::channel::<AgentEvent>(4096);
    agent.run_turn("remember", tx).await.unwrap();
}

#[tokio::test]
async fn test_w13_the_memory_index_should_include_tool_memories_only_for_their_project() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let (a_dir, b_dir) = (
        std::fs::canonicalize(a.path()).unwrap(),
        std::fs::canonicalize(b.path()).unwrap(),
    );
    let store: Arc<dyn MemoryStore> = Arc::new(InMemoryStore::new());
    write_from(&a_dir, store.clone(), "Alpha deploy notes written by the model.").await;
    store
        .record(Memory {
            id: "u1".to_owned(),
            content: "Shared notes written by the user.".to_owned(),
            tags: vec![],
            weight: 1.0,
            created_at: 1_000_000,
            decay_at: None,
            kind: MemoryKind::Reference,
            description: None,
            embedding: None,
        })
        .await
        .unwrap();

    let in_a = render_index_in(store.as_ref(), Default::default(), &a_dir).await.unwrap();
    let in_b = render_index_in(store.as_ref(), Default::default(), &b_dir).await.unwrap();

    assert!(
        in_a.contains("written by the model") && in_a.contains("written by the user"),
        "{in_a}"
    );
    assert!(in_b.contains("written by the user"), "{in_b}");
    assert!(!in_b.contains("written by the model"), "{in_b}");
}
