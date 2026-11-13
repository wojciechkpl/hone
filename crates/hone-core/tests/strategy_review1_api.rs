#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Review round 1 of `docs/plans/strategy-registry-spec.md` in `hone-core`:
//! S13 (`--ensemble` uses the selection), S14 (writer = off stops outcome logs) and
//! the writer half of S21 (writer ids use their own slot). Separate from
//! `strategy_review1.rs` because these need new API and do not compile yet.
//!
//! API assumed (new):
//! - `AgentEnsemble::with_strategies(self, Arc<ResolvedStrategies>) -> AgentEnsemble`
//!   (spec S13): applied to every candidate and the ranker.
//! - `AdaptiveClassifier::with_strategies(store, &ResolvedStrategies) -> AdaptiveClassifier`
//!   (S14): the classifier the TUI keeps for after-turn outcome records; its
//!   `record_outcome` writes nothing when the resolved writer is off, and behaves
//!   as before otherwise.
//! - `ResolvedStrategies::writer_id(&self) -> &hone_proto::StrategyId` (S21): the
//!   active writer's id; its `slot` is not `Slot::Recall`.

mod receipts_support;

use std::sync::Arc;

use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};
use hone_core::strategy::{ResolvedStrategies, StrategyDeps, StrategyRegistry};
use hone_core::{AdaptiveClassifier, AgentEnsemble, ComplexityTier, OutcomeRecord};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryQuery, MemoryStore};
use hone_proto::{AgentEvent, Slot};
use receipts_support::*;
use serde_json::json;
use tokio::sync::mpsc;

const USER: &str = "/home/u/.config/hone/config.toml";
const PROJECT: &str = "/repo/.hone/config.toml";

fn no_inject() -> Flags {
    Flags {
        no_inject: true,
        ..Default::default()
    }
}

fn resolve(user: Option<&str>, project: Option<&str>) -> Arc<ResolvedStrategies> {
    let u = user.map(|t| ConfigLayer::from_toml(USER, t).unwrap());
    let p = project.map(|t| ConfigLayer::from_toml(PROJECT, t).unwrap());
    let sel =
        StrategySelection::from_sources(u.as_ref(), p.as_ref(), &|_| None, &no_inject()).unwrap();
    Arc::new(
        StrategyRegistry::with_builtins()
            .build(&sel, &StrategyDeps::default())
            .unwrap_or_else(|e| panic!("build failed: {e}")),
    )
}

fn mem(id: &str, content: &str, kind: MemoryKind) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

async fn all(store: &Arc<InMemoryStore>) -> Vec<Memory> {
    store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 100,
            kind: None,
        })
        .await
        .unwrap()
}

// ---- S13: --ensemble uses the selection ----------------------------------------------------------------------

const CORRECTION: &str = "don't use tabs for indentation, use four spaces instead";

async fn ensemble_requests_and_store(
    s: Arc<ResolvedStrategies>,
    store: Arc<InMemoryStore>,
    prompt: &str,
) -> Vec<String> {
    let provider = Script::new(vec![]);
    let proto = config(
        provider.clone(),
        Opts {
            memory: Some(store),
            ..Default::default()
        },
    );
    let ensemble = AgentEnsemble::new(proto, vec![0.0, 0.5]).with_strategies(s);
    let (tx, _rx) = mpsc::channel::<AgentEvent>(4096);
    ensemble.run(prompt, tx).await.expect("ensemble run");
    (0..provider.request_count())
        .map(|i| {
            provider
                .request(i)
                .messages
                .iter()
                .map(|m| m.content.as_text())
                .collect::<Vec<_>>()
                .join("\n---msg---\n")
        })
        .collect()
}

#[tokio::test]
async fn test_s13_ensemble_with_writer_off_should_write_no_memories() {
    let store = Arc::new(InMemoryStore::new());
    ensemble_requests_and_store(resolve(None, None), store.clone(), CORRECTION).await;
    assert!(!all(&store).await.is_empty(), "control: the triggers writer records a correction");

    let store = Arc::new(InMemoryStore::new());
    ensemble_requests_and_store(resolve(Some("[memory]\nwriter = \"off\"\n"), None), store.clone(), CORRECTION).await;
    assert_eq!(all(&store).await.len(), 0, "no candidate or ranker wrote anything");
}

#[tokio::test]
async fn test_s13_ensemble_with_recaller_none_should_send_no_relevant_memories() {
    let seed = || async {
        let store = Arc::new(InMemoryStore::new());
        store
            .record(mem(
                "m1",
                "rebasing the zorblax branch before merging",
                MemoryKind::Feedback,
            ))
            .await
            .unwrap();
        store
    };
    let prompt = "rebasing the zorblax branch";

    let control = ensemble_requests_and_store(resolve(None, None), seed().await, prompt).await;
    assert!(
        control.iter().any(|r| r.contains("# Relevant Memories")),
        "control: recall feeds the candidates"
    );

    let none = ensemble_requests_and_store(resolve(Some("[memory]\nrecaller = \"none\"\n"), None), seed().await, prompt).await;
    assert!(
        none.iter().all(|r| !r.contains("# Relevant Memories") && !r.contains("zorblax branch before")),
        "recaller = none holds for every candidate and the ranker"
    );
}

#[tokio::test]
async fn test_s13_ensemble_should_apply_the_compactor_and_sources_too() {
    // sources without "recall": no Relevant Memories even though one matches.
    let store = Arc::new(InMemoryStore::new());
    store
        .record(mem("m1", "rebasing the zorblax branch before merging", MemoryKind::Feedback))
        .await
        .unwrap();
    let s = resolve(Some("[context]\nsources = [\"memory-index\", \"skills\"]\n"), None);

    let reqs = ensemble_requests_and_store(s, store, "rebasing the zorblax branch").await;

    assert!(reqs.iter().all(|r| !r.contains("# Relevant Memories")));
}

// ---- S14: writer = off stops outcome logs -------------------------------------------------------------------

fn outcome() -> OutcomeRecord {
    OutcomeRecord {
        tier: ComplexityTier::Simple,
        model: "mock:test".to_owned(),
        success: true,
        cost_usd: 0.01,
        total_tokens: 100,
        task_summary: "fix the typo".to_owned(),
    }
}

#[tokio::test]
async fn test_s14_writer_off_should_stop_outcome_records() {
    let store = Arc::new(InMemoryStore::new());
    let on = AdaptiveClassifier::with_strategies(store.clone(), &resolve(None, None));
    on.record_outcome(outcome()).await;
    let rows = all(&store).await;
    assert_eq!(rows.len(), 1, "control: the default writer logs outcomes");
    assert_eq!(rows[0].kind, MemoryKind::Outcome);

    let store = Arc::new(InMemoryStore::new());
    let off = AdaptiveClassifier::with_strategies(store.clone(), &resolve(Some("[memory]\nwriter = \"off\"\n"), None));
    off.record_outcome(outcome()).await;
    off.record_outcome(OutcomeRecord { success: false, ..outcome() }).await;
    assert_eq!(all(&store).await.len(), 0, "writer = off: no Outcome rows");
}

#[tokio::test]
async fn test_s14_a_project_file_can_turn_the_writer_off_for_outcomes_too() {
    let store = Arc::new(InMemoryStore::new());
    let off = AdaptiveClassifier::with_strategies(store.clone(), &resolve(None, Some("[memory]\nwriter = \"off\"\n")));

    off.record_outcome(outcome()).await;

    assert_eq!(all(&store).await.len(), 0);
}

#[test]
fn test_s21_the_writer_id_should_use_its_own_slot_not_recall() {
    for toml in ["", "[memory]\nwriter = \"off\"\n"] {
        let s = resolve(Some(toml), None);

        let id = s.writer_id();

        assert_ne!(id.slot, Slot::Recall, "{toml:?}");
        let slot = serde_json::to_value(id.slot).unwrap().to_string();
        assert!(slot.contains("writer"), "the slot is the memory writer's: {slot}");
    }
}
