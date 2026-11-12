//! The strategy seam: traits for context sources, memory recallers and memory
//! writers, a registry of named factories, and the resolved set an [`Agent`]
//! runs with.
//!
//! Built-ins wrap today's code. The built-in `recall`, `skills` and `memory-index`
//! sources have no work of their own to do in `gather`: the agent applies them in
//! the fixed block order (system prompt with the memory index, recall, skills,
//! history), which keeps the prompt-cache prefix steady. `has_source` tells the
//! runtime which are on. See `docs/plans/strategy-registry-spec.md`.
//!
//! [`Agent`]: crate::Agent

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use hone_config::strategy::StrategySelection;
use hone_index::SqliteIndex;
use hone_memory::{MemoryError, MemoryKind, MemoryQuery, MemoryStore, ScoredMemory};
use hone_proto::{Message, ScoredSource, Slot, SourceRef, StrategyId, canonical_json_sha256};

use crate::context_injector::{ContextInjector, SkeletonStrategy};
use crate::memory_worthy::{MemoryCandidate, classify_turn};

// ── Errors and origins ────────────────────────────────────────────────────────

/// Where a registered strategy comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Builtin,
    /// Compiled in behind a cargo feature.
    Feature(String),
    /// Provided by a plugin.
    Plugin(String),
}

#[derive(Debug, thiserror::Error)]
pub enum StrategyError {
    #[error("unknown {slot} strategy '{name}'; registered: {}", .valid.join(", "))]
    Unknown {
        slot: String,
        name: String,
        valid: Vec<String>,
    },
    #[error("strategy '{name}' ({slot}) failed: {reason}")]
    Failed {
        slot: String,
        name: String,
        reason: String,
    },
}

/// When a context source is gathered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cadence {
    /// On the first turn only.
    Once,
    /// On every turn.
    EachTurn,
}

// ── Inputs and outputs ────────────────────────────────────────────────────────

/// Handles a factory may use to build a strategy.
#[derive(Default, Clone)]
pub struct StrategyDeps {
    pub memory_store: Option<Arc<dyn MemoryStore>>,
    pub index: Option<Arc<SqliteIndex>>,
    pub working_dir: Option<PathBuf>,
}

/// A factory builds a strategy from its parameters (JSON) and the dependencies.
pub type Factory<T> =
    Arc<dyn Fn(&serde_json::Value, &StrategyDeps) -> Result<Arc<T>, StrategyError> + Send + Sync>;

/// What a context source sees of the turn.
pub struct TurnView<'a> {
    pub prompt: &'a str,
    pub turn: u32,
    pub history: &'a [Message],
    pub working_dir: &'a Path,
    pub budget_tokens: u32,
}

/// A block a source adds to the prompt.
#[derive(Debug, Clone)]
pub struct ContextBlock {
    pub slot: Slot,
    pub text: String,
    pub sources: Vec<ScoredSource>,
}

/// What a recaller is asked.
pub struct RecallQuery<'a> {
    pub text: &'a str,
    pub k: usize,
    pub exclude_kinds: &'a [MemoryKind],
    pub scope: Option<&'a str>,
}

/// A finished turn, for the memory writer.
pub struct TurnRecord {
    pub turn: u32,
    pub user_text: String,
    pub response_text: String,
}

#[async_trait]
pub trait ContextSource: Send + Sync {
    fn id(&self) -> &StrategyId;
    fn cadence(&self) -> Cadence;
    async fn gather(&self, view: &TurnView<'_>) -> Result<Vec<ContextBlock>, StrategyError>;
}

#[async_trait]
pub trait Recaller: Send + Sync {
    fn id(&self) -> &StrategyId;
    async fn recall(
        &self,
        store: &dyn MemoryStore,
        q: &RecallQuery<'_>,
    ) -> Result<Vec<ScoredMemory>, MemoryError>;
}

#[async_trait]
pub trait MemoryWriter: Send + Sync {
    fn id(&self) -> &StrategyId;
    async fn propose(&self, turn: &TurnRecord) -> Vec<MemoryCandidate>;
}

// ── Built-ins ─────────────────────────────────────────────────────────────────

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn id(slot: Slot, name: &str, params: &serde_json::Value) -> StrategyId {
    StrategyId {
        slot,
        name: name.to_owned(),
        version: VERSION.to_owned(),
        params_sha256: canonical_json_sha256(params),
    }
}

/// An `f32` as the `f64` of its shortest decimal form (0.4 stays 0.4).
pub(crate) fn f32_json(v: f32) -> serde_json::Value {
    format!("{v}")
        .parse::<f64>()
        .map_or(serde_json::Value::Null, |f| serde_json::json!(f))
}

/// The receipt parameters of the skeleton strategy.
pub(crate) fn skeleton_params(max_tokens: u32, gap_threshold: f32) -> serde_json::Value {
    serde_json::json!({ "gap_threshold": f32_json(gap_threshold), "max_tokens": max_tokens })
}

/// The receipt parameters of the built-in recaller.
pub(crate) fn recall_params(top_k: usize, exclude: &[MemoryKind]) -> serde_json::Value {
    let kinds: Vec<String> = exclude.iter().map(|k| kind_name(*k).to_owned()).collect();
    serde_json::json!({ "exclude": kinds, "limit": top_k })
}

/// The lowercase name of a memory kind, as in config files.
pub fn kind_name(kind: MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Conversation => "conversation",
        MemoryKind::User => "user",
        MemoryKind::Feedback => "feedback",
        MemoryKind::Project => "project",
        MemoryKind::Reference => "reference",
        MemoryKind::Outcome => "outcome",
    }
}

fn parse_kind(name: &str) -> Option<MemoryKind> {
    [
        MemoryKind::Conversation,
        MemoryKind::User,
        MemoryKind::Feedback,
        MemoryKind::Project,
        MemoryKind::Reference,
        MemoryKind::Outcome,
    ]
    .into_iter()
    .find(|k| kind_name(*k) == name.to_ascii_lowercase())
}

/// The built-in `skeleton` source: the repository skeleton and tier-2 hits.
struct SkeletonSource {
    id: StrategyId,
    strategy: SkeletonStrategy,
    max_tokens: u32,
    gap_threshold: f32,
    index: Option<Arc<SqliteIndex>>,
}

#[async_trait]
impl ContextSource for SkeletonSource {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    fn cadence(&self) -> Cadence {
        Cadence::Once
    }
    async fn gather(&self, view: &TurnView<'_>) -> Result<Vec<ContextBlock>, StrategyError> {
        let mut injector =
            ContextInjector::new(self.max_tokens, self.gap_threshold).with_strategy(self.strategy);
        let result = injector
            .inject(view.prompt, view.working_dir, self.index.as_deref())
            .await;
        if result.context_block.is_empty() {
            return Ok(vec![]);
        }
        let sources = result
            .bm25_hits
            .iter()
            .map(|(path, score)| ScoredSource {
                source: SourceRef::File {
                    path: PathBuf::from(path),
                    sha256: None,
                    lines: None,
                },
                score: Some(*score),
            })
            .collect();
        Ok(vec![ContextBlock {
            slot: Slot::Injection,
            text: result.context_block,
            sources,
        }])
    }
}

/// A source the agent runtime applies itself (`memory-index`, `recall`,
/// `skills`): selecting it switches the behaviour on.
struct RuntimeSource {
    id: StrategyId,
}

#[async_trait]
impl ContextSource for RuntimeSource {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    fn cadence(&self) -> Cadence {
        Cadence::EachTurn
    }
    async fn gather(&self, _view: &TurnView<'_>) -> Result<Vec<ContextBlock>, StrategyError> {
        Ok(vec![])
    }
}

/// Today's recall: BM25 over the memory store, kind exclusions applied before
/// the cap.
struct Bm25Recaller {
    id: StrategyId,
}

#[async_trait]
impl Recaller for Bm25Recaller {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    async fn recall(
        &self,
        store: &dyn MemoryStore,
        q: &RecallQuery<'_>,
    ) -> Result<Vec<ScoredMemory>, MemoryError> {
        let query = MemoryQuery {
            text: q.text.to_owned(),
            limit: q.k,
            kind: None,
        };
        let found = store.recall_excluding(&query, q.exclude_kinds).await?;
        Ok(found
            .into_iter()
            .map(|memory| ScoredMemory { memory, score: 0.0 })
            .collect())
    }
}

/// Recall switched off.
struct NoRecaller {
    id: StrategyId,
}

#[async_trait]
impl Recaller for NoRecaller {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    async fn recall(
        &self,
        _store: &dyn MemoryStore,
        _q: &RecallQuery<'_>,
    ) -> Result<Vec<ScoredMemory>, MemoryError> {
        Ok(vec![])
    }
}

/// Today's writer: trigger phrases decide whether a turn is worth remembering.
struct TriggersWriter {
    id: StrategyId,
}

#[async_trait]
impl MemoryWriter for TriggersWriter {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    async fn propose(&self, turn: &TurnRecord) -> Vec<MemoryCandidate> {
        classify_turn(&turn.user_text, &turn.response_text)
            .into_iter()
            .collect()
    }
}

/// Memory writes switched off.
struct NoWriter {
    id: StrategyId,
}

#[async_trait]
impl MemoryWriter for NoWriter {
    fn id(&self) -> &StrategyId {
        &self.id
    }
    async fn propose(&self, _turn: &TurnRecord) -> Vec<MemoryCandidate> {
        Vec::new()
    }
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// Named factories per slot.
pub struct StrategyRegistry {
    sources: BTreeMap<String, (Origin, Factory<dyn ContextSource>)>,
    skeletons: BTreeMap<String, Origin>,
    compactors: BTreeMap<String, Origin>,
    recallers: BTreeMap<String, (Origin, Factory<dyn Recaller>)>,
    writers: BTreeMap<String, (Origin, Factory<dyn MemoryWriter>)>,
}

fn names<V>(m: &BTreeMap<String, V>) -> Vec<String> {
    m.keys().cloned().collect()
}

fn unknown(slot: &str, name: &str, valid: Vec<String>) -> StrategyError {
    StrategyError::Unknown {
        slot: slot.to_owned(),
        name: name.to_owned(),
        valid,
    }
}

impl StrategyRegistry {
    /// An empty registry.
    pub fn empty() -> Self {
        Self {
            sources: BTreeMap::new(),
            skeletons: BTreeMap::new(),
            compactors: BTreeMap::new(),
            recallers: BTreeMap::new(),
            writers: BTreeMap::new(),
        }
    }

    /// Every built-in.
    pub fn with_builtins() -> Self {
        let mut r = Self::empty();

        for name in ["memory-index", "recall", "skills"] {
            let n = name.to_owned();
            r.register_source(
                name,
                Origin::Builtin,
                Arc::new(move |_p, _d| {
                    Ok(Arc::new(RuntimeSource {
                        id: id(source_slot(&n), &n, &serde_json::json!({})),
                    }) as Arc<dyn ContextSource>)
                }),
            );
        }
        r.register_source(
            "skeleton",
            Origin::Builtin,
            Arc::new(|params, deps| {
                let max_tokens = params["max_tokens"].as_u64().unwrap_or(2000) as u32;
                let gap = params["gap_threshold"].as_f64().unwrap_or(0.4) as f32;
                let strategy = params["use"]
                    .as_str()
                    .and_then(SkeletonStrategy::from_str)
                    .unwrap_or_default();
                Ok(Arc::new(SkeletonSource {
                    id: id(
                        Slot::Injection,
                        &format!("skeleton:{}", strategy.as_str()),
                        &skeleton_params(max_tokens, gap),
                    ),
                    strategy,
                    max_tokens,
                    gap_threshold: gap,
                    index: deps.index.clone(),
                }) as Arc<dyn ContextSource>)
            }),
        );

        for name in hone_config::strategy::SKELETON_NAMES {
            r.skeletons.insert((*name).to_owned(), Origin::Builtin);
        }
        for name in hone_config::strategy::COMPACTOR_NAMES {
            r.compactors.insert((*name).to_owned(), Origin::Builtin);
        }

        r.register_recaller(
            "bm25",
            Origin::Builtin,
            Arc::new(|params, _d| {
                let (k, exclude) = recall_args(params);
                Ok(Arc::new(Bm25Recaller {
                    id: id(Slot::Recall, "bm25", &recall_params(k, &exclude)),
                }) as Arc<dyn Recaller>)
            }),
        );
        r.register_recaller(
            "none",
            Origin::Builtin,
            Arc::new(|_p, _d| {
                Ok(Arc::new(NoRecaller {
                    id: id(Slot::Recall, "none", &serde_json::json!({})),
                }) as Arc<dyn Recaller>)
            }),
        );
        r.register_writer(
            "triggers",
            Origin::Builtin,
            Arc::new(|_p, _d| {
                Ok(Arc::new(TriggersWriter {
                    id: id(Slot::Recall, "triggers", &serde_json::json!({})),
                }) as Arc<dyn MemoryWriter>)
            }),
        );
        r.register_writer(
            "off",
            Origin::Builtin,
            Arc::new(|_p, _d| {
                Ok(Arc::new(NoWriter {
                    id: id(Slot::Recall, "off", &serde_json::json!({})),
                }) as Arc<dyn MemoryWriter>)
            }),
        );
        r
    }

    pub fn register_source(&mut self, name: &str, origin: Origin, f: Factory<dyn ContextSource>) {
        self.sources.insert(name.to_owned(), (origin, f));
    }

    pub fn register_recaller(&mut self, name: &str, origin: Origin, f: Factory<dyn Recaller>) {
        self.recallers.insert(name.to_owned(), (origin, f));
    }

    pub fn register_writer(&mut self, name: &str, origin: Origin, f: Factory<dyn MemoryWriter>) {
        self.writers.insert(name.to_owned(), (origin, f));
    }

    /// `(slot, name, origin)` for everything registered; `slot` is the config key.
    pub fn list(&self) -> Vec<(String, String, Origin)> {
        let mut out = Vec::new();
        for (n, (o, _)) in &self.sources {
            out.push(("context.sources".to_owned(), n.clone(), o.clone()));
        }
        for (n, o) in &self.skeletons {
            out.push(("context.skeleton".to_owned(), n.clone(), o.clone()));
        }
        for (n, o) in &self.compactors {
            out.push(("context.compactor".to_owned(), n.clone(), o.clone()));
        }
        for (n, (o, _)) in &self.recallers {
            out.push(("memory.recaller".to_owned(), n.clone(), o.clone()));
        }
        for (n, (o, _)) in &self.writers {
            out.push(("memory.writer".to_owned(), n.clone(), o.clone()));
        }
        out
    }

    /// Build the strategies a selection names. An unregistered name is an error
    /// naming it and listing the registered ones.
    pub fn build(
        &self,
        sel: &StrategySelection,
        deps: &StrategyDeps,
    ) -> Result<ResolvedStrategies, StrategyError> {
        let skeleton = SkeletonStrategy::from_str(&sel.skeleton.value)
            .filter(|_| self.skeletons.contains_key(&sel.skeleton.value))
            .ok_or_else(|| {
                unknown(
                    "context.skeleton",
                    &sel.skeleton.value,
                    names(&self.skeletons),
                )
            })?;
        if !self.compactors.contains_key(&sel.compactor.value) {
            return Err(unknown(
                "context.compactor",
                &sel.compactor.value,
                names(&self.compactors),
            ));
        }
        let exclude_kinds: Vec<MemoryKind> = sel
            .exclude_kinds
            .value
            .iter()
            .map(|k| {
                parse_kind(k).ok_or_else(|| StrategyError::Failed {
                    slot: "memory.exclude_kinds".to_owned(),
                    name: k.clone(),
                    reason: "not a memory kind".to_owned(),
                })
            })
            .collect::<Result<_, _>>()?;

        let source_params = |name: &str| match name {
            "skeleton" => serde_json::json!({
                "use": skeleton.as_str(),
                "max_tokens": sel.skeleton_max_tokens.value,
                "gap_threshold": f32_json(sel.skeleton_gap_threshold.value),
            }),
            _ => serde_json::json!({}),
        };
        let mut source_impls = Vec::new();
        for name in &sel.sources.value {
            let (_, factory) = self
                .sources
                .get(name)
                .ok_or_else(|| unknown("context.sources", name, names(&self.sources)))?;
            source_impls.push((name.clone(), factory(&source_params(name), deps)?));
        }

        let recall_args = serde_json::json!({
            "top_k": sel.top_k.value,
            "exclude_kinds": sel.exclude_kinds.value,
        });
        let (recaller_origin, recaller_factory) =
            self.recallers.get(&sel.recaller.value).ok_or_else(|| {
                unknown(
                    "memory.recaller",
                    &sel.recaller.value,
                    names(&self.recallers),
                )
            })?;
        let recaller = recaller_factory(&recall_args, deps)?;
        let (writer_origin, writer_factory) = self
            .writers
            .get(&sel.writer.value)
            .ok_or_else(|| unknown("memory.writer", &sel.writer.value, names(&self.writers)))?;
        let writer = writer_factory(&serde_json::json!({}), deps)?;

        let builtin = |o: &Origin| *o == Origin::Builtin;
        let recaller_kind = match (sel.recaller.value.as_str(), builtin(recaller_origin)) {
            ("bm25", true) => RecallerKind::Bm25,
            ("none", true) => RecallerKind::None,
            _ => RecallerKind::Custom,
        };
        let writer_kind = match (sel.writer.value.as_str(), builtin(writer_origin)) {
            ("triggers", true) => WriterKind::Triggers,
            ("off", true) => WriterKind::Off,
            _ => WriterKind::Custom,
        };

        Ok(ResolvedStrategies {
            sources: sel.sources.value.clone(),
            source_impls,
            skeleton,
            skeleton_max_tokens: sel.skeleton_max_tokens.value,
            skeleton_gap_threshold: sel.skeleton_gap_threshold.value,
            trim_only: sel.compactor.value == "trim-only",
            graduated_phase: sel.graduated_phase.value,
            budget_ratio: sel.budget_ratio.value,
            recaller,
            recaller_kind,
            top_k: sel.top_k.value,
            exclude_kinds,
            writer,
            writer_kind,
        })
    }
}

fn source_slot(name: &str) -> Slot {
    match name {
        "recall" => Slot::Recall,
        "skills" => Slot::Skills,
        "skeleton" => Slot::Injection,
        _ => Slot::SystemPrompt,
    }
}

fn recall_args(params: &serde_json::Value) -> (usize, Vec<MemoryKind>) {
    let k = params["top_k"].as_u64().unwrap_or(5) as usize;
    let kinds = params["exclude_kinds"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().and_then(parse_kind))
                .collect()
        })
        .unwrap_or_else(|| vec![MemoryKind::Outcome]);
    (k, kinds)
}

// ── Resolved strategies ───────────────────────────────────────────────────────

/// How the agent treats the recaller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecallerKind {
    /// The built-in BM25 recall (fused with semantic recall when an embedder runs).
    Bm25,
    /// Recall is off.
    None,
    /// A registered strategy: its results are used as they are.
    Custom,
}

/// How the agent treats the writer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriterKind {
    Triggers,
    Off,
    Custom,
}

/// The strategies an agent runs with.
pub struct ResolvedStrategies {
    sources: Vec<String>,
    #[allow(dead_code)] // gathered by callers that drive sources themselves
    source_impls: Vec<(String, Arc<dyn ContextSource>)>,
    pub(crate) skeleton: SkeletonStrategy,
    pub(crate) skeleton_max_tokens: u32,
    pub(crate) skeleton_gap_threshold: f32,
    pub(crate) trim_only: bool,
    pub(crate) graduated_phase: bool,
    pub(crate) budget_ratio: Option<f32>,
    pub(crate) recaller: Arc<dyn Recaller>,
    pub(crate) recaller_kind: RecallerKind,
    pub(crate) top_k: usize,
    pub(crate) exclude_kinds: Vec<MemoryKind>,
    pub(crate) writer: Arc<dyn MemoryWriter>,
    pub(crate) writer_kind: WriterKind,
}

impl ResolvedStrategies {
    /// Whether the source `name` is on.
    pub fn has_source(&self, name: &str) -> bool {
        self.sources.iter().any(|s| s == name)
    }

    /// The strategies of an agent built without `with_strategies`: today's
    /// behaviour, from the existing `inject_context` and `skeleton_strategy`
    /// fields.
    pub(crate) fn derived(inject_context: bool, skeleton: SkeletonStrategy) -> Self {
        let mut sources: Vec<String> = ["memory-index", "recall", "skills"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        if inject_context {
            sources.push("skeleton".to_owned());
        }
        let exclude = vec![MemoryKind::Outcome];
        Self {
            sources,
            source_impls: Vec::new(),
            skeleton,
            skeleton_max_tokens: 2000,
            skeleton_gap_threshold: 0.4,
            trim_only: false,
            graduated_phase: false,
            budget_ratio: None,
            recaller: Arc::new(Bm25Recaller {
                id: id(Slot::Recall, "bm25", &recall_params(5, &exclude)),
            }),
            recaller_kind: RecallerKind::Bm25,
            top_k: 5,
            exclude_kinds: exclude,
            writer: Arc::new(TriggersWriter {
                id: id(Slot::Recall, "triggers", &serde_json::json!({})),
            }),
            writer_kind: WriterKind::Triggers,
        }
    }
}
