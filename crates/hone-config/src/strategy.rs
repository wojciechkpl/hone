//! The `[context]` and `[memory]` tables as plain data, and the selection of
//! context strategies from them.
//!
//! The tables only name strategies and their parameters; this crate does not
//! know the registry. Precedence is flag, then environment variable, then the
//! project file, then the user file, then the default. The project file comes
//! with a repository and is untrusted: it can only narrow (fewer sources, lower
//! numbers, features switched off), never widen.
//!
//! See `docs/plans/strategy-registry-spec.md`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::resolve::{ConfigLayer, Warning};
use crate::types::ConfigError;

// ── The tables ────────────────────────────────────────────────────────────────

/// The `[context]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct ContextTable {
    /// Which context sources are on: `memory-index`, `recall`, `skills`, `skeleton`.
    pub sources: Option<Vec<String>>,
    /// `graduated` or `trim-only`.
    pub compactor: Option<String>,
    /// Fraction of the model's context window to use; `None` = the per-model table.
    pub budget_ratio: Option<f32>,
    /// `[context.skeleton]`.
    pub skeleton: Option<SkeletonTable>,
    /// `[context.graduated]`.
    pub graduated: Option<GraduatedTable>,
    /// `[context.env-snapshot]`.
    #[serde(rename = "env-snapshot")]
    pub env_snapshot: Option<EnvSnapshotTable>,
}

/// The `[context.env-snapshot]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct EnvSnapshotTable {
    /// Seconds the whole snapshot may take (1–60, default 15).
    pub timeout_secs: Option<u64>,
}

/// The `[context.skeleton]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SkeletonTable {
    /// Which skeleton: `adaptive`, `flat`, `dependency-graph`, `call-graph`, `hybrid`.
    #[serde(rename = "use")]
    pub use_: Option<String>,
    pub max_tokens: Option<u32>,
    pub gap_threshold: Option<f32>,
}

/// The `[context.graduated]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GraduatedTable {
    /// Phase compaction (off by default).
    pub phase: Option<bool>,
}

/// The `[memory]` table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct MemoryTable {
    /// `bm25`, `fts5` or `none`.
    pub recaller: Option<String>,
    /// `triggers` or `off`.
    pub writer: Option<String>,
    pub top_k: Option<usize>,
    pub exclude_kinds: Option<Vec<String>>,
}

// ── Names ─────────────────────────────────────────────────────────────────────

/// Valid `[context] sources` names.
pub const SOURCE_NAMES: &[&str] = &[
    "memory-index",
    "recall",
    "skills",
    "skeleton",
    "env-snapshot",
];
/// The sources that are on unless a file says otherwise (`env-snapshot` is off).
pub const DEFAULT_SOURCES: &[&str] = &["memory-index", "recall", "skills", "skeleton"];
/// The default `[context.env-snapshot] timeout_secs`.
pub const DEFAULT_ENV_TIMEOUT_SECS: u64 = 15;
/// The accepted range of `[context.env-snapshot] timeout_secs`.
pub const ENV_TIMEOUT_RANGE: std::ops::RangeInclusive<u64> = 1..=60;
/// Valid `[context.skeleton] use` names.
pub const SKELETON_NAMES: &[&str] = &[
    "adaptive",
    "flat",
    "dependency-graph",
    "call-graph",
    "hybrid",
];
/// Valid `[context] compactor` names.
pub const COMPACTOR_NAMES: &[&str] = &["graduated", "trim-only"];
/// Valid `[memory] recaller` names.
pub const RECALLER_NAMES: &[&str] = &["bm25", "fts5", "none"];
/// Valid `[memory] writer` names.
pub const WRITER_NAMES: &[&str] = &["triggers", "off", "tool", "triggers+tool"];

/// Valid `[memory] exclude_kinds` entries (any case).
pub const KIND_NAMES: &[&str] = &[
    "conversation",
    "user",
    "feedback",
    "project",
    "reference",
    "outcome",
];

/// Whether `HONE_PHASE_COMPACTION` turns phase compaction on: trimmed,
/// case-insensitive `1`, `true` or `on`. The one parser, used by the selection
/// and by agents built without one.
pub fn phase_env_enabled(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "on"
    )
}

/// The default skeleton token budget.
pub const DEFAULT_MAX_TOKENS: u32 = 2000;
/// The default tier-2 score-gap threshold.
pub const DEFAULT_GAP_THRESHOLD: f32 = 0.4;
/// The default number of memories recalled.
pub const DEFAULT_TOP_K: usize = 5;

/// The largest per-model budget ratio (see `ContextConfig::for_model`): a
/// project value above it cannot narrow the default.
const MAX_MODEL_BUDGET_RATIO: f32 = 0.60;

// ── Origin and Chosen ─────────────────────────────────────────────────────────

/// What chose a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    Flag,
    Env(String),
    File(PathBuf),
    Default,
}

impl Origin {
    /// `default`, `flag`, `env:<NAME>` or `file:<path>`.
    pub fn label(&self) -> String {
        match self {
            Origin::Flag => "flag".to_owned(),
            Origin::Env(n) => format!("env:{n}"),
            Origin::File(p) => format!("file:{}", p.display()),
            Origin::Default => "default".to_owned(),
        }
    }
}

/// A value and what chose it.
#[derive(Debug, Clone, PartialEq)]
pub struct Chosen<T> {
    pub value: T,
    pub origin: Origin,
}

impl<T> Chosen<T> {
    fn default_of(value: T) -> Self {
        Self {
            value,
            origin: Origin::Default,
        }
    }
}

/// Command-line inputs to the selection.
#[derive(Debug, Clone, Default)]
pub struct Flags {
    /// `--skeleton-strategy`.
    pub skeleton_strategy: Option<String>,
    /// `--no-inject`.
    pub no_inject: bool,
    /// `honed`: the default sources omit `skeleton`.
    pub daemon: bool,
    /// `--encrypted-memory`: FTS5 cannot index ciphertext, so `recaller =
    /// "fts5"` falls back to `bm25` with a warning.
    pub encrypted_memory: bool,
}

// ── The selection ─────────────────────────────────────────────────────────────

/// The chosen strategies and parameters, with what chose each.
#[derive(Debug, Clone)]
pub struct StrategySelection {
    pub sources: Chosen<Vec<String>>,
    pub skeleton: Chosen<String>,
    pub skeleton_max_tokens: Chosen<u32>,
    pub skeleton_gap_threshold: Chosen<f32>,
    pub compactor: Chosen<String>,
    pub graduated_phase: Chosen<bool>,
    /// `[context.env-snapshot] timeout_secs`.
    pub env_snapshot_timeout_secs: Chosen<u64>,
    /// `None` = the per-model table.
    pub budget_ratio: Chosen<Option<f32>>,
    /// A project file's `budget_ratio` when the user set none: a cap, applied
    /// as `min(per-model default, cap)` where the model is known.
    pub budget_cap: Chosen<Option<f32>>,
    pub recaller: Chosen<String>,
    pub top_k: Chosen<usize>,
    pub exclude_kinds: Chosen<Vec<String>>,
    pub writer: Chosen<String>,
    /// Values of the project file that were ignored.
    pub warnings: Vec<Warning>,
}

fn invalid(path: impl Into<PathBuf>, reason: String) -> ConfigError {
    ConfigError::InvalidField {
        path: path.into(),
        reason,
    }
}

/// Longest part of an invalid value that is echoed in an error.
const MAX_ECHO_CHARS: usize = 80;

/// An invalid value as it may appear in a message: cut to 80 characters and
/// escaped (`{:?}` style: control characters become `\u{..}`), so a file, an
/// environment variable or a flag cannot write terminal escapes or forge log
/// lines through an error.
fn show(value: &str) -> String {
    let cut: String = value.chars().take(MAX_ECHO_CHARS).collect();
    let mut shown = format!("{cut:?}");
    if value.chars().count() > MAX_ECHO_CHARS {
        shown.push('…');
    }
    shown
}

fn unknown(key: &str, got: &str, valid: &[&str]) -> String {
    format!(
        "{key} = {} is not a known strategy; valid names: {}",
        show(got),
        valid.join(", ")
    )
}

/// `deps` -> `dependency-graph`, `call_graph` -> `call-graph`, case-insensitive:
/// the spellings `SkeletonStrategy::from_str` has always accepted.
fn canonical_skeleton(raw: &str) -> Option<&'static str> {
    match raw.to_ascii_lowercase().replace('_', "-").as_str() {
        "flat" => Some("flat"),
        "dependency-graph" | "deps" => Some("dependency-graph"),
        "call-graph" | "calls" => Some("call-graph"),
        "hybrid" => Some("hybrid"),
        "adaptive" => Some("adaptive"),
        _ => None,
    }
}

fn check_one(path: &Path, key: &str, got: &str, valid: &[&str]) -> Result<String, ConfigError> {
    if valid.contains(&got) {
        Ok(got.to_owned())
    } else {
        Err(invalid(path, unknown(key, got, valid)))
    }
}

/// Validate one layer's names and ranges (any file, trusted or not).
fn validate(layer: &ConfigLayer) -> Result<(), ConfigError> {
    let path = &layer.path;
    let cfg = &layer.config;
    if let Some(c) = &cfg.context {
        for s in c.sources.iter().flatten() {
            check_one(path, "context.sources", s, SOURCE_NAMES)?;
        }
        if let Some(n) = &c.compactor {
            check_one(path, "context.compactor", n, COMPACTOR_NAMES)?;
        }
        if let Some(r) = c.budget_ratio {
            if !(r > 0.0 && r <= 1.0) {
                return Err(invalid(
                    path,
                    format!("context.budget_ratio = {r} must be above 0 and at most 1"),
                ));
            }
        }
        if let Some(u) = c.skeleton.as_ref().and_then(|s| s.use_.as_deref()) {
            if canonical_skeleton(u).is_none() {
                return Err(invalid(
                    path,
                    unknown("context.skeleton.use", u, SKELETON_NAMES),
                ));
            }
        }
        if let Some(t) = c.env_snapshot.as_ref().and_then(|e| e.timeout_secs) {
            if !ENV_TIMEOUT_RANGE.contains(&t) {
                return Err(invalid(
                    path,
                    format!(
                        "context.env-snapshot.timeout_secs = {t} must be between {} and {}",
                        ENV_TIMEOUT_RANGE.start(),
                        ENV_TIMEOUT_RANGE.end()
                    ),
                ));
            }
        }
        if let Some(g) = c.skeleton.as_ref().and_then(|s| s.gap_threshold) {
            if !(0.0..=1.0).contains(&g) {
                return Err(invalid(
                    path,
                    format!("context.skeleton.gap_threshold = {g} must be between 0 and 1"),
                ));
            }
        }
    }
    if let Some(m) = &cfg.memory {
        if let Some(n) = &m.recaller {
            check_one(path, "memory.recaller", n, RECALLER_NAMES)?;
        }
        if let Some(n) = &m.writer {
            check_one(path, "memory.writer", n, WRITER_NAMES)?;
        }
        if m.top_k == Some(0) {
            return Err(invalid(
                path,
                "memory.top_k = 0 must be at least 1 (use recaller = \"none\" to turn recall off)"
                    .to_owned(),
            ));
        }
        for kind in m.exclude_kinds.iter().flatten() {
            if !KIND_NAMES.contains(&kind.to_ascii_lowercase().as_str()) {
                return Err(invalid(
                    path,
                    format!(
                        "memory.exclude_kinds has {} which is not a memory kind; valid kinds: {}",
                        show(kind),
                        KIND_NAMES.join(", ")
                    ),
                ));
            }
        }
    }
    Ok(())
}

impl StrategySelection {
    /// Resolve the selection. Precedence: flag, environment variable, project
    /// file, user file, default. The project layer is untrusted and may only
    /// narrow; what it asks beyond that is ignored with a warning naming the
    /// file. An unknown name is an error: for a file `path` is that file, for an
    /// environment variable or flag it is the variable name or flag.
    pub fn from_sources(
        user: Option<&ConfigLayer>,
        project: Option<&ConfigLayer>,
        env: &dyn Fn(&str) -> Option<String>,
        flags: &Flags,
    ) -> Result<StrategySelection, ConfigError> {
        for layer in [user, project].into_iter().flatten() {
            validate(layer)?;
        }

        let default_sources: Vec<String> = DEFAULT_SOURCES
            .iter()
            .filter(|n| !(flags.daemon && **n == "skeleton"))
            .map(|s| (*s).to_owned())
            .collect();
        let mut sel = StrategySelection {
            sources: Chosen::default_of(default_sources),
            skeleton: Chosen::default_of("adaptive".to_owned()),
            skeleton_max_tokens: Chosen::default_of(DEFAULT_MAX_TOKENS),
            skeleton_gap_threshold: Chosen::default_of(DEFAULT_GAP_THRESHOLD),
            compactor: Chosen::default_of("graduated".to_owned()),
            graduated_phase: Chosen::default_of(false),
            env_snapshot_timeout_secs: Chosen::default_of(DEFAULT_ENV_TIMEOUT_SECS),
            budget_ratio: Chosen::default_of(None),
            budget_cap: Chosen::default_of(None),
            recaller: Chosen::default_of("bm25".to_owned()),
            top_k: Chosen::default_of(DEFAULT_TOP_K),
            exclude_kinds: Chosen::default_of(vec!["outcome".to_owned()]),
            writer: Chosen::default_of("triggers".to_owned()),
            warnings: Vec::new(),
        };

        if let Some(u) = user {
            sel.apply_trusted(u);
        }
        if let Some(p) = project {
            sel.apply_project(p);
        }
        sel.apply_env_and_flags(env, flags)?;
        if flags.encrypted_memory && sel.recaller.value == "fts5" {
            sel.recaller = Chosen {
                value: "bm25".to_owned(),
                origin: Origin::Flag,
            };
            sel.warnings.push(Warning {
                path: PathBuf::from("--encrypted-memory"),
                message: "recaller \"fts5\" cannot index encrypted memory; using \"bm25\" instead"
                    .to_owned(),
            });
        }
        Ok(sel)
    }

    /// Apply `layer` as a trusted file: any value stands, including widening
    /// ones. For the evaluator's own profiles (`hone eval`), which are written by
    /// the person running the evaluation. The layer's names are validated first.
    pub fn with_trusted_layer(mut self, layer: &ConfigLayer) -> Result<Self, ConfigError> {
        validate(layer)?;
        self.apply_trusted(layer);
        Ok(self)
    }

    /// The user file: any value stands.
    fn apply_trusted(&mut self, layer: &ConfigLayer) {
        let origin = || Origin::File(layer.path.clone());
        if let Some(c) = &layer.config.context {
            if let Some(s) = &c.sources {
                self.sources = Chosen {
                    value: s.clone(),
                    origin: origin(),
                };
            }
            if let Some(n) = &c.compactor {
                self.compactor = Chosen {
                    value: n.clone(),
                    origin: origin(),
                };
            }
            if let Some(r) = c.budget_ratio {
                self.budget_ratio = Chosen {
                    value: Some(r),
                    origin: origin(),
                };
            }
            if let Some(s) = &c.skeleton {
                if let Some(u) = s.use_.as_deref().and_then(canonical_skeleton) {
                    self.skeleton = Chosen {
                        value: u.to_owned(),
                        origin: origin(),
                    };
                }
                if let Some(t) = s.max_tokens {
                    self.skeleton_max_tokens = Chosen {
                        value: t,
                        origin: origin(),
                    };
                }
                if let Some(g) = s.gap_threshold {
                    self.skeleton_gap_threshold = Chosen {
                        value: g,
                        origin: origin(),
                    };
                }
            }
            if let Some(p) = c.graduated.as_ref().and_then(|g| g.phase) {
                self.graduated_phase = Chosen {
                    value: p,
                    origin: origin(),
                };
            }
            if let Some(t) = c.env_snapshot.as_ref().and_then(|e| e.timeout_secs) {
                self.env_snapshot_timeout_secs = Chosen {
                    value: t,
                    origin: origin(),
                };
            }
        }
        if let Some(m) = &layer.config.memory {
            if let Some(n) = &m.recaller {
                self.recaller = Chosen {
                    value: n.clone(),
                    origin: origin(),
                };
            }
            if let Some(n) = &m.writer {
                self.writer = Chosen {
                    value: n.clone(),
                    origin: origin(),
                };
            }
            if let Some(k) = m.top_k {
                self.top_k = Chosen {
                    value: k,
                    origin: origin(),
                };
            }
            if let Some(kinds) = &m.exclude_kinds {
                self.exclude_kinds = Chosen {
                    value: kinds.clone(),
                    origin: origin(),
                };
            }
        }
    }

    /// The project file: narrowing only.
    fn apply_project(&mut self, layer: &ConfigLayer) {
        let path = layer.path.clone();
        let origin = || Origin::File(path.clone());
        let mut warnings: Vec<String> = Vec::new();

        if let Some(c) = &layer.config.context {
            if let Some(s) = &c.sources {
                let dropped: Vec<&String> = s
                    .iter()
                    .filter(|n| !self.sources.value.contains(n))
                    .collect();
                if !dropped.is_empty() {
                    let names: Vec<&str> = dropped.iter().map(|n| n.as_str()).collect();
                    warnings.push(format!(
                        "context.sources: {} not in the sources the user has on; ignored (a project file can only narrow the list)",
                        names.join(", ")
                    ));
                }
                let kept: Vec<String> = self
                    .sources
                    .value
                    .iter()
                    .filter(|n| s.contains(n))
                    .cloned()
                    .collect();
                self.sources = Chosen {
                    value: kept,
                    origin: origin(),
                };
            }
            if let Some(n) = &c.compactor {
                self.compactor = Chosen {
                    value: n.clone(),
                    origin: origin(),
                };
            }
            if let Some(r) = c.budget_ratio {
                match self.budget_ratio.value {
                    // The user chose a ratio: a project can only lower it.
                    Some(cur) if r < cur => {
                        self.budget_ratio = Chosen {
                            value: Some(r),
                            origin: origin(),
                        };
                    }
                    Some(cur) if r > cur => warnings.push(format!(
                        "context.budget_ratio = {r} is higher than the current limit; ignored (a project file can only lower it)"
                    )),
                    Some(_) => {}
                    // No user value: the per-model default is not known here, so
                    // the project value is a cap, applied where the model is.
                    None if r <= MAX_MODEL_BUDGET_RATIO => {
                        self.budget_cap = Chosen {
                            value: Some(r),
                            origin: origin(),
                        };
                    }
                    None => warnings.push(format!(
                        "context.budget_ratio = {r} is higher than any model's default; ignored (a project file can only lower the budget)"
                    )),
                }
            }
            if let Some(s) = &c.skeleton {
                if let Some(u) = s.use_.as_deref().and_then(canonical_skeleton) {
                    self.skeleton = Chosen {
                        value: u.to_owned(),
                        origin: origin(),
                    };
                }
                if let Some(t) = s.max_tokens {
                    if t < self.skeleton_max_tokens.value {
                        self.skeleton_max_tokens = Chosen {
                            value: t,
                            origin: origin(),
                        };
                    } else if t > self.skeleton_max_tokens.value {
                        warnings.push(format!(
                            "context.skeleton.max_tokens = {t} is higher than the current limit; ignored (a project file can only lower it)"
                        ));
                    }
                }
                if let Some(g) = s.gap_threshold {
                    if g > self.skeleton_gap_threshold.value {
                        self.skeleton_gap_threshold = Chosen {
                            value: g,
                            origin: origin(),
                        };
                    } else if g < self.skeleton_gap_threshold.value {
                        warnings.push(format!(
                            "context.skeleton.gap_threshold = {g} is lower than the current value; ignored (a project file can only raise it)"
                        ));
                    }
                }
            }
            if let Some(t) = c.env_snapshot.as_ref().and_then(|e| e.timeout_secs) {
                if t < self.env_snapshot_timeout_secs.value {
                    self.env_snapshot_timeout_secs = Chosen {
                        value: t,
                        origin: origin(),
                    };
                } else if t > self.env_snapshot_timeout_secs.value {
                    warnings.push(format!(
                        "context.env-snapshot.timeout_secs = {t} is higher than the current limit; ignored (a project file can only lower it)"
                    ));
                }
            }
            if let Some(p) = c.graduated.as_ref().and_then(|g| g.phase) {
                if !p {
                    self.graduated_phase = Chosen {
                        value: false,
                        origin: origin(),
                    };
                } else {
                    warnings.push(
                        "context.graduated.phase = true ignored: a project file cannot turn on a lossy compaction"
                            .to_owned(),
                    );
                }
            }
        }
        if let Some(m) = &layer.config.memory {
            if let Some(n) = &m.recaller {
                if n == "none" {
                    self.recaller = Chosen {
                        value: n.clone(),
                        origin: origin(),
                    };
                } else {
                    warnings.push(format!(
                        "memory.recaller = {} ignored: a project file can only turn recall off (\"none\")",
                        show(n)
                    ));
                }
            }
            if let Some(n) = &m.writer {
                if n == "off" {
                    self.writer = Chosen {
                        value: n.clone(),
                        origin: origin(),
                    };
                } else {
                    warnings.push(format!(
                        "memory.writer = {} ignored: a project file can only turn the writer off (\"off\")",
                        show(n)
                    ));
                }
            }
            if let Some(k) = m.top_k {
                if k < self.top_k.value {
                    self.top_k = Chosen {
                        value: k,
                        origin: origin(),
                    };
                } else if k > self.top_k.value {
                    warnings.push(format!(
                        "memory.top_k = {k} is higher than the current limit; ignored (a project file can only lower it)"
                    ));
                }
            }
            if let Some(kinds) = &m.exclude_kinds {
                // Excluding more is narrowing; a project cannot let a kind back in.
                let mut merged = self.exclude_kinds.value.clone();
                for k in kinds {
                    if !merged.contains(k) {
                        merged.push(k.clone());
                    }
                }
                if merged != self.exclude_kinds.value {
                    self.exclude_kinds = Chosen {
                        value: merged,
                        origin: origin(),
                    };
                }
            }
        }
        for message in warnings {
            self.warnings.push(Warning {
                path: path.clone(),
                message,
            });
        }
    }

    fn apply_env_and_flags(
        &mut self,
        env: &dyn Fn(&str) -> Option<String>,
        flags: &Flags,
    ) -> Result<(), ConfigError> {
        // Skeleton: flag over env var.
        if let Some(raw) = &flags.skeleton_strategy {
            let name = canonical_skeleton(raw).ok_or_else(|| {
                invalid(
                    "--skeleton-strategy",
                    unknown("--skeleton-strategy", raw, SKELETON_NAMES),
                )
            })?;
            self.skeleton = Chosen {
                value: name.to_owned(),
                origin: Origin::Flag,
            };
        } else if let Some(raw) = env("HONE_SKELETON_STRATEGY").filter(|v| !v.is_empty()) {
            let name = canonical_skeleton(&raw).ok_or_else(|| {
                invalid(
                    "HONE_SKELETON_STRATEGY",
                    unknown("HONE_SKELETON_STRATEGY", &raw, SKELETON_NAMES),
                )
            })?;
            self.skeleton = Chosen {
                value: name.to_owned(),
                origin: Origin::Env("HONE_SKELETON_STRATEGY".to_owned()),
            };
        }

        // Injection off: the flag, else HONE_NO_INJECTION=1.
        let off_origin = if flags.no_inject {
            Some(Origin::Flag)
        } else if env("HONE_NO_INJECTION").as_deref() == Some("1") {
            Some(Origin::Env("HONE_NO_INJECTION".to_owned()))
        } else {
            None
        };
        if let Some(origin) = off_origin {
            self.sources.value.retain(|n| n != "skeleton");
            self.sources.origin = origin;
        }

        if let Some(raw) = env("HONE_PHASE_COMPACTION") {
            self.graduated_phase = Chosen {
                value: phase_env_enabled(&raw),
                origin: Origin::Env("HONE_PHASE_COMPACTION".to_owned()),
            };
        }
        Ok(())
    }
}
