#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! G4 and G6 of `docs/plans/fts5-recall-spec.md` through a real `Agent` with a
//! `SqliteStore`: choosing `[memory] recaller = "fts5"` makes receipts name `fts5` with
//! params `{"exclude":["outcome"],"limit":5}`; with an encrypted store the choice falls
//! back to `bm25` (receipts name `bm25`).
//!
//! API required (new): `fts5` registered as a built-in recaller in
//! `StrategyRegistry::with_builtins()`, and a public `StrategyDeps.encrypted_memory: bool`
//! (default false) telling the registry the store is encrypted. Existing API (PR D):
//! `hone_core::strategy::{StrategyRegistry, StrategyDeps, ResolvedStrategies}`,
//! `hone_config::strategy::{StrategySelection, Flags}`, `Agent::with_strategies`.

mod receipts_support;

use std::sync::Arc;

use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};
use hone_core::strategy::{ResolvedStrategies, StrategyDeps, StrategyRegistry};
use hone_memory::{Memory, MemoryKind, MemoryStore, SqliteStore};
use hone_proto::{AgentEvent, ContextReceipt, Slot, SourceRef, sha256_hex};
use receipts_support::*;

fn strategies(deps: StrategyDeps) -> Arc<ResolvedStrategies> {
    let user = ConfigLayer::from_toml(
        "/home/u/.config/hone/config.toml",
        "[memory]\nrecaller = \"fts5\"\n",
    )
    .unwrap();
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
    Arc::new(
        StrategyRegistry::with_builtins()
            .build(&sel, &deps)
            .unwrap_or_else(|e| panic!("build failed: {e}")),
    )
}

async fn store() -> Arc<SqliteStore> {
    let store = Arc::new(SqliteStore::in_memory().await.unwrap());
    store
        .record(Memory {
            id: "m1".to_owned(),
            content: "rebasing the zorblax branch before merging".to_owned(),
            tags: vec![],
            weight: 1.0,
            created_at: 1_000_000,
            decay_at: None,
            kind: MemoryKind::Feedback,
            description: None,
            embedding: None,
        })
        .await
        .unwrap();
    store
}

async fn recall_receipt(s: Arc<ResolvedStrategies>) -> ContextReceipt {
    let Rig { provider, agent } = rig(
        vec![],
        Opts {
            memory: Some(store().await),
            ..Default::default()
        },
    );
    let mut r = Rig {
        provider,
        agent: agent.with_strategies(s),
    };
    let events = r.turn("rebasing the zorblax branch").await;
    events
        .into_iter()
        .find_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt),
            _ => None,
        })
        .expect("a receipt")
}

fn recall_block(r: &ContextReceipt) -> &hone_proto::BlockReceipt {
    r.blocks
        .iter()
        .find(|b| b.slot == Slot::Recall)
        .expect("a Recall block")
}

fn recalled_m1(b: &hone_proto::BlockReceipt) -> bool {
    b.sources
        .iter()
        .any(|s| matches!(&s.source, SourceRef::Memory { id, .. } if id == "m1"))
}

#[tokio::test]
async fn test_g4_receipts_should_name_the_fts5_strategy_with_its_params() {
    let receipt = recall_receipt(strategies(StrategyDeps::default())).await;

    let b = recall_block(&receipt);
    assert_eq!(b.strategy.name, "fts5");
    assert_eq!(
        b.strategy.params_sha256,
        sha256_hex(br#"{"exclude":["outcome"],"limit":5}"#)
    );
    assert!(recalled_m1(b), "recalled through fts5: {:?}", b.sources);
}

#[tokio::test]
async fn test_g6_fts5_with_an_encrypted_store_should_fall_back_to_bm25() {
    let receipt = recall_receipt(strategies(StrategyDeps {
        encrypted_memory: true,
        ..Default::default()
    }))
    .await;

    let b = recall_block(&receipt);
    assert_eq!(b.strategy.name, "bm25", "FTS5 cannot index ciphertext");
    assert!(recalled_m1(b), "recall still works through bm25");
}
