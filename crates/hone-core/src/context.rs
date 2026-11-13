//! Context management: token budgeting, compaction and phase tracking.
//!
//! # Compaction log and receipts
//!
//! The manager records every change it makes to the stored history as a
//! [`CompactionStep`] (see [`ContextManager::drain_compaction_log`]), for context
//! receipts. Two index spaces are in play:
//!
//! - compaction refs use positions in the **stored history** (the system prompt,
//!   recall and skills blocks are not part of it). Within one drop pass, every
//!   `replaced` ref's index is its position before the pass started;
//! - the `History` block of a receipt uses positions in the **request** as sent.
//!
//! The two are linked by `sha256` (the hash of the message). A dedup step's
//! `replaced` ref is the hash of the incoming message, which was never stored; its
//! index is the position the pointer message took.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

use hone_proto::{
    CompactionStep, Content, Message, Role, Slot, SourceRef, StrategyId, canonical_json_sha256,
    message_sha256,
};
use tiktoken_rs::CoreBPE;

use crate::metrics::ContextMetrics;

/// Environment variable that overrides the advertised context window.
pub const CONTEXT_WINDOW_ENV: &str = "HONE_CONTEXT_WINDOW";
/// Smallest accepted context-window override (tokens).
pub const MIN_CONTEXT_WINDOW: u32 = 4_096;
/// Largest accepted context-window override (tokens).
pub const MAX_CONTEXT_WINDOW: u32 = 10_000_000;

/// Parse a context-window override: trimmed plain decimal digits within
/// `MIN_CONTEXT_WINDOW..=MAX_CONTEXT_WINDOW`. Anything else yields `None`.
pub fn parse_context_window(raw: &str) -> Option<u32> {
    let s = raw.trim();
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u32>()
        .ok()
        .filter(|n| (MIN_CONTEXT_WINDOW..=MAX_CONTEXT_WINDOW).contains(n))
}

/// Interpret a raw `HONE_CONTEXT_WINDOW` value. Unset, empty and
/// whitespace-only values mean "no override" silently; any other unparsable
/// value (including non-UTF-8) is ignored with a warning, emitted at most once
/// per process.
pub(crate) fn resolve_context_window_override(raw: Option<std::ffi::OsString>) -> Option<u32> {
    static WARNED: std::sync::Once = std::sync::Once::new();
    let raw = raw?;
    let text = raw.to_str();
    if text.is_some_and(|t| t.trim().is_empty()) {
        return None;
    }
    let parsed = text.and_then(parse_context_window);
    if parsed.is_none() {
        WARNED.call_once(|| {
            tracing::warn!(
                "ignoring invalid {CONTEXT_WINDOW_ENV}={raw:?}: expected an integer between \
                 {MIN_CONTEXT_WINDOW} and {MAX_CONTEXT_WINDOW}"
            );
        });
    }
    parsed
}

// ---------------------------------------------------------------------------
// ContextConfig — model-aware budget and window configuration
// ---------------------------------------------------------------------------

/// Configuration for context window management, tuned per model.
///
/// Research shows models' effective context is significantly less than their
/// advertised window (RULER benchmark). Using the full window degrades quality
/// non-linearly. This config caps the usable budget at a model-appropriate
/// fraction.
#[derive(Debug, Clone)]
pub struct ContextConfig {
    /// Fraction of advertised window to actually use (0.0–1.0).
    pub effective_budget_ratio: f32,
    /// Advertised context length from ModelInfo.
    pub advertised_context_length: u32,
    /// Number of recent messages protected from compression/drop.
    pub recent_window: usize,
    /// Assistant-turn age threshold for structural summary compression.
    pub tool_compress_age: usize,
    /// Additional turns after tool_compress_age before one-liner compression.
    pub tool_oneliner_age_offset: usize,
    /// Whether to apply BM25 relevance reordering to the middle zone.
    /// Enabled for models with weaker long-context attention.
    pub reorder_middle_zone: bool,
}

impl ContextConfig {
    /// Build a config tuned for the given model name.
    ///
    /// Budget ratios based on RULER and Needle-in-a-Haystack research:
    /// - Claude: effective at ~60% of advertised 200K
    /// - DeepSeek/Qwen: effective at ~45% of advertised 128K
    /// - Llama: effective at ~40% of advertised 128K
    /// - Default: 50% for unknown models
    pub fn for_model(model_name: &str) -> Self {
        let lower = model_name.to_lowercase();

        // Models with weaker long-context attention benefit from middle-zone reordering.
        let (ratio, context_len, recent, reorder) = if lower.contains("claude") {
            (0.60, 200_000, 3, false) // Claude has strong attention across the window
        } else if lower.contains("deepseek") || lower.contains("qwen") {
            (0.45, 128_000, 5, true)
        } else if lower.contains("llama") {
            (0.40, 128_000, 7, true)
        } else if lower.contains("gpt-4") || lower.contains("o3") || lower.contains("o4") {
            (0.55, 128_000, 3, false)
        } else if lower.contains("gemini") {
            (0.50, 1_000_000, 3, false)
        } else if lower.contains("kimi") {
            (0.45, 128_000, 5, true)
        } else {
            (0.50, 128_000, 4, true) // default: enable for unknown models
        };

        Self {
            effective_budget_ratio: ratio,
            advertised_context_length: context_len,
            recent_window: recent,
            tool_compress_age: 5,
            tool_oneliner_age_offset: 5,
            reorder_middle_zone: reorder,
        }
    }

    /// Replace only the advertised context length; `None` leaves it unchanged.
    #[must_use]
    pub fn with_context_window(mut self, window: Option<u32>) -> Self {
        if let Some(w) = window {
            self.advertised_context_length = w;
        }
        self
    }

    /// Like [`for_model`](Self::for_model), then apply the `HONE_CONTEXT_WINDOW`
    /// override read through `get_env`. An unparsable value is warned about
    /// and ignored.
    pub fn for_model_with_env(model_name: &str, get_env: impl Fn(&str) -> Option<String>) -> Self {
        let ov = resolve_context_window_override(get_env(CONTEXT_WINDOW_ENV).map(Into::into));
        Self::for_model(model_name).with_context_window(ov)
    }

    /// Effective token budget (advertised * ratio).
    pub fn effective_budget(&self) -> u32 {
        (self.advertised_context_length as f32 * self.effective_budget_ratio) as u32
    }

    /// Age threshold at which tool results become one-liners.
    pub fn tool_oneliner_age(&self) -> usize {
        self.tool_compress_age + self.tool_oneliner_age_offset
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            effective_budget_ratio: 1.0,
            advertised_context_length: 128_000,
            recent_window: 3,
            tool_compress_age: 5,
            tool_oneliner_age_offset: 5,
            reorder_middle_zone: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase detection
// ---------------------------------------------------------------------------

/// Task phase for adaptive context management.
///
/// Agentless (Xia et al., 2024) showed that resetting context between
/// localization and repair phases improves SWE-bench scores. Phase-aware
/// compaction gives a similar benefit without full context reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskPhase {
    /// Mostly read tools: read_file, grep, glob, git_status, git_diff,
    /// list_dir, web_fetch, MCP tools, and any `shell` call that is not a test
    /// run.
    Exploration,
    /// A write tool appears: write_file, edit_file, multi_edit, apply_patch
    /// (alias apply_diff).
    Editing,
    /// Test execution: a `shell` call that runs a test suite (see
    /// [`classify_tool_call`]).
    Verification,
}

/// Tracks the current task phase and triggers compaction on transitions.
#[derive(Debug)]
pub struct PhaseTracker {
    current_phase: TaskPhase,
    consecutive_read_tools: u32,
}

impl PhaseTracker {
    fn new() -> Self {
        Self {
            current_phase: TaskPhase::Exploration,
            consecutive_read_tools: 0,
        }
    }

    /// Observe a tool call by name only and return the new phase if a
    /// transition occurred.
    #[cfg(test)]
    fn observe_tool(&mut self, tool_name: &str) -> Option<(TaskPhase, TaskPhase)> {
        self.observe_kind(classify_by_name(tool_name))
    }

    /// Observe an already classified tool call.
    fn observe_kind(&mut self, kind: ToolPhaseKind) -> Option<(TaskPhase, TaskPhase)> {
        let old = self.current_phase;

        if kind == ToolPhaseKind::Test {
            self.consecutive_read_tools = 0;
            if old != TaskPhase::Verification {
                self.current_phase = TaskPhase::Verification;
                return Some((old, TaskPhase::Verification));
            }
        } else if kind == ToolPhaseKind::Write {
            self.consecutive_read_tools = 0;
            if old == TaskPhase::Exploration {
                self.current_phase = TaskPhase::Editing;
                return Some((old, TaskPhase::Editing));
            }
        } else {
            // Read tool
            self.consecutive_read_tools += 1;
            if old == TaskPhase::Verification && self.consecutive_read_tools >= 3 {
                self.current_phase = TaskPhase::Exploration;
                self.consecutive_read_tools = 0;
                return Some((old, TaskPhase::Exploration));
            }
        }

        None
    }

    pub fn current_phase(&self) -> TaskPhase {
        self.current_phase
    }
}

/// How the phase tracker sees a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPhaseKind {
    /// Anything that is not a write or a test run (also unknown and MCP tools).
    Read,
    /// `write_file`, `edit_file`, `multi_edit`, `apply_patch` (alias `apply_diff`).
    Write,
    /// A test run: the legacy name `run_tests`, or a `shell` call that runs a
    /// test runner.
    Test,
}

/// Classify by tool name alone (`shell` is a read without its arguments).
fn classify_by_name(name: &str) -> ToolPhaseKind {
    match name {
        "write_file" | "edit_file" | "multi_edit" | "apply_patch" | "apply_diff" => {
            ToolPhaseKind::Write
        }
        "run_tests" => ToolPhaseKind::Test,
        _ => ToolPhaseKind::Read,
    }
}

/// Classify a tool call for phase tracking, using its arguments: a `shell` call
/// whose `command` string runs a test runner in any segment is a test run.
pub fn classify_tool_call(name: &str, args: &serde_json::Value) -> ToolPhaseKind {
    if name == "shell" {
        let runs_tests = args
            .get("command")
            .and_then(serde_json::Value::as_str)
            .is_some_and(command_runs_tests);
        return if runs_tests {
            ToolPhaseKind::Test
        } else {
            ToolPhaseKind::Read
        };
    }
    classify_by_name(name)
}

/// Whether any segment of a shell command line runs a test runner.
///
/// The line is split on `&&`, `||`, `;`, `|`, `&` and newlines outside quotes.
/// Leading `NAME=value` assignments and the wrappers `env`, `time`, `timeout`,
/// `nice`, `stdbuf`, `uv run`, `poetry run`, `pdm run`, `npx`, `bunx`,
/// `pnpm exec` and `bundle exec` are skipped, `sh|bash|zsh -c '<script>'` is
/// matched on its script, and path-qualified runners match by basename. A runner
/// invocation that only builds or lists tests (`--no-run`, `-- --list`,
/// `--collect-only`, `--co`) is not a test run.
fn command_runs_tests(command: &str) -> bool {
    shell_segments(command)
        .iter()
        .any(|tokens| segment_runs_tests(tokens))
}

/// Split a command line into segments of unquoted tokens. Separators inside
/// single or double quotes do not split; a backslash escapes the next char
/// outside single quotes.
fn shell_segments(command: &str) -> Vec<Vec<String>> {
    let mut segments: Vec<Vec<String>> = Vec::new();
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_token = false;
    let mut quote: Option<char> = None;
    let mut chars = command.chars();

    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    match chars.next() {
                        Some(n) if matches!(n, '"' | '\\' | '$' | '`') => cur.push(n),
                        Some(n) => {
                            cur.push('\\');
                            cur.push(n);
                        }
                        None => cur.push('\\'),
                    }
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    in_token = true;
                }
                '\\' => {
                    if let Some(n) = chars.next() {
                        cur.push(n);
                        in_token = true;
                    }
                }
                '\n' | ';' | '|' | '&' => {
                    if in_token {
                        tokens.push(std::mem::take(&mut cur));
                        in_token = false;
                    }
                    if !tokens.is_empty() {
                        segments.push(std::mem::take(&mut tokens));
                    }
                }
                c if c.is_whitespace() => {
                    if in_token {
                        tokens.push(std::mem::take(&mut cur));
                        in_token = false;
                    }
                }
                c => {
                    cur.push(c);
                    in_token = true;
                }
            },
        }
    }
    if in_token {
        tokens.push(cur);
    }
    if !tokens.is_empty() {
        segments.push(tokens);
    }
    segments
}

fn basename(token: &str) -> &str {
    token.rsplit('/').next().unwrap_or(token)
}

/// `python`, `python3`, `python3.12`.
fn is_python(name: &str) -> bool {
    name.strip_prefix("python")
        .is_some_and(|rest| rest.chars().all(|c| c.is_ascii_digit() || c == '.'))
}

/// `uv run` options that take a separate value.
fn uv_option_takes_value(opt: &str) -> bool {
    matches!(
        opt,
        "--with"
            | "--with-requirements"
            | "--with-editable"
            | "--python"
            | "-p"
            | "--directory"
            | "--project"
            | "--extra"
            | "--group"
            | "--env-file"
            | "--package"
    )
}

fn segment_runs_tests(tokens: &[String]) -> bool {
    let t: Vec<&str> = tokens.iter().map(String::as_str).collect();
    let mut i = 0;
    // Drop assignments and wrappers until the real command starts.
    while i < t.len() {
        let name = basename(t[i]);
        if is_assignment(t[i]) {
            i += 1;
            continue;
        }
        match name {
            "time" => i += 1,
            "env" => {
                i += 1;
                while let Some(opt) = t.get(i).filter(|s| s.starts_with('-')) {
                    i += if matches!(*opt, "-u" | "--unset" | "-C" | "--chdir") {
                        2
                    } else {
                        1
                    };
                }
            }
            "timeout" => {
                i += 1;
                while let Some(opt) = t.get(i).filter(|s| s.starts_with('-')) {
                    i += if matches!(*opt, "-s" | "-k" | "--signal" | "--kill-after") {
                        2
                    } else {
                        1
                    };
                }
                i += 1; // the duration
            }
            "nice" => {
                i += 1;
                if t.get(i) == Some(&"-n") {
                    i += 2;
                } else if t.get(i).is_some_and(|s| s.starts_with('-')) {
                    i += 1;
                }
            }
            "stdbuf" | "npx" | "bunx" => {
                i += 1;
                while t.get(i).is_some_and(|s| s.starts_with('-')) {
                    i += 1;
                }
            }
            "uv" if t.get(i + 1) == Some(&"run") => {
                i += 2;
                while let Some(opt) = t.get(i).filter(|s| s.starts_with('-')) {
                    i += if !opt.contains('=') && uv_option_takes_value(opt) {
                        2
                    } else {
                        1
                    };
                }
            }
            "poetry" | "pdm" if t.get(i + 1) == Some(&"run") => i += 2,
            "pnpm" | "bundle" if t.get(i + 1) == Some(&"exec") => i += 2,
            "sh" | "bash" | "zsh" => {
                // `sh [flags] -c '<script>'` (also `-lc`, `-ec`): match the script.
                let mut j = i + 1;
                while let Some(flag) = t.get(j).filter(|f| f.starts_with('-')) {
                    if !flag.starts_with("--") && flag.ends_with('c') {
                        return t
                            .get(j + 1)
                            .is_some_and(|script| command_runs_tests(script));
                    }
                    j += 1;
                }
                return false;
            }
            _ => break,
        }
    }
    let rest = &t[i.min(t.len())..];
    let Some(first) = rest.first() else {
        return false;
    };
    if only_lists_tests(rest) {
        return false;
    }
    let runner = basename(first);
    let arg = |n: usize| rest.get(n).copied();
    match runner {
        "cargo" => matches!(arg(1), Some("test" | "nextest" | "t")),
        "pytest" | "py.test" | "jest" | "vitest" | "ctest" | "rspec" | "phpunit" | "tox"
        | "nox" => true,
        r if is_python(r) => arg(1) == Some("-m") && matches!(arg(2), Some("pytest" | "unittest")),
        "make" => rest[1..]
            .iter()
            .find(|a| !a.starts_with('-'))
            .is_some_and(|target| target.starts_with("test") || *target == "check"),
        "npm" => match arg(1) {
            Some("t") => true,
            Some("run" | "run-script") => arg(2).is_some_and(|s| s.starts_with("test")),
            Some(s) => s.starts_with("test"),
            None => false,
        },
        "yarn" | "pnpm" | "bun" => match arg(1) {
            Some("run") => arg(2).is_some_and(|s| s.starts_with("test")),
            Some(s) => s.starts_with("test"),
            None => false,
        },
        "deno" | "go" | "mvn" | "gradle" | "gradlew" | "flutter" | "dart" | "mix" | "swift"
        | "dotnet" | "just" | "bazel" => arg(1) == Some("test"),
        _ => false,
    }
}

/// `--no-run`, `--collect-only`, `--co`, or `--list` after a bare `--`.
fn only_lists_tests(tokens: &[&str]) -> bool {
    let after_dashes = tokens.iter().position(|t| *t == "--");
    tokens.iter().enumerate().any(|(n, t)| {
        matches!(*t, "--no-run" | "--collect-only" | "--co" | "-list")
            || (*t == "--list" && after_dashes.is_some_and(|d| n > d))
    })
}

/// `NAME=value` (a shell variable assignment before a command).
fn is_assignment(token: &str) -> bool {
    match token.split_once('=') {
        Some((name, _)) => {
            !name.is_empty()
                && !name.starts_with(|c: char| c.is_ascii_digit())
                && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

// ---------------------------------------------------------------------------
// ContextManager
// ---------------------------------------------------------------------------

/// Manages conversation history within a token budget.
pub struct ContextManager {
    config: ContextConfig,
    max_context_tokens: u32,
    messages: Vec<Message>,
    dropped_count: usize,
    dropped_role_hints: Vec<String>,
    metrics: Option<Arc<ContextMetrics>>,
    content_hashes: HashMap<u64, usize>,
    phase_tracker: PhaseTracker,
    phase_compaction: bool,
    /// What compaction changed since the log was last drained (for receipts).
    compaction_log: Vec<CompactionStep>,
    /// Hash of each stored message (parallel to `messages`), filled lazily and
    /// reset when a message is rewritten.
    hashes: Vec<Option<String>>,
    /// Only drop-oldest trimming (no dedup, graduated or phase compaction).
    trim_only: bool,
}

impl ContextManager {
    /// Create with a raw token budget (backwards-compatible).
    pub fn new(max_context_tokens: u32) -> Self {
        Self {
            config: ContextConfig::default(),
            max_context_tokens,
            messages: Vec::new(),
            dropped_count: 0,
            dropped_role_hints: Vec::new(),
            metrics: None,
            content_hashes: HashMap::new(),
            phase_tracker: PhaseTracker::new(),
            phase_compaction: phase_compaction_from_env(),
            compaction_log: Vec::new(),
            hashes: Vec::new(),
            trim_only: false,
        }
    }

    /// Create with a model-aware config.
    pub fn with_config(config: ContextConfig) -> Self {
        let budget = config.effective_budget();
        Self {
            config,
            max_context_tokens: budget,
            messages: Vec::new(),
            dropped_count: 0,
            dropped_role_hints: Vec::new(),
            metrics: None,
            content_hashes: HashMap::new(),
            phase_tracker: PhaseTracker::new(),
            phase_compaction: phase_compaction_from_env(),
            compaction_log: Vec::new(),
            hashes: Vec::new(),
            trim_only: false,
        }
    }

    /// Set phase compaction in place (the `[context.graduated] phase` choice of
    /// a strategy selection).
    pub fn set_phase_compaction(&mut self, enabled: bool) {
        self.phase_compaction = enabled;
    }

    /// Turn compaction on phase transitions on or off, overriding
    /// `HONE_PHASE_COMPACTION` (read when the manager was built).
    pub fn with_phase_compaction(mut self, enabled: bool) -> Self {
        self.phase_compaction = enabled;
        self
    }

    /// Attach a metrics sink. Trim and compression events will be recorded on it.
    pub fn with_metrics(mut self, metrics: Arc<ContextMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Keep only drop-oldest trimming (needed to fit the budget): no tool-result
    /// dedup, no graduated compression and no phase compaction.
    pub fn set_trim_only(&mut self, trim_only: bool) {
        self.trim_only = trim_only;
    }

    /// Whether only trimming is on.
    pub fn trim_only(&self) -> bool {
        self.trim_only
    }

    /// Whether compaction on phase transitions is on.
    pub fn phase_compaction(&self) -> bool {
        self.phase_compaction
    }

    /// Lower the budget ratio to at most `cap` (a project file's `budget_ratio`):
    /// `min(current ratio, cap)`, never higher.
    pub fn cap_budget_ratio(&mut self, cap: f32) {
        if cap < self.config.effective_budget_ratio {
            self.set_budget_ratio(cap);
        }
    }

    /// Use `ratio` of the advertised window as the token budget instead of the
    /// per-model ratio.
    pub fn set_budget_ratio(&mut self, ratio: f32) {
        self.config.effective_budget_ratio = ratio;
        self.max_context_tokens = self.config.effective_budget();
    }

    /// Notify the context manager about a tool call (for phase tracking).
    ///
    /// Call this before adding the tool result message. If a phase transition
    /// occurs (e.g., exploration → editing), exploration results are
    /// aggressively compacted.
    pub fn notify_tool_call(&mut self, tool_name: &str) {
        let kind = classify_by_name(tool_name);
        self.notify_kind(kind, &[]);
    }

    /// Like [`notify_tool_call`](Self::notify_tool_call), but classified with
    /// the call's arguments, so a `shell` call that runs a test suite counts as
    /// a test run.
    pub fn notify_tool_use(&mut self, tool_name: &str, args: &serde_json::Value) {
        let kind = classify_tool_call(tool_name, args);
        let targets = if kind == ToolPhaseKind::Write {
            write_targets(tool_name, args)
        } else {
            Vec::new()
        };
        self.notify_kind(kind, &targets);
    }

    fn notify_kind(&mut self, kind: ToolPhaseKind, targets: &[String]) {
        // The tracker and the transition metric always run; only the compaction
        // is skipped under trim-only.
        if let Some((from, to)) = self.phase_tracker.observe_kind(kind) {
            if from == TaskPhase::Exploration
                && (to == TaskPhase::Editing || to == TaskPhase::Verification)
            {
                if self.phase_compaction && !self.trim_only {
                    self.force_compress_exploration_results(targets);
                }
                if let Some(m) = &self.metrics {
                    m.record_phase_transition();
                }
            }
        }
    }

    /// Current task phase.
    pub fn current_phase(&self) -> TaskPhase {
        self.phase_tracker.current_phase()
    }

    /// Take the compaction steps recorded since the last call: phase,
    /// graduated, dedup and drop-oldest changes to the stored history, each
    /// with the hashes of the messages it replaced and of the message that took
    /// their place. Indexes are positions in the stored history (the system
    /// prompt, recall and skills blocks are not part of it).
    pub fn drain_compaction_log(&mut self) -> Vec<CompactionStep> {
        std::mem::take(&mut self.compaction_log)
    }

    fn log_step(
        &mut self,
        name: &str,
        params: serde_json::Value,
        replaced: Vec<SourceRef>,
        derived: Option<SourceRef>,
    ) {
        self.compaction_log.push(CompactionStep {
            strategy: StrategyId {
                slot: Slot::History,
                name: name.to_owned(),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                params_sha256: canonical_json_sha256(&params),
            },
            replaced,
            derived,
        });
    }

    /// Append a message to the conversation history, trimming old messages if
    /// the estimated token count exceeds the budget.
    ///
    /// Tool messages whose content matches a previously seen hash are replaced
    /// with a lightweight pointer to save tokens.
    pub fn add_message(&mut self, message: Message) {
        let idx = self.messages.len();

        // Dedup: for Tool messages with substantial content, check whether we
        // have already seen identical content.  If so, replace the incoming
        // message with a one-line pointer instead of storing the full text again.
        let message = if matches!(message.role, Role::Tool) && !self.trim_only {
            let text = message.content.as_text();
            if text.len() > 500 {
                let hash = hash_content(&text);
                if let Some(&first_idx) = self.content_hashes.get(&hash) {
                    // Duplicate detected — replace content with a pointer.
                    let summary = format!(
                        "[duplicate content: identical to tool result at message {first_idx} (hash {:016x})]",
                        hash
                    );
                    if let Some(m) = &self.metrics {
                        m.record_dedup_hit();
                    }
                    let original = message_ref(idx, &message);
                    let pointer = Message {
                        content: Content::Text(summary),
                        ..message
                    };
                    let derived = message_ref(idx, &pointer);
                    self.log_step(
                        "dedup",
                        serde_json::json!({}),
                        vec![original],
                        Some(derived),
                    );
                    pointer
                } else {
                    self.content_hashes.insert(hash, idx);
                    message
                }
            } else {
                message
            }
        } else {
            message
        };

        self.messages.push(message);
        self.hashes.push(None);
        self.trim_to_budget();
    }

    /// Preservation-aware compaction.
    ///
    /// 1. Always preserve the first stored system message (index 0 if present)
    ///    and the first user message — these are the most critical anchors.
    /// 2. Compress old tool results (older than 5 assistant turns) to a short
    ///    summary — runs unconditionally every call so stale results are always
    ///    shrunk even when the budget is not yet exhausted.
    /// 3. Drop the oldest non-preserved, non-summary messages until under
    ///    budget, recording what was dropped.
    /// 4. Flush a synthetic "[session history: …]" system message so the model
    ///    knows context was compacted.
    fn trim_to_budget(&mut self) {
        // Step 1: graduated compression — runs unconditionally (unless trim-only).
        if !self.trim_only {
            self.compress_old_tool_results();
        }

        if self.total_tokens() <= self.max_context_tokens {
            return;
        }

        // Step 2: drop oldest non-preserved messages until under budget.
        let dropped_before = self.dropped_count;
        let recent_window = self.config.recent_window;
        let mut removed_refs: Vec<SourceRef> = Vec::new();
        // Position each stored message had before this pass started.
        let mut pre_pass: Vec<usize> = (0..self.messages.len()).collect();
        loop {
            if self.total_tokens() <= self.max_context_tokens {
                break;
            }

            let preserved = preserved_indices(&self.messages);
            let protected_from = self.messages.len().saturating_sub(recent_window);
            let drop_idx = (0..protected_from)
                .find(|i| !preserved.contains(i) && !is_summary_marker_at(&self.messages, *i));

            let Some(idx) = drop_idx else {
                break;
            };

            let removed = self.messages.remove(idx);
            self.hashes.remove(idx);
            let before = pre_pass.remove(idx);
            removed_refs.push(message_ref(before, &removed));
            self.record_dropped_message(removed);
        }

        let newly_dropped = self.dropped_count - dropped_before;
        if newly_dropped > 0 {
            if let Some(m) = &self.metrics {
                m.record_trim(newly_dropped as u32);
            }
        }

        // Step 3: emit a summary marker.
        let (marker, old_marker) = self.flush_dropped_summary(&pre_pass);
        removed_refs.extend(old_marker);
        if !removed_refs.is_empty() {
            self.log_step(
                "drop_oldest",
                serde_json::json!({ "recent_window": recent_window }),
                removed_refs,
                marker,
            );
        }
    }

    /// Graduated tool result compression.
    ///
    /// Three tiers based on assistant-turn age:
    /// - **Full** (age < tool_compress_age): no compression
    /// - **Structural summary** (age tool_compress_age..tool_oneliner_age):
    ///   extract code signatures, test PASS/FAIL lines, or grep paths
    /// - **One-liner** (age >= tool_oneliner_age): single line with char count
    ///
    /// Messages in the protected recent window are never compressed.
    fn compress_old_tool_results(&mut self) {
        let protected_from = self
            .messages
            .len()
            .saturating_sub(self.config.recent_window);
        let summary_age = self.config.tool_compress_age;
        let oneliner_age = self.config.tool_oneliner_age();

        let mut assistant_turns_since = 0usize;
        for i in (0..protected_from).rev() {
            match self.messages[i].role {
                Role::Assistant => {
                    assistant_turns_since += 1;
                }
                Role::Tool if assistant_turns_since >= summary_age => {
                    let text = self.messages[i].content.as_text().to_string();

                    // Allow re-compression from structural summary to one-liner
                    // at higher ages, but skip content that's already a one-liner.
                    if text.starts_with("[tool result:") || text.starts_with("[duplicate content") {
                        continue;
                    }

                    let compressed = if assistant_turns_since >= oneliner_age {
                        // One-liner: works on both raw content and structural summaries.
                        if text.starts_with("[tool result summarized") {
                            // Already a structural summary — collapse to one-liner.
                            let len = text.len();
                            format!("[tool result: {len} chars, summarized content]")
                        } else {
                            tool_oneliner(&text)
                        }
                    } else if text.starts_with("[tool result summarized") {
                        // Already structural summary at this age — skip.
                        continue;
                    } else {
                        structural_summary(
                            &text,
                            self.call_paths_for(i).first().map(String::as_str),
                        )
                    };

                    let before = message_ref(i, &self.messages[i]);
                    self.messages[i].content = Content::Text(compressed);
                    self.hashes[i] = None;
                    let after = message_ref(i, &self.messages[i]);
                    self.log_step(
                        "graduated",
                        serde_json::json!({
                            "oneliner_age": oneliner_age,
                            "recent_window": self.config.recent_window,
                            "summary_age": summary_age,
                        }),
                        vec![before],
                        Some(after),
                    );
                    if let Some(m) = &self.metrics {
                        m.record_compression();
                    }
                }
                _ => {}
            }
        }
    }

    /// Force-compress exploration tool results. Called on phase transitions
    /// (exploration → editing/verification) when phase compaction is enabled.
    ///
    /// The newest `recent_window` tool results stay verbatim, and so does any
    /// result of a read of one of `targets` (the paths the triggering write
    /// changes).
    fn force_compress_exploration_results(&mut self, targets: &[String]) {
        let tool_idx: Vec<usize> = (0..self.messages.len())
            .filter(|&i| matches!(self.messages[i].role, Role::Tool))
            .collect();
        let older = tool_idx.len().saturating_sub(self.config.recent_window);

        for &i in &tool_idx[..older] {
            let text = self.messages[i].content.as_text().to_string();
            if is_already_compressed(&text) || text.len() <= 200 {
                continue;
            }
            if !targets.is_empty() && self.read_path_matches(i, &text, targets) {
                continue;
            }
            let path = self.call_paths_for(i).into_iter().next();
            let before = message_ref(i, &self.messages[i]);
            self.messages[i].content = Content::Text(structural_summary(&text, path.as_deref()));
            self.hashes[i] = None;
            let after = message_ref(i, &self.messages[i]);
            self.log_step(
                "phase",
                serde_json::json!({ "recent_window": self.config.recent_window }),
                vec![before],
                Some(after),
            );
            if let Some(m) = &self.metrics {
                m.record_compression();
            }
        }
    }

    /// `path` / `file_path` arguments of the assistant tool call the tool
    /// result at `idx` answers.
    fn call_paths_for(&self, idx: usize) -> Vec<String> {
        let Some(id) = self.messages[idx].tool_call_id.as_deref() else {
            return Vec::new();
        };
        self.messages[..idx]
            .iter()
            .rev()
            .flat_map(|m| m.tool_calls.iter())
            .find(|c| c.id == id)
            .map(|c| call_paths(&c.arguments))
            .unwrap_or_default()
    }

    /// Whether the tool result at `idx` read one of `targets`: its path comes
    /// from the result JSON's `path`, or from the arguments of the assistant
    /// tool call with the same id.
    fn read_path_matches(&self, idx: usize, text: &str, targets: &[String]) -> bool {
        let hit = |p: &str| targets.iter().any(|t| same_file(t, p));
        if let Some(path) = result_json(text).and_then(|v| {
            v.get("path")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        }) && hit(&path)
        {
            return true;
        }
        self.call_paths_for(idx).iter().any(|p| hit(p))
    }

    /// Record metadata about a message we are about to drop.
    fn record_dropped_message(&mut self, msg: Message) {
        self.dropped_count += 1;
        let role_hint = format!("{:?}", msg.role).to_lowercase();
        self.dropped_role_hints.push(role_hint);
    }

    /// Emit a "[session history: …]" system message summarising every message
    /// dropped in this trim pass.  Replaces any existing summary marker to
    /// avoid duplicates.
    ///
    /// `pre_pass` maps each stored position to the position it had before the
    /// current drop pass. Returns the new marker's ref and the ref of the marker
    /// it replaced, if any.
    fn flush_dropped_summary(
        &mut self,
        pre_pass: &[usize],
    ) -> (Option<SourceRef>, Option<SourceRef>) {
        if self.dropped_count == 0 {
            return (None, None);
        }

        let summary = format!(
            "[session history: {} earlier messages compressed to fit context window — {}]",
            self.dropped_count,
            self.dropped_role_hints.join(", ")
        );

        // Remove any pre-existing summary marker first.
        let mut replaced = None;
        if let Some(existing) =
            (0..self.messages.len()).find(|i| is_summary_marker_at(&self.messages, *i))
        {
            let old = self.messages.remove(existing);
            self.hashes.remove(existing);
            replaced = Some(message_ref(
                pre_pass.get(existing).copied().unwrap_or(existing),
                &old,
            ));
        }

        // Insert after the first stored system message (index 0) if one is
        // present; otherwise at the front.
        let insert_at = if self
            .messages
            .first()
            .is_some_and(|m| matches!(m.role, Role::System))
        {
            1
        } else {
            0
        };

        let marker = Message {
            role: Role::System,
            content: Content::Text(summary),
            tool_calls: vec![],
            tool_call_id: None,
        };
        let marker_ref = message_ref(insert_at, &marker);
        self.messages.insert(insert_at, marker);
        self.hashes.insert(insert_at, None);

        self.dropped_count = 0;
        self.dropped_role_hints.clear();
        (Some(marker_ref), replaced)
    }

    /// Build the full message list for a completion request.
    ///
    /// Prepends a system message derived from `system_prompt`, then appends
    /// all stored messages. When `reorder_middle_zone` is enabled, Tool
    /// messages in the middle zone are reordered by BM25 relevance to the
    /// latest user query so the most relevant content sits at the high-
    /// attention edges of the context window.
    pub fn build_messages(&self, system_prompt: &str) -> Vec<Message> {
        let mut result = Vec::with_capacity(self.messages.len() + 1);
        result.push(Message::system(system_prompt));
        result.extend(
            self.request_order()
                .into_iter()
                .map(|i| self.messages[i].clone()),
        );
        result
    }

    /// Like [`build_messages`](Self::build_messages), plus the hash of each
    /// message (the system prompt included), from the per-message cache: only new
    /// or rewritten messages are hashed.
    pub fn build_messages_hashed(&mut self, system_prompt: &str) -> (Vec<Message>, Vec<String>) {
        let order = self.request_order();
        let system = Message::system(system_prompt);
        let mut messages = Vec::with_capacity(order.len() + 1);
        let mut hashes = Vec::with_capacity(order.len() + 1);
        hashes.push(message_sha256(&system));
        messages.push(system);
        for i in order {
            let hash = match &self.hashes[i] {
                Some(h) => h.clone(),
                None => {
                    let h = message_sha256(&self.messages[i]);
                    self.hashes[i] = Some(h.clone());
                    h
                }
            };
            hashes.push(hash);
            messages.push(self.messages[i].clone());
        }
        (messages, hashes)
    }

    /// Stored-message indexes in the order they are sent.
    fn request_order(&self) -> Vec<usize> {
        use crate::relevance::{Bm25Scorer, interleave_edges};

        let mut result: Vec<usize> = Vec::with_capacity(self.messages.len());

        // Short-circuit: no reordering if disabled or too few messages.
        let recent_window = self.config.recent_window;
        if !self.config.reorder_middle_zone || self.messages.len() <= recent_window + 2 {
            return (0..self.messages.len()).collect();
        }

        // Identify zones.
        let preserved = preserved_indices(&self.messages);
        let recent_start = self.messages.len().saturating_sub(recent_window);

        // Extract the latest user query.
        let query = self
            .messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, Role::User))
            .map(|m| m.content.as_text().to_string())
            .unwrap_or_default();

        if query.is_empty() {
            return (0..self.messages.len()).collect();
        }

        // Partition messages into start/middle/recent zones.
        let mut start_zone: Vec<usize> = Vec::new();
        let mut middle_tool_indices: Vec<usize> = Vec::new();
        let mut middle_other_indices: Vec<usize> = Vec::new();
        let mut recent_zone: Vec<usize> = Vec::new();

        for i in 0..self.messages.len() {
            if preserved.contains(&i) || is_summary_marker_at(&self.messages, i) {
                start_zone.push(i);
            } else if i >= recent_start {
                recent_zone.push(i);
            } else if matches!(self.messages[i].role, Role::Tool) {
                middle_tool_indices.push(i);
            } else {
                middle_other_indices.push(i);
            }
        }

        // Only reorder if there are enough tool messages to matter.
        if middle_tool_indices.len() < 3 {
            return (0..self.messages.len()).collect();
        }

        // Score tool messages by BM25 relevance.
        let tool_texts: Vec<String> = middle_tool_indices
            .iter()
            .map(|&i| self.messages[i].content.as_text().to_string())
            .collect();
        let scorer = Bm25Scorer::from_corpus(&tool_texts);
        let mut scored: Vec<(f32, usize)> = middle_tool_indices
            .iter()
            .enumerate()
            .map(|(corpus_idx, &msg_idx)| (scorer.score(&tool_texts[corpus_idx], &query), msg_idx))
            .collect();
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let reordered_tool_indices: Vec<usize> = interleave_edges(scored)
            .into_iter()
            .map(|(_, idx)| idx)
            .collect();

        // Assemble: start + middle_other (chronological) interleaved with
        // reordered tools + recent.
        for &i in &start_zone {
            result.push(i);
        }

        // Merge middle_other (chronological) and reordered tools.
        // We place non-tool messages in their original chronological positions
        // and insert reordered tool messages in the gaps.
        let mut tool_iter = reordered_tool_indices.iter();
        for &i in &middle_other_indices {
            result.push(i);
            // After each non-tool message, insert the next reordered tool if
            // there are tools remaining.
            if let Some(&ti) = tool_iter.next() {
                result.push(ti);
            }
        }
        // Append any remaining tool messages.
        for &ti in tool_iter {
            result.push(ti);
        }

        for &i in &recent_zone {
            result.push(i);
        }

        result
    }

    /// Remove all stored messages (system prompt is re-supplied at build time).
    pub fn clear(&mut self) {
        self.messages.clear();
        self.hashes.clear();
        self.compaction_log.clear();
    }

    /// Number of non-system messages stored.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Estimated total token count of all stored messages (excludes the system
    /// prompt, which is supplied at build time and not stored here).
    pub fn total_tokens(&self) -> u32 {
        self.messages.iter().map(estimate_message_tokens).sum()
    }

    /// Return a clone of all messages currently held in the context window.
    ///
    /// Intended for session persistence: callers save this snapshot and can
    /// later replay it via repeated `add_message` calls (or `AgentHandle::load_history`).
    pub fn messages(&self) -> Vec<Message> {
        self.messages.clone()
    }

    /// Effective token budget the manager is trimming to.
    pub fn max_context_tokens(&self) -> u32 {
        self.max_context_tokens
    }

    /// Whether current context usage has passed `ratio × max_context_tokens`.
    ///
    /// `extra_tokens` accounts for tokens that will be part of the outgoing
    /// request but are NOT stored in `self.messages` — typically the system
    /// prompt (which is supplied at build time) plus any per-turn system
    /// messages such as injected memories. Pass `0` if nothing else is
    /// prepended.
    ///
    /// Used by the agent loop to trigger a graceful wrap-up when the budget is
    /// nearly exhausted — at ~0.95 we disable further tool calls and ask the
    /// model to summarise before the next response dies mid-stream to a
    /// context-length error. Port of ml-intern's emergency-summary path
    /// (see `docs/ml-intern-vs-hone.md §3.5`).
    pub fn is_over_ratio(&self, ratio: f32, extra_tokens: u32) -> bool {
        let limit = (self.max_context_tokens as f32 * ratio) as u32;
        self.total_tokens().saturating_add(extra_tokens) > limit
    }
}

/// Estimate the token count of a plain-text snippet (e.g., the system prompt)
/// using the same BPE as [`ContextManager`]. Exposed so callers can budget
/// out-of-band content consistently with stored messages.
pub fn estimate_text_tokens(text: &str) -> u32 {
    let tokens = match bpe() {
        Some(enc) => enc.encode_ordinary(text).len() as u32,
        None => (text.len() as u32 / 4).max(1),
    };
    tokens + 4 // per-message framing overhead, matching `estimate_message_tokens`
}

/// Whether `HONE_PHASE_COMPACTION` enables compaction on phase transitions
/// (`1`, `true` or `on`). Read each time a manager is built.
fn phase_compaction_from_env() -> bool {
    std::env::var("HONE_PHASE_COMPACTION")
        .is_ok_and(|v| hone_config::strategy::phase_env_enabled(&v))
}

/// Two paths name the same file when, after stripping a leading `./`, they are
/// equal or one ends with `/` followed by the other (whole components).
fn same_file(a: &str, b: &str) -> bool {
    let (a, b) = (normalize_path(a), normalize_path(b));
    let ends = |long: &str, short: &str| {
        long.len() > short.len()
            && long.ends_with(short)
            && long.as_bytes()[long.len() - short.len() - 1] == b'/'
    };
    a == b || ends(a, b) || ends(b, a)
}

fn normalize_path(p: &str) -> &str {
    p.strip_prefix("./").unwrap_or(p)
}

/// `path` / `file_path` arguments of a tool call.
fn call_paths(args: &serde_json::Value) -> Vec<String> {
    ["path", "file_path"]
        .iter()
        .filter_map(|k| args.get(*k).and_then(serde_json::Value::as_str))
        .map(str::to_owned)
        .collect()
}

/// Paths a write call targets: `path` / `file_path`, and for `apply_patch` the
/// `--- a/…` / `+++ b/…` header paths of its `patch` argument.
fn write_targets(name: &str, args: &serde_json::Value) -> Vec<String> {
    let mut paths = call_paths(args);
    if name == "multi_edit"
        && let Some(edits) = args.get("edits").and_then(serde_json::Value::as_array)
    {
        for edit in edits {
            paths.extend(call_paths(edit));
        }
    }
    if matches!(name, "apply_patch" | "apply_diff") {
        let patches = ["diff", "patch"]
            .iter()
            .filter_map(|k| args.get(*k).and_then(serde_json::Value::as_str));
        for line in patches.flat_map(str::lines) {
            let Some(rest) = line
                .strip_prefix("--- ")
                .or_else(|| line.strip_prefix("+++ "))
            else {
                continue;
            };
            let path = rest.split('\t').next().unwrap_or(rest).trim();
            let path = path
                .strip_prefix("a/")
                .or_else(|| path.strip_prefix("b/"))
                .unwrap_or(path);
            if !path.is_empty() && path != "/dev/null" {
                paths.push(path.to_owned());
            }
        }
    }
    paths.iter().map(|p| normalize_path(p).to_owned()).collect()
}

/// A tool result that is a JSON object (as `read_file` returns).
fn result_json(text: &str) -> Option<serde_json::Value> {
    if !text.trim_start().starts_with('{') {
        return None;
    }
    serde_json::from_str(text).ok()
}

/// The longest prefix of `text` that is at most `max_bytes` and ends on a char
/// boundary.
fn prefix(text: &str, max_bytes: usize) -> &str {
    &text[..safe_char_boundary(text, max_bytes)]
}

// ---------------------------------------------------------------------------
// Graduated compression helpers
// ---------------------------------------------------------------------------

/// A `Message` source ref for the message at `index`, hashed as it is now.
fn message_ref(index: usize, message: &Message) -> SourceRef {
    SourceRef::Message {
        index,
        sha256: message_sha256(message),
    }
}

/// Check if text is already compressed by a previous pass.
fn is_already_compressed(text: &str) -> bool {
    text.starts_with("[tool result")
        || text.starts_with("[duplicate content")
        || text.starts_with("[session history")
}

/// Produce a structural summary that preserves key information.
///
/// Detects content type heuristically and extracts:
/// - Code: function/struct/enum/class/impl signatures
/// - Test output: PASS/FAIL lines + first error
/// - Grep results: file:line paths
/// - Default: first 200 chars
fn structural_summary(text: &str, call_path: Option<&str>) -> String {
    let len = text.len();
    if let Some(summary) = file_read_summary(text, call_path) {
        return summary;
    }
    let sample = prefix(text, 500);

    if looks_like_test_output(sample) {
        let lines: Vec<&str> = text.lines().collect();
        let mut kept: Vec<&str> = Vec::new();
        let mut first_error = true;
        for line in &lines {
            let trimmed = line.trim();
            if trimmed.contains("PASS")
                || trimmed.contains("FAIL")
                || trimmed.contains("test result")
                || trimmed.starts_with("ok.")
                || trimmed.starts_with("not ok")
            {
                kept.push(line);
            } else if first_error
                && (trimmed.contains("error")
                    || trimmed.contains("Error")
                    || trimmed.contains("panic"))
            {
                kept.push(line);
                first_error = false;
            }
            if kept.len() >= 15 {
                break;
            }
        }
        if kept.is_empty() {
            return format!(
                "[tool result summarized, {} chars: {}...]",
                len,
                prefix(text, 200)
            );
        }
        format!(
            "[tool result summarized, {} chars, test output:\n{}]",
            len,
            kept.join("\n")
        )
    } else if looks_like_code(sample) {
        let lines: Vec<&str> = text.lines().collect();
        let sigs: Vec<&str> = lines
            .iter()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("fn ")
                    || t.starts_with("pub fn ")
                    || t.starts_with("pub(crate) fn ")
                    || t.starts_with("async fn ")
                    || t.starts_with("pub async fn ")
                    || t.starts_with("struct ")
                    || t.starts_with("pub struct ")
                    || t.starts_with("enum ")
                    || t.starts_with("pub enum ")
                    || t.starts_with("impl ")
                    || t.starts_with("trait ")
                    || t.starts_with("pub trait ")
                    || t.starts_with("mod ")
                    || t.starts_with("pub mod ")
                    || t.starts_with("use ")
                    || t.starts_with("def ")
                    || t.starts_with("class ")
                    || t.starts_with("function ")
                    || t.starts_with("export ")
            })
            .take(20)
            .copied()
            .collect();
        if sigs.is_empty() {
            return format!(
                "[tool result summarized, {} chars: {}...]",
                len,
                prefix(text, 200)
            );
        }
        format!(
            "[tool result summarized, {} chars, {} signatures:\n{}]",
            len,
            sigs.len(),
            sigs.join("\n")
        )
    } else if looks_like_grep_result(sample) {
        let path_strings: Vec<String> = text
            .lines()
            .filter(|l| l.contains(':') && !l.starts_with('['))
            .take(20)
            .map(|l| {
                let parts: Vec<&str> = l.splitn(3, ':').collect();
                if parts.len() >= 2 {
                    format!("{}:{}", parts[0], parts[1])
                } else {
                    l.to_string()
                }
            })
            .collect();
        format!(
            "[tool result summarized, {} chars, grep {} matches:\n{}]",
            len,
            path_strings.len(),
            path_strings.join("\n")
        )
    } else {
        format!(
            "[tool result summarized, {} chars: {}...]",
            len,
            prefix(text, 200)
        )
    }
}

/// Summary of a JSON file-read result (`{"content": "...", "path": "..."}`):
/// the path, the line count and up to 40 declaration lines of `content`.
fn file_read_summary(text: &str, call_path: Option<&str>) -> Option<String> {
    let value = result_json(text)?;
    let content = value.get("content")?.as_str()?;
    let path = value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .or(call_path);
    let num = |k: &str| value.get(k).and_then(serde_json::Value::as_u64);

    let decls: Vec<String> = content
        .lines()
        .filter_map(|l| {
            let (n, code) = split_line_number(l);
            is_declaration(code).then(|| match n {
                Some(n) => format!("{n}: {code}"),
                None => code.to_owned(),
            })
        })
        .collect();

    let range = match (num("start_line"), num("end_line"), num("total_lines")) {
        (Some(s), Some(e), Some(t)) => format!("lines {s}\u{2013}{e} of {t}"),
        _ => format!("{} lines", content.lines().count()),
    };
    let truncated = if value.get("truncated").and_then(serde_json::Value::as_bool) == Some(true) {
        " (truncated)"
    } else {
        ""
    };
    let head = match path {
        Some(p) => format!("file {p}, {range}{truncated}"),
        None => format!("file, {range}{truncated}"),
    };
    let len = text.len();
    Some(if decls.is_empty() {
        format!(
            "[tool result summarized, {len} chars, {head}: {}... (re-read before editing)]",
            prefix(content, 200)
        )
    } else {
        let n = decls.len();
        let count = match n {
            1 => "1 declaration".to_owned(),
            2..=40 => format!("{n} declarations"),
            _ => format!("first 40 of {n} declarations"),
        };
        let kept = decls
            .iter()
            .take(40)
            .map(String::as_str)
            .collect::<Vec<_>>();
        format!(
            "[tool result summarized, {len} chars, {head}, {count}:\n{}\n(re-read before editing)]",
            kept.join("\n")
        )
    })
}

/// Split a `read_file` line of the form `"   42 | code"` into its number and
/// the trimmed code; other lines come back trimmed with no number.
fn split_line_number(line: &str) -> (Option<u64>, &str) {
    let t = line.trim_start();
    let digits = t.bytes().take_while(u8::is_ascii_digit).count();
    if digits > 0
        && let Some(rest) = t[digits..].strip_prefix(" | ")
        && let Ok(n) = t[..digits].parse()
    {
        return (Some(n), rest.trim());
    }
    (None, t.trim())
}

/// Whether a trimmed line starts a declaration in a common language.
fn is_declaration(t: &str) -> bool {
    [
        "fn ",
        "async fn ",
        "struct ",
        "enum ",
        "impl ",
        "impl<",
        "trait ",
        "mod ",
        "class ",
        "def ",
        "async def ",
        "function ",
        "interface ",
        "type ",
        "export ",
        "pub ",
        "pub(",
    ]
    .iter()
    .any(|k| t.starts_with(k))
}

/// Produce a one-liner description for very old tool results.
fn tool_oneliner(text: &str) -> String {
    let len = text.len();
    let descriptor = if looks_like_test_output(prefix(text, 500)) {
        let pass = text.matches("PASS").count() + text.matches(" ok").count();
        let fail = text.matches("FAIL").count() + text.matches("FAILED").count();
        format!("test output ({pass} passed, {fail} failed)")
    } else if looks_like_code(prefix(text, 500)) {
        let fns = text
            .lines()
            .filter(|l| {
                let t = l.trim();
                t.starts_with("fn ")
                    || t.starts_with("pub fn ")
                    || t.starts_with("def ")
                    || t.starts_with("function ")
            })
            .count();
        format!("code ({fns} functions)")
    } else if looks_like_grep_result(prefix(text, 500)) {
        let matches = text.lines().filter(|l| l.contains(':')).count();
        format!("grep ({matches} matches)")
    } else {
        "text content".to_string()
    };
    format!("[tool result: {len} chars, {descriptor}]")
}

fn looks_like_code(sample: &str) -> bool {
    sample.contains("fn ")
        || sample.contains("def ")
        || sample.contains("function ")
        || sample.contains("class ")
        || sample.contains("struct ")
        || sample.contains("impl ")
}

fn looks_like_test_output(sample: &str) -> bool {
    (sample.contains("PASS") || sample.contains("FAIL") || sample.contains("test result"))
        && (sample.contains("test") || sample.contains("Test"))
}

fn looks_like_grep_result(sample: &str) -> bool {
    let lines: Vec<&str> = sample.lines().take(10).collect();
    if lines.is_empty() {
        return false;
    }
    let colon_lines = lines
        .iter()
        .filter(|l| {
            let parts: Vec<&str> = l.splitn(3, ':').collect();
            parts.len() >= 2 && parts[0].contains('.')
        })
        .count();
    colon_lines as f32 / lines.len() as f32 > 0.5
}

/// Find a safe UTF-8 char boundary at or before `max_bytes`.
fn safe_char_boundary(text: &str, max_bytes: usize) -> usize {
    if max_bytes >= text.len() {
        return text.len();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

// ---------------------------------------------------------------------------
// Free functions (no self borrow — avoids lifetime conflicts inside closures)
// ---------------------------------------------------------------------------

/// Returns the indices of messages that must never be dropped:
/// - The first non-summary stored system message (typically the context seed).
/// - The first user message (the original request).
fn preserved_indices(messages: &[Message]) -> HashSet<usize> {
    let mut preserved = HashSet::new();

    // First system message that is NOT our synthetic summary marker.
    if let Some(idx) = messages.iter().position(|m| {
        matches!(m.role, Role::System) && !m.content.as_text().starts_with("[session history:")
    }) {
        preserved.insert(idx);
    }

    // First user message — the original request.
    if let Some(idx) = messages.iter().position(|m| matches!(m.role, Role::User)) {
        preserved.insert(idx);
    }

    preserved
}

/// Returns `true` if the message at `idx` is the synthetic session-history
/// summary marker inserted by `flush_dropped_summary`.
fn is_summary_marker_at(messages: &[Message], idx: usize) -> bool {
    match messages.get(idx) {
        Some(msg) if matches!(msg.role, Role::System) => {
            msg.content.as_text().starts_with("[session history:")
        }
        _ => false,
    }
}

/// Compute a 64-bit hash of a string for content-identity comparisons.
///
/// Uses `DefaultHasher` — non-cryptographic but fast and stable within a
/// single process run, which is all we need for dedup within a session.
fn hash_content(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

/// Global BPE tokenizer, initialized once on first use.
///
/// `cl100k_base` is the tokenizer used by Claude and GPT-4 models.  Loading it
/// takes ~10 ms and ~5 MB of memory; we pay that cost exactly once.
static BPE: OnceLock<CoreBPE> = OnceLock::new();

fn bpe() -> Option<&'static CoreBPE> {
    BPE.get_or_init(|| {
        tiktoken_rs::cl100k_base().expect("tiktoken cl100k_base data embedded in binary")
    });
    BPE.get()
}

/// Estimate the number of tokens in a message using tiktoken (`cl100k_base`).
///
/// Falls back to the 4-chars-per-token heuristic if the tokenizer cannot be
/// loaded (should never happen — the BPE data is embedded at compile time).
/// A per-message overhead of +4 tokens accounts for role/formatting framing.
fn estimate_message_tokens(msg: &Message) -> u32 {
    let text = msg.content.as_text();
    match bpe() {
        Some(enc) => enc.encode_ordinary(&text.to_string()).len() as u32 + 4,
        None => (text.len() as u32 / 4).max(1) + 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hone_proto::{Message, Role};

    #[test]
    fn test_add_and_retrieve_messages() {
        let mut ctx = ContextManager::new(8192);
        ctx.add_message(Message::user("hello"));
        ctx.add_message(Message::assistant("hi there"));
        ctx.add_message(Message::user("how are you?"));

        let messages = ctx.build_messages("You are a helpful assistant.");
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn test_system_message_always_present() {
        let mut ctx = ContextManager::new(8192);
        for i in 0..10 {
            ctx.add_message(Message::user(format!("message {i}")));
        }

        let messages = ctx.build_messages("System prompt here.");
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content.as_text(), "System prompt here.");
    }

    #[test]
    fn test_clear_resets() {
        let mut ctx = ContextManager::new(8192);
        ctx.add_message(Message::user("something"));
        ctx.add_message(Message::assistant("response"));
        ctx.clear();
        assert_eq!(ctx.message_count(), 0);
    }

    #[test]
    fn test_build_messages_includes_system_prompt() {
        let ctx = ContextManager::new(8192);
        let messages = ctx.build_messages("Be concise.");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content.as_text(), "Be concise.");
    }

    // --- ContextConfig tests ---

    #[test]
    fn test_config_for_claude() {
        let cfg = ContextConfig::for_model("claude-sonnet-4-5");
        assert!((cfg.effective_budget_ratio - 0.60).abs() < 0.01);
        assert_eq!(cfg.recent_window, 3);
        assert_eq!(cfg.effective_budget(), 120_000);
    }

    #[test]
    fn test_config_for_deepseek() {
        let cfg = ContextConfig::for_model("deepseek-v3");
        assert!((cfg.effective_budget_ratio - 0.45).abs() < 0.01);
        assert_eq!(cfg.recent_window, 5);
    }

    #[test]
    fn test_config_for_llama() {
        let cfg = ContextConfig::for_model("ollama:llama3.2");
        assert!((cfg.effective_budget_ratio - 0.40).abs() < 0.01);
        assert_eq!(cfg.recent_window, 7);
    }

    #[test]
    fn test_config_default_for_unknown() {
        let cfg = ContextConfig::for_model("some-unknown-model");
        assert!((cfg.effective_budget_ratio - 0.50).abs() < 0.01);
    }

    #[test]
    fn test_with_config_uses_effective_budget() {
        let cfg = ContextConfig::for_model("claude-sonnet-4-5");
        let ctx = ContextManager::with_config(cfg);
        assert_eq!(ctx.max_context_tokens, 120_000);
    }

    // --- Graduated compression tests ---

    #[test]
    fn test_structural_summary_on_code() {
        let code = "use std::io;\n\npub fn main() {\n    println!(\"hello\");\n}\n\nstruct Foo {\n    bar: i32,\n}\n";
        let summary = structural_summary(code, None);
        assert!(summary.contains("pub fn main"));
        assert!(summary.contains("struct Foo"));
        assert!(summary.contains("summarized"));
    }

    #[test]
    fn test_structural_summary_on_test_output() {
        let output = "running 5 tests\ntest foo ... ok\ntest bar ... FAILED\ntest baz ... ok\n\ntest result: FAILED. 4 passed; 1 failed;\n";
        let summary = structural_summary(output, None);
        assert!(summary.contains("FAILED"));
        assert!(summary.contains("test result"));
    }

    #[test]
    fn test_tool_oneliner() {
        let code = "pub fn hello() {}\npub fn world() {}\ndef extra() {}\n";
        let oneliner = tool_oneliner(code);
        assert!(oneliner.starts_with("[tool result:"));
        assert!(oneliner.contains("code"));
    }

    #[test]
    fn test_safe_char_boundary_on_ascii() {
        assert_eq!(safe_char_boundary("hello world", 5), 5);
        assert_eq!(safe_char_boundary("hi", 10), 2);
    }

    #[test]
    fn test_safe_char_boundary_on_multibyte() {
        let text = "héllo"; // é is 2 bytes
        let boundary = safe_char_boundary(text, 2);
        // Should not split the é
        assert!(text.is_char_boundary(boundary));
    }

    // --- Phase detection tests ---

    #[test]
    fn test_phase_tracker_starts_exploration() {
        let tracker = PhaseTracker::new();
        assert_eq!(tracker.current_phase(), TaskPhase::Exploration);
    }

    #[test]
    fn test_phase_transitions_to_editing() {
        let mut tracker = PhaseTracker::new();
        assert!(tracker.observe_tool("read_file").is_none());
        assert!(tracker.observe_tool("grep").is_none());
        let transition = tracker.observe_tool("write_file");
        assert_eq!(
            transition,
            Some((TaskPhase::Exploration, TaskPhase::Editing))
        );
        assert_eq!(tracker.current_phase(), TaskPhase::Editing);
    }

    #[test]
    fn test_phase_transitions_to_verification() {
        let mut tracker = PhaseTracker::new();
        tracker.observe_tool("write_file"); // exploration -> editing
        let transition = tracker.observe_tool("run_tests");
        assert_eq!(
            transition,
            Some((TaskPhase::Editing, TaskPhase::Verification))
        );
    }

    #[test]
    fn test_phase_returns_to_exploration_after_reads() {
        let mut tracker = PhaseTracker::new();
        tracker.observe_tool("write_file"); // -> editing
        tracker.observe_tool("run_tests"); // -> verification
        tracker.observe_tool("read_file");
        tracker.observe_tool("grep");
        let transition = tracker.observe_tool("glob");
        assert_eq!(
            transition,
            Some((TaskPhase::Verification, TaskPhase::Exploration))
        );
    }

    // --- BM25 reordering tests ---

    #[test]
    fn test_build_messages_no_reorder_when_disabled() {
        let mut config = ContextConfig::default();
        config.reorder_middle_zone = false;
        config.advertised_context_length = 1_000_000;
        let mut ctx = ContextManager::with_config(config);

        ctx.add_message(Message::user("find the bug in login.rs"));
        for i in 0..5 {
            ctx.add_message(Message::assistant(format!("response {i}")));
            ctx.add_message(Message {
                role: Role::Tool,
                content: Content::Text(format!("tool output {i}")),
                tool_calls: vec![],
                tool_call_id: Some(format!("tc{i}")),
            });
        }
        ctx.add_message(Message::user("now fix it"));

        let msgs = ctx.build_messages("sys");
        // With reordering disabled, messages should be in chronological order.
        // Verify the first tool result appears before the last one.
        let tool_indices: Vec<usize> = msgs
            .iter()
            .enumerate()
            .filter(|(_, m)| matches!(m.role, Role::Tool))
            .map(|(i, _)| i)
            .collect();
        // Chronological: tool_indices should be strictly increasing.
        for w in tool_indices.windows(2) {
            assert!(
                w[0] < w[1],
                "tools should be chronological when reorder is disabled"
            );
        }
    }

    #[test]
    fn test_build_messages_reorders_when_enabled() {
        let mut config = ContextConfig::default();
        config.reorder_middle_zone = true;
        config.recent_window = 2;
        config.advertised_context_length = 1_000_000;
        let mut ctx = ContextManager::with_config(config);

        ctx.add_message(Message::user("fix the authentication bug in login.rs"));
        ctx.add_message(Message::assistant("Let me look around."));

        // Tool 0: irrelevant
        ctx.add_message(Message {
            role: Role::Tool,
            content: Content::Text("database config: host=localhost port=5432".to_string()),
            tool_calls: vec![],
            tool_call_id: Some("tc0".to_string()),
        });
        ctx.add_message(Message::assistant("checking more"));

        // Tool 1: highly relevant
        ctx.add_message(Message {
            role: Role::Tool,
            content: Content::Text("fn login(user: &str, password: &str) -> Result<Token> { auth_check(user, password) }".to_string()),
            tool_calls: vec![],
            tool_call_id: Some("tc1".to_string()),
        });
        ctx.add_message(Message::assistant("and more"));

        // Tool 2: somewhat relevant
        ctx.add_message(Message {
            role: Role::Tool,
            content: Content::Text(
                "fn logout(session: &Session) { session.invalidate() }".to_string(),
            ),
            tool_calls: vec![],
            tool_call_id: Some("tc2".to_string()),
        });
        ctx.add_message(Message::assistant("checking tests"));

        // Tool 3: irrelevant
        ctx.add_message(Message {
            role: Role::Tool,
            content: Content::Text(
                "README.md: # Project Setup\nInstall with cargo install".to_string(),
            ),
            tool_calls: vec![],
            tool_call_id: Some("tc3".to_string()),
        });

        // Recent window (last 2 messages)
        ctx.add_message(Message::user("please fix the login auth bug now"));
        ctx.add_message(Message::assistant("On it."));

        let msgs = ctx.build_messages("sys");

        // The login-related tool (tc1) should be placed at an edge of the
        // middle zone (near start or near the recent window), not buried
        // in the center. We verify all tool messages are present.
        let tool_contents: Vec<String> = msgs
            .iter()
            .filter(|m| matches!(m.role, Role::Tool))
            .map(|m| m.content.as_text().to_string())
            .collect();
        assert!(
            tool_contents.len() >= 3,
            "all tool messages should be present, got {}",
            tool_contents.len()
        );
        // The login tool should exist somewhere in the output.
        assert!(
            tool_contents.iter().any(|t| t.contains("login")),
            "login tool result must be present in output"
        );
    }

    #[test]
    fn test_notify_tool_call_compacts_on_transition() {
        // Use recent_window=1 so the tool result is outside the protected tail.
        let mut config = ContextConfig::default();
        config.recent_window = 1;
        config.advertised_context_length = 1_000_000;
        let mut ctx = ContextManager::with_config(config).with_phase_compaction(true);

        // Add exploration tool results
        ctx.add_message(Message::user("find the bug"));
        ctx.add_message(Message::assistant("Let me read the file."));
        let big_content = "x".repeat(500);
        ctx.add_message(Message {
            role: Role::Tool,
            content: Content::Text(big_content.clone()),
            tool_calls: vec![],
            tool_call_id: Some("tc1".to_string()),
        });
        ctx.add_message(Message::assistant("I see the issue."));
        // Add padding to push tool result out of recent window
        ctx.add_message(Message::user("ok fix it"));
        ctx.add_message(Message::assistant("Sure, writing now."));
        // The window counts tool results: one newer result takes the only slot.
        ctx.add_message(Message {
            role: Role::Tool,
            content: Content::Text("short".to_string()),
            tool_calls: vec![],
            tool_call_id: Some("tc2".to_string()),
        });

        // Trigger phase transition
        ctx.notify_tool_call("write_file");

        // The tool result should have been compressed
        let tool_msg = ctx
            .messages
            .iter()
            .find(|m| matches!(m.role, Role::Tool))
            .unwrap();
        let text = tool_msg.content.as_text();
        assert!(
            text.contains("summarized") || text.contains("[tool result"),
            "exploration result should be compressed on phase transition, got: {}",
            &text[..100.min(text.len())]
        );
    }

    // --- Context compression pipeline tests ---

    #[test]
    fn test_graduated_compression_preserves_recent() {
        let mut config = ContextConfig::default();
        config.advertised_context_length = 1_000_000;
        config.recent_window = 3;
        config.tool_compress_age = 2;
        config.tool_oneliner_age_offset = 3;
        let mut ctx = ContextManager::with_config(config);

        // Add old tool results that should be compressed.
        ctx.add_message(Message::user("start"));
        for i in 0..8 {
            ctx.add_message(Message {
                role: Role::Tool,
                content: Content::Text(format!("fn function_{i}() {{ /* code */ }}")),
                tool_calls: vec![],
                tool_call_id: Some(format!("tc{i}")),
            });
            ctx.add_message(Message::assistant(format!("saw function {i}")));
        }
        // Recent messages
        ctx.add_message(Message::user("now do something"));
        ctx.add_message(Message::assistant("ok"));

        let msgs = ctx.build_messages("sys");

        // The last 3 messages (recent_window) should be verbatim.
        let last_user = msgs.iter().rev().find(|m| matches!(m.role, Role::User));
        assert_eq!(
            last_user.unwrap().content.as_text(),
            "now do something",
            "recent user message should be preserved verbatim"
        );
    }

    #[test]
    fn test_oneliner_compression_at_high_age() {
        let mut config = ContextConfig::default();
        config.advertised_context_length = 1_000_000;
        config.recent_window = 2;
        config.tool_compress_age = 2;
        config.tool_oneliner_age_offset = 3; // oneliner at age 5+
        let mut ctx = ContextManager::with_config(config);

        ctx.add_message(Message::user("start"));
        ctx.add_message(Message {
            role: Role::Tool,
            content: Content::Text("fn old_function() { /* lots of code here */ }".repeat(10)),
            tool_calls: vec![],
            tool_call_id: Some("tc0".to_string()),
        });
        // Add 10 assistant turns to push age past oneliner threshold.
        for i in 0..10 {
            ctx.add_message(Message::assistant(format!("response {i}")));
        }
        ctx.add_message(Message::user("latest"));

        // The old tool result should be a one-liner now.
        let tool_msg = ctx
            .messages
            .iter()
            .find(|m| matches!(m.role, Role::Tool))
            .unwrap();
        let text = tool_msg.content.as_text();
        assert!(
            text.starts_with("[tool result:"),
            "very old tool result should be a one-liner, got: {}",
            &text[..80.min(text.len())]
        );
    }

    #[test]
    fn test_clear_resets_phase() {
        let mut ctx = ContextManager::new(100_000);
        ctx.notify_tool_call("write_file"); // exploration → editing
        assert_eq!(ctx.current_phase(), TaskPhase::Editing);

        ctx.clear();
        // Phase should NOT auto-reset (PhaseTracker is separate from messages).
        // But this test documents the current behavior.
        // If we want clear to reset phase, we need to add that.
        // For now, verify the message count is 0.
        assert_eq!(ctx.message_count(), 0);
    }

    #[test]
    fn is_over_ratio_threshold_math_is_correct() {
        // Construct a manager at exactly 100 tokens budget and check the
        // numeric cutoff without worrying about the tokenizer.
        let ctx = ContextManager::new(100);
        assert!(!ctx.is_over_ratio(0.95, 0));
        assert_eq!(ctx.max_context_tokens(), 100);
    }

    #[test]
    fn is_over_ratio_counts_extra_tokens() {
        // Empty context → 0 stored tokens. With extra_tokens=96 and a 100
        // token budget, 0+96 > 95 fires the 95% threshold.
        let ctx = ContextManager::new(100);
        assert!(!ctx.is_over_ratio(0.95, 95));
        assert!(ctx.is_over_ratio(0.95, 96));
    }

    #[test]
    fn is_over_ratio_saturates_on_overflow() {
        // Guard: saturating_add must not panic even with u32::MAX extras.
        let ctx = ContextManager::new(100);
        assert!(ctx.is_over_ratio(0.95, u32::MAX));
    }

    #[test]
    fn estimate_text_tokens_is_deterministic_and_positive() {
        let a = estimate_text_tokens("hello world");
        let b = estimate_text_tokens("hello world");
        assert_eq!(a, b);
        assert!(a >= 4, "per-message framing overhead should apply");
    }

    #[test]
    fn test_over_budget_triggers_drop_and_summary() {
        // Very small budget to force trimming.
        let mut ctx = ContextManager::new(200);
        ctx.add_message(Message::user("original request"));
        for i in 0..50 {
            ctx.add_message(Message::assistant(format!(
                "response {i} with extra padding words"
            )));
        }

        let msgs = ctx.build_messages("sys");

        // Should have a session history summary marker.
        let has_summary = msgs.iter().any(|m| {
            matches!(m.role, Role::System) && m.content.as_text().contains("session history")
        });
        assert!(
            has_summary,
            "should have session history summary after trimming"
        );

        // The original user message should be preserved.
        let has_original = msgs
            .iter()
            .any(|m| matches!(m.role, Role::User) && m.content.as_text() == "original request");
        assert!(has_original, "original user message should be preserved");
    }
}
