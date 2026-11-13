#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! S5, S6 and S8 of `docs/plans/strategy-registry-spec.md`: the strategy traits and
//! registry, built-ins that wrap today's code byte for byte, and
//! `Agent::with_strategies`. Receipt assertions need PR B underneath.
//!
//! API required (new module `hone_core::strategy`, re-exported where noted):
//!
//! ```ignore
//! pub enum Origin { Builtin, Feature(String), Plugin(String) }       // Debug Clone PartialEq Eq
//! #[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Cadence { Once, EachTurn }
//! #[derive(Default)] pub struct StrategyDeps { /* memory store, index, working dir, ... */ }
//! pub enum StrategyError { .. }                                      // Display names the strategy
//!                                                                    // and lists the registered names
//! pub type Factory<T> = Arc<dyn Fn(&serde_json::Value, &StrategyDeps)
//!                           -> Result<Arc<T>, StrategyError> + Send + Sync>;   // T: ?Sized
//!
//! pub struct TurnView<'a> { pub prompt: &'a str, pub turn: u32, pub history: &'a [Message],
//!                           pub working_dir: &'a Path, pub budget_tokens: u32 }
//! pub struct ContextBlock { pub slot: hone_proto::Slot, pub text: String,
//!                           pub sources: Vec<hone_proto::ScoredSource> }
//! pub struct RecallQuery<'a> { pub text: &'a str, pub k: usize,
//!                              pub exclude_kinds: &'a [MemoryKind], pub scope: Option<&'a str> }
//! pub struct TurnRecord { pub turn: u32, pub user_text: String, pub response_text: String }
//!
//! #[async_trait] pub trait ContextSource: Send + Sync {
//!     fn id(&self) -> &StrategyId; fn cadence(&self) -> Cadence;
//!     async fn gather(&self, view: &TurnView<'_>) -> Result<Vec<ContextBlock>, StrategyError>; }
//! #[async_trait] pub trait Recaller: Send + Sync {
//!     fn id(&self) -> &StrategyId;
//!     async fn recall(&self, store: &dyn MemoryStore, q: &RecallQuery<'_>)
//!         -> Result<Vec<ScoredMemory>, MemoryError>; }
//! #[async_trait] pub trait MemoryWriter: Send + Sync {
//!     fn id(&self) -> &StrategyId;
//!     async fn propose(&self, turn: &TurnRecord) -> Vec<MemoryCandidate>; }   // memory_worthy::MemoryCandidate
//!
//! impl StrategyRegistry {
//!     pub fn with_builtins() -> Self;
//!     pub fn register_source(&mut self, name: &str, origin: Origin, f: Factory<dyn ContextSource>);
//!     pub fn register_recaller(&mut self, name: &str, origin: Origin, f: Factory<dyn Recaller>);
//!     pub fn register_writer(&mut self, name: &str, origin: Origin, f: Factory<dyn MemoryWriter>);
//!     /// (slot, name, origin); slot is the config key: "context.sources", "context.skeleton",
//!     /// "context.compactor", "memory.recaller", "memory.writer".
//!     pub fn list(&self) -> Vec<(String, String, Origin)>;
//!     pub fn build(&self, sel: &StrategySelection, deps: &StrategyDeps)
//!         -> Result<ResolvedStrategies, StrategyError>;
//! }
//! impl ResolvedStrategies { pub fn has_source(&self, name: &str) -> bool; }
//! impl Agent { pub fn with_strategies(self, s: Arc<ResolvedStrategies>) -> Agent; }
//! ```
//!
//! No field is added to `AgentConfig` (S8): the golden fixture of PR B still holds for an
//! agent built without `with_strategies`, and for one built with the default selection.
//! With built-ins, receipts carry the names and `params_sha256` values of PR B
//! (recall `bm25` / `{"exclude":["outcome"],"limit":5}`; skeleton `skeleton:<name>` /
//! `{"gap_threshold":0.4,"max_tokens":2000}`).

mod receipts_support;

use std::path::Path;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::{Flags, StrategySelection};
use hone_core::memory_format::StructuredBody;
use hone_core::memory_worthy::MemoryCandidate;
use hone_core::strategy::{
    Cadence, ContextBlock, ContextSource, MemoryWriter, Origin, RecallQuery, Recaller,
    ResolvedStrategies, StrategyDeps, StrategyError, StrategyRegistry, TurnRecord, TurnView,
};
use hone_core::{Agent, Skill, SkillRegistry};
use hone_memory::{InMemoryStore, Memory, MemoryError, MemoryKind, MemoryStore, ScoredMemory};
use hone_proto::{AgentEvent, ContextReceipt, Slot, StrategyId, sha256_hex};
use receipts_support::*;
use serde_json::json;

const GOLDEN: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/context_request_golden.json"
);

fn no_env(_: &str) -> Option<String> {
    None
}

/// No injection (the golden session is built with `inject_context: false`).
fn no_inject() -> Flags {
    Flags {
        no_inject: true,
        ..Default::default()
    }
}

fn resolved(sel: &StrategySelection) -> Arc<ResolvedStrategies> {
    Arc::new(
        StrategyRegistry::with_builtins()
            .build(sel, &StrategyDeps::default())
            .unwrap_or_else(|e| panic!("build failed: {e}")),
    )
}

fn user_layer(raw: &str) -> ConfigLayer {
    ConfigLayer::from_toml("/home/u/.config/hone/config.toml", raw).unwrap()
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

async fn seeded_store() -> Arc<InMemoryStore> {
    let store = Arc::new(InMemoryStore::new());
    for (id, text, kind) in [
        (
            "m1",
            "prefer small commits when refactoring the parser module",
            MemoryKind::Feedback,
        ),
        (
            "m2",
            "the parser module lives in src/parser and has golden tests",
            MemoryKind::Project,
        ),
        ("m3", "unrelated note about lunch", MemoryKind::User),
    ] {
        store.record(mem(id, text, kind)).await.unwrap();
    }
    store
}

fn skills() -> Arc<SkillRegistry> {
    let mut reg = SkillRegistry::new();
    reg.register(Skill {
        name: "refactor-helper".to_owned(),
        description: "help with refactors".to_owned(),
        triggers: vec!["refactor".to_owned()],
        tools: vec![],
        model: None,
        always: false,
        body: "Keep diffs small.".to_owned(),
    });
    Arc::new(reg)
}

fn receipts(events: &[AgentEvent]) -> Vec<ContextReceipt> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .collect()
}

// ---- S5: registry ------------------------------------------------------------------------

#[test]
fn test_s5_with_builtins_should_list_every_builtin_under_its_config_key() {
    let listed = StrategyRegistry::with_builtins().list();
    let names = |slot: &str| -> Vec<String> {
        let mut v: Vec<String> = listed
            .iter()
            .filter(|(s, _, o)| s == slot && *o == Origin::Builtin)
            .map(|(_, n, _)| n.clone())
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        names("context.sources"),
        ["env-snapshot", "memory-index", "recall", "retrieval", "skeleton", "skills"]
    );
    assert_eq!(
        names("context.skeleton"),
        [
            "adaptive",
            "call-graph",
            "dependency-graph",
            "flat",
            "hybrid"
        ]
    );
    assert_eq!(names("context.compactor"), ["graduated", "trim-only"]);
    assert_eq!(names("memory.recaller"), ["bm25", "fts5", "none"]);
    assert_eq!(names("memory.writer"), ["off", "tool", "triggers", "triggers+tool"]);
}

#[test]
fn test_s5_the_default_selection_should_build() {
    let sel = StrategySelection::from_sources(None, None, &no_env, &Flags::default()).unwrap();
    let r = resolved(&sel);
    for source in ["memory-index", "recall", "skills", "skeleton"] {
        assert!(r.has_source(source), "{source}");
    }
}

#[test]
fn test_s5_an_unregistered_name_should_fail_the_build_naming_it_and_the_valid_ones() {
    let mut sel = StrategySelection::from_sources(None, None, &no_env, &Flags::default()).unwrap();
    sel.recaller.value = "vector-db".to_owned(); // plain data: nothing stops a caller setting it
    let err = StrategyRegistry::with_builtins()
        .build(&sel, &StrategyDeps::default())
        .err()
        .expect("vector-db is not registered");
    let text = err.to_string();
    assert!(text.contains("vector-db"), "{text}");
    assert!(text.contains("bm25"), "valid names are listed: {text}");
}

// ---- custom strategies through the seam ----------------------------------------------------------

struct FixedRecaller {
    id: StrategyId,
    seen: Arc<Mutex<Vec<(String, usize, Vec<MemoryKind>)>>>,
}

#[async_trait]
impl Recaller for FixedRecaller {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    async fn recall(
        &self,
        _store: &dyn MemoryStore,
        q: &RecallQuery<'_>,
    ) -> Result<Vec<ScoredMemory>, MemoryError> {
        self.seen
            .lock()
            .unwrap()
            .push((q.text.to_owned(), q.k, q.exclude_kinds.to_vec()));
        Ok(vec![ScoredMemory {
            memory: mem(
                "fixed-1",
                "FIXED-RECALL-TEXT from the custom recaller",
                MemoryKind::Feedback,
            ),
            score: 1.0,
        }])
    }
}

fn custom_recaller_registry(
    seen: Arc<Mutex<Vec<(String, usize, Vec<MemoryKind>)>>>,
) -> StrategyRegistry {
    let mut reg = StrategyRegistry::with_builtins();
    reg.register_recaller(
        "fixed",
        Origin::Plugin("test".to_owned()),
        Arc::new(move |_params, _deps| {
            Ok(Arc::new(FixedRecaller {
                id: StrategyId {
                    slot: Slot::Recall,
                    name: "fixed".to_owned(),
                    version: "1.0".to_owned(),
                    params_sha256: sha256_hex(b"{}"),
                },
                seen: seen.clone(),
            }) as Arc<dyn Recaller>)
        }),
    );
    reg
}

#[test]
fn test_s5_a_registered_strategy_should_be_listed_with_its_origin() {
    let reg = custom_recaller_registry(Arc::default());
    assert!(reg.list().contains(&(
        "memory.recaller".to_owned(),
        "fixed".to_owned(),
        Origin::Plugin("test".to_owned())
    )));
}

#[tokio::test]
async fn test_s5_a_custom_recaller_should_feed_the_prompt_and_get_the_selections_parameters() {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let reg = custom_recaller_registry(seen.clone());
    let u = user_layer("[memory]\ntop_k = 4\n");
    let mut sel = StrategySelection::from_sources(Some(&u), None, &no_env, &no_inject()).unwrap();
    sel.recaller.value = "fixed".to_owned();
    let strategies = Arc::new(reg.build(&sel, &StrategyDeps::default()).unwrap());
    let store = seeded_store().await;
    let provider = Script::new(vec![]);
    let mut agent = Agent::new(config(
        provider.clone(),
        Opts {
            memory: Some(store),
            ..Default::default()
        },
    ))
    .with_strategies(strategies);

    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);
    agent
        .run_turn("rebase the zorblax branch", tx)
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    let sent = provider.request(0);
    let text: String = sent
        .messages
        .iter()
        .map(|m| m.content.as_text())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("FIXED-RECALL-TEXT"),
        "the custom recaller's memory is in the prompt"
    );
    assert_eq!(
        *seen.lock().unwrap(),
        [(
            "rebase the zorblax branch".to_owned(),
            4,
            vec![MemoryKind::Outcome]
        )],
        "text, top_k and exclude_kinds come from the prompt and the selection"
    );
    // PR B: the receipt names the strategy that ran.
    let r = receipts(&events);
    let recall = r[0]
        .blocks
        .iter()
        .find(|b| b.slot == Slot::Recall)
        .expect("a Recall block");
    assert_eq!(recall.strategy.name, "fixed");
}

struct AlwaysWriter {
    id: StrategyId,
}

#[async_trait]
impl MemoryWriter for AlwaysWriter {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    async fn propose(&self, turn: &TurnRecord) -> Vec<MemoryCandidate> {
        vec![MemoryCandidate {
            kind: MemoryKind::Feedback,
            description: Some(format!("written after turn {}", turn.turn)),
            body: StructuredBody {
                rule: format!("WRITER-RULE for: {}", turn.user_text),
                why: None,
                how: None,
            },
        }]
    }
}

#[tokio::test]
async fn test_s5_a_custom_writer_should_have_its_proposals_recorded_by_the_runtime() {
    let mut reg = StrategyRegistry::with_builtins();
    reg.register_writer(
        "always",
        Origin::Feature("test".to_owned()),
        Arc::new(|_p, _d| {
            Ok(Arc::new(AlwaysWriter {
                id: StrategyId {
                    slot: Slot::Recall,
                    name: "always".to_owned(),
                    version: "1.0".to_owned(),
                    params_sha256: sha256_hex(b"{}"),
                },
            }) as Arc<dyn MemoryWriter>)
        }),
    );
    let mut sel = StrategySelection::from_sources(None, None, &no_env, &no_inject()).unwrap();
    sel.writer.value = "always".to_owned();
    let strategies = Arc::new(reg.build(&sel, &StrategyDeps::default()).unwrap());
    let store = Arc::new(InMemoryStore::new());
    let mut agent = Agent::new(config(
        Script::new(vec![]),
        Opts {
            memory: Some(store.clone()),
            ..Default::default()
        },
    ))
    .with_strategies(strategies);

    let (tx, _rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);
    agent.run_turn("hello there", tx).await.unwrap();

    let stored = store
        .recall(&hone_memory::MemoryQuery {
            text: String::new(),
            limit: 10,
            kind: None,
        })
        .await
        .unwrap();
    assert!(
        stored
            .iter()
            .any(|m| m.content.contains("WRITER-RULE for: hello there")),
        "{:?}",
        stored.iter().map(|m| &m.content).collect::<Vec<_>>()
    );
}

struct EmptySource {
    id: StrategyId,
}

#[async_trait]
impl ContextSource for EmptySource {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    fn cadence(&self) -> Cadence {
        Cadence::Once
    }
    async fn gather(&self, view: &TurnView<'_>) -> Result<Vec<ContextBlock>, StrategyError> {
        let _ = (
            view.prompt,
            view.turn,
            view.history,
            view.working_dir,
            view.budget_tokens,
        );
        Ok(vec![])
    }
}

#[tokio::test]
async fn test_s5_a_custom_context_source_should_register_build_and_gather() {
    let mut reg = StrategyRegistry::with_builtins();
    reg.register_source(
        "empty",
        Origin::Builtin,
        Arc::new(|_p, _d| {
            Ok(Arc::new(EmptySource {
                id: StrategyId {
                    slot: Slot::Injection,
                    name: "empty".to_owned(),
                    version: "1.0".to_owned(),
                    params_sha256: sha256_hex(b"{}"),
                },
            }) as Arc<dyn ContextSource>)
        }),
    );
    let mut sel = StrategySelection::from_sources(None, None, &no_env, &no_inject()).unwrap();
    sel.sources.value.push("empty".to_owned());

    let r = reg.build(&sel, &StrategyDeps::default()).unwrap();

    assert!(r.has_source("empty"));
    let src = EmptySource {
        id: StrategyId {
            slot: Slot::Injection,
            name: "empty".to_owned(),
            version: "1.0".to_owned(),
            params_sha256: String::new(),
        },
    };
    let view = TurnView {
        prompt: "p",
        turn: 1,
        history: &[],
        working_dir: Path::new("/tmp"),
        budget_tokens: 100,
    };
    assert!(src.gather(&view).await.unwrap().is_empty());
    assert_eq!(src.cadence(), Cadence::Once);
}

// ---- S6 / S8: byte for byte ---------------------------------------------------------------------------

async fn golden_requests(strategies: Option<Arc<ResolvedStrategies>>) -> String {
    let store = seeded_store().await;
    let provider = Script::new(vec![vec![read("r1")], vec![read("r2")]]);
    let agent = Agent::new(config(
        provider.clone(),
        Opts {
            memory: Some(store),
            skills: Some(skills()),
            ..Default::default()
        },
    ));
    let mut agent = match strategies {
        Some(s) => agent.with_strategies(s),
        None => agent,
    };
    for prompt in ["refactor the parser module", "thanks, now add docs"] {
        let (tx, _rx) = tokio::sync::mpsc::channel::<AgentEvent>(1024);
        agent.run_turn(prompt, tx).await.unwrap();
    }
    let recorded: Vec<serde_json::Value> = (0..provider.request_count())
        .map(|i| {
            let req = provider.request(i);
            json!({
                "model": req.model.to_string(),
                "messages": req.messages,
                "tool_names": req.tools.iter().map(|t| t.name.clone()).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&recorded).unwrap() + "\n"
}

#[tokio::test]
async fn test_s8_an_agent_without_strategies_should_still_match_the_golden_fixture() {
    let expected = std::fs::read_to_string(GOLDEN).expect("golden fixture");
    assert_eq!(golden_requests(None).await, expected);
}

#[tokio::test]
async fn test_s6_the_default_selection_should_send_byte_identical_requests() {
    let sel = StrategySelection::from_sources(None, None, &no_env, &no_inject()).unwrap();
    let expected = std::fs::read_to_string(GOLDEN).expect("golden fixture");
    assert_eq!(golden_requests(Some(resolved(&sel))).await, expected);
}

#[tokio::test]
async fn test_s6_a_config_spelling_out_every_default_should_send_byte_identical_requests() {
    let u = user_layer(
        r#"
[context]
sources = ["memory-index", "recall", "skills", "skeleton"]
compactor = "graduated"

[context.skeleton]
use = "adaptive"
max_tokens = 2000
gap_threshold = 0.4

[context.graduated]
phase = false

[memory]
recaller = "bm25"
writer = "triggers"
top_k = 5
exclude_kinds = ["outcome"]
"#,
    );
    let sel = StrategySelection::from_sources(Some(&u), None, &no_env, &no_inject()).unwrap();
    let expected = std::fs::read_to_string(GOLDEN).expect("golden fixture");
    assert_eq!(golden_requests(Some(resolved(&sel))).await, expected);
}

#[tokio::test]
async fn test_s6_builtin_receipts_should_carry_the_names_and_params_hashes_of_pr_b() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/auth.rs"), "pub fn login() {}\n").unwrap();
    let store = seeded_store().await;
    let sel = StrategySelection::from_sources(None, None, &no_env, &Flags::default()).unwrap();
    let mut agent = Agent::new(config(
        Script::new(vec![]),
        Opts {
            memory: Some(store),
            working_dir: Some(dir.path().to_path_buf()),
            ..Default::default()
        },
    ))
    .with_strategies(resolved(&sel));

    let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);
    agent
        .run_turn("refactor the parser module", tx)
        .await
        .unwrap();
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    let r = receipts(&events);
    let block = |slot: Slot| {
        r[0].blocks
            .iter()
            .find(|b| b.slot == slot)
            .unwrap_or_else(|| panic!("{slot:?}"))
    };
    let recall = block(Slot::Recall);
    assert_eq!(recall.strategy.name, "bm25");
    assert_eq!(
        recall.strategy.params_sha256,
        sha256_hex(br#"{"exclude":["outcome"],"limit":5}"#)
    );
    let inj = block(Slot::Injection);
    assert_eq!(inj.strategy.name, "skeleton:adaptive");
    assert_eq!(
        inj.strategy.params_sha256,
        sha256_hex(br#"{"gap_threshold":0.4,"max_tokens":2000}"#)
    );
}
