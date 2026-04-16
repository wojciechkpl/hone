//! Per-turn observation ledger.
//!
//! Every `TurnLedger` accumulates events that occur within a single agent turn:
//! file reads/writes/execs, injected skeleton context, memory recalls, tool
//! invocations, and phase transitions.  At turn end the ledger is sealed and
//! appended to `App::completed_turns`.  The current in-progress turn lives in
//! `App::current_turn`.
//!
//! The ledger is the data source for the Agent Rail widget and the session
//! report.  It is intentionally kept TUI-side (no hone-core changes) so it can
//! be compiled out of the server/headless builds.

use std::ops::Range;
use std::time::Instant;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// File touch
// ---------------------------------------------------------------------------

/// The operation performed on a file during a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOp {
    Read,
    Write,
    Exec,
}

impl FileOp {
    /// Short badge used in the Agent Rail (unicode).
    pub fn badge(&self) -> &'static str {
        match self {
            FileOp::Read => "✦",
            FileOp::Write => "✎",
            FileOp::Exec => "❯",
        }
    }

    /// ASCII fallback badge for NO_COLOR mode.
    pub fn badge_ascii(&self) -> &'static str {
        match self {
            FileOp::Read => "[R]",
            FileOp::Write => "[W]",
            FileOp::Exec => "[X]",
        }
    }
}

fn instant_now() -> Instant {
    Instant::now()
}

/// A file that was touched during this turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileTouch {
    /// Path relative to the project root (or full path if not relative-able).
    pub path: String,
    /// The last operation performed on this file this turn.
    pub op: FileOp,
    /// Optional line range (0-indexed, exclusive end), e.g. `45..60`.
    pub line_range: Option<Range<usize>>,
    #[serde(skip, default = "instant_now")]
    pub timestamp: Instant,
}

// ---------------------------------------------------------------------------
// Skeleton injection
// ---------------------------------------------------------------------------

/// A file injected via the skeleton / BM25 context injection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonEntry {
    pub path: String,
    pub strategy: String,
    /// BM25 relevance score (None for tier-1 skeleton entries).
    pub bm25_score: Option<f32>,
    /// Approximate token count injected for this file.
    pub tokens: u32,
}

// ---------------------------------------------------------------------------
// Memory recall
// ---------------------------------------------------------------------------

/// A memory entry recalled from the memory store this turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryRecall {
    pub key: String,
    /// Short excerpt of the memory content (≤ 80 chars).
    pub snippet: String,
    #[serde(skip, default = "instant_now")]
    pub timestamp: Instant,
}

// ---------------------------------------------------------------------------
// Tool invocation
// ---------------------------------------------------------------------------

/// A tool call that fired during this turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub name: String,
    pub duration_ms: u64,
    pub cache_hit: bool,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Phase
// ---------------------------------------------------------------------------

/// High-level agent phase, mirrors StatusInfo::phase labels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    Exploration,
    Editing,
    Verification,
    Planning,
    Complete,
    Other(String),
}

impl Phase {
    pub fn from_label(label: &str) -> Self {
        match label {
            "Exploration" => Phase::Exploration,
            "Editing" => Phase::Editing,
            "Verification" => Phase::Verification,
            "Planning" => Phase::Planning,
            "Complete" => Phase::Complete,
            other => Phase::Other(other.to_string()),
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Phase::Exploration => "Exploration",
            Phase::Editing => "Editing",
            Phase::Verification => "Verification",
            Phase::Planning => "Planning",
            Phase::Complete => "Complete",
            Phase::Other(s) => s.as_str(),
        }
    }
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

/// Lightweight aggregate counts, suitable for display in compact widgets.
#[derive(Debug, Clone, Default)]
pub struct LedgerSummary {
    pub files_touched: usize,
    pub read_ops: usize,
    pub write_ops: usize,
    pub exec_ops: usize,
    pub skeleton_entries: usize,
    pub memory_recalls: usize,
    pub tool_invocations: usize,
    pub errors: usize,
}

// ---------------------------------------------------------------------------
// TurnLedger
// ---------------------------------------------------------------------------

/// Accumulates observability data for a single agent turn.
///
/// Use `TurnLedger::new()` at turn start and call `record_*` methods as
/// events arrive.  Call `summary()` for lightweight aggregate counts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnLedger {
    /// Monotonic turn index within the session (0-based).
    pub turn_index: usize,
    /// Files touched this turn, keyed by path (last-write-wins for op).
    pub files_touched: Vec<FileTouch>,
    pub skeleton_injections: Vec<SkeletonEntry>,
    pub memory_recalls: Vec<MemoryRecall>,
    pub tools_invoked: Vec<ToolInvocation>,
    /// Phase transitions: (phase, elapsed_ms_since_turn_start).
    pub phase_transitions: Vec<(Phase, u64)>,
    /// Millisecond timestamp at turn start (for relative offsets).
    #[serde(skip, default = "instant_now")]
    turn_start: Instant,
}

impl TurnLedger {
    /// Create a fresh ledger for a new turn.
    pub fn new(turn_index: usize) -> Self {
        Self {
            turn_index,
            files_touched: Vec::new(),
            skeleton_injections: Vec::new(),
            memory_recalls: Vec::new(),
            tools_invoked: Vec::new(),
            phase_transitions: Vec::new(),
            turn_start: Instant::now(),
        }
    }

    // ------------------------------------------------------------------
    // Record* methods
    // ------------------------------------------------------------------

    /// Record a tool invocation.  Call once per ToolCallEnd event.
    pub fn record_tool_call(
        &mut self,
        name: impl Into<String>,
        duration_ms: u64,
        cache_hit: bool,
        error: Option<String>,
    ) {
        self.tools_invoked.push(ToolInvocation {
            name: name.into(),
            duration_ms,
            cache_hit,
            error,
        });
    }

    /// Record a file operation.  If the path already exists in the ledger
    /// the entry is updated in-place (last operation wins the badge; the line
    /// range is replaced).
    pub fn record_file_touch(
        &mut self,
        path: impl Into<String>,
        op: FileOp,
        line_range: Option<Range<usize>>,
    ) {
        let path = path.into();
        if let Some(existing) = self.files_touched.iter_mut().find(|f| f.path == path) {
            existing.op = op;
            existing.line_range = line_range;
            existing.timestamp = Instant::now();
        } else {
            self.files_touched.push(FileTouch {
                path,
                op,
                line_range,
                timestamp: Instant::now(),
            });
        }
    }

    /// Record a skeleton / BM25 injection entry.
    pub fn record_skeleton_entry(
        &mut self,
        path: impl Into<String>,
        strategy: impl Into<String>,
        bm25_score: Option<f32>,
        tokens: u32,
    ) {
        self.skeleton_injections.push(SkeletonEntry {
            path: path.into(),
            strategy: strategy.into(),
            bm25_score,
            tokens,
        });
    }

    /// Record a memory recall.
    pub fn record_memory_recall(&mut self, key: impl Into<String>, snippet: impl Into<String>) {
        let snippet = snippet.into();
        let snippet = if snippet.len() > 80 {
            format!("{}…", &snippet[..79])
        } else {
            snippet
        };
        self.memory_recalls.push(MemoryRecall {
            key: key.into(),
            snippet,
            timestamp: Instant::now(),
        });
    }

    /// Record a phase transition.
    pub fn record_phase_transition(&mut self, phase: Phase) {
        let elapsed = self.turn_start.elapsed().as_millis() as u64;
        self.phase_transitions.push((phase, elapsed));
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// Return aggregate counts without cloning the full ledger.
    pub fn summary(&self) -> LedgerSummary {
        let read_ops = self.files_touched.iter().filter(|f| f.op == FileOp::Read).count();
        let write_ops = self.files_touched.iter().filter(|f| f.op == FileOp::Write).count();
        let exec_ops = self.files_touched.iter().filter(|f| f.op == FileOp::Exec).count();
        let errors = self.tools_invoked.iter().filter(|t| t.error.is_some()).count();
        LedgerSummary {
            files_touched: self.files_touched.len(),
            read_ops,
            write_ops,
            exec_ops,
            skeleton_entries: self.skeleton_injections.len(),
            memory_recalls: self.memory_recalls.len(),
            tool_invocations: self.tools_invoked.len(),
            errors,
        }
    }

    /// Return the top-N skeleton entries sorted by BM25 score (descending),
    /// with un-scored entries appearing last.
    pub fn top_skeleton(&self, n: usize) -> Vec<&SkeletonEntry> {
        let mut entries: Vec<&SkeletonEntry> = self.skeleton_injections.iter().collect();
        entries.sort_by(|a, b| {
            let sa = a.bm25_score.unwrap_or(-1.0);
            let sb = b.bm25_score.unwrap_or(-1.0);
            sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.truncate(n);
        entries
    }
}

// ---------------------------------------------------------------------------
// AgentEvent → FileOp mapping helpers
// ---------------------------------------------------------------------------

/// Classify a tool name into a `FileOp`, returning `None` for tools that do
/// not touch files (e.g. git_status, lsp_hover).
pub fn tool_op(tool_name: &str) -> Option<FileOp> {
    let name = tool_name.to_ascii_lowercase();
    if name.contains("write") || name.contains("edit") || name.contains("multi_edit")
        || name.contains("create")
    {
        Some(FileOp::Write)
    } else if name.contains("read") || name.contains("grep") || name.contains("glob")
        || name.contains("search") || name.contains("find") || name.contains("lsp")
        || name.contains("symbol") || name.contains("git_diff") || name.contains("git_status")
    {
        Some(FileOp::Read)
    } else if name.contains("shell") || name.contains("bash") || name.contains("exec")
        || name.contains("run") || name.contains("cargo") || name.contains("pytest")
        || name.contains("git_commit")
    {
        Some(FileOp::Exec)
    } else {
        None
    }
}

/// Extract a file path from tool arguments JSON, if present.
///
/// Checks common argument names: `path`, `file_path`, `filepath`, `filename`.
pub fn extract_path(args: &serde_json::Value) -> Option<String> {
    let keys = ["path", "file_path", "filepath", "filename", "target"];
    for key in keys {
        if let Some(v) = args.get(key).and_then(|v| v.as_str()) {
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Strip the current working directory prefix from a path for display.
pub fn strip_cwd(path: &str) -> String {
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(stripped) = std::path::Path::new(path).strip_prefix(&cwd) {
            return stripped.to_string_lossy().into_owned();
        }
    }
    path.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_ledger() -> TurnLedger {
        TurnLedger::new(0)
    }

    #[test]
    fn record_tool_call_appends_entry() {
        let mut l = make_ledger();
        l.record_tool_call("read_file", 100, false, None);
        assert_eq!(l.tools_invoked.len(), 1);
        assert_eq!(l.tools_invoked[0].name, "read_file");
        assert_eq!(l.tools_invoked[0].duration_ms, 100);
        assert!(!l.tools_invoked[0].cache_hit);
        assert!(l.tools_invoked[0].error.is_none());
    }

    #[test]
    fn record_tool_call_with_error() {
        let mut l = make_ledger();
        l.record_tool_call("shell", 50, false, Some("exit code 1".to_string()));
        assert_eq!(l.tools_invoked[0].error.as_deref(), Some("exit code 1"));
    }

    #[test]
    fn record_file_touch_deduplicates_by_path() {
        let mut l = make_ledger();
        l.record_file_touch("src/auth.rs", FileOp::Read, None);
        l.record_file_touch("src/auth.rs", FileOp::Write, Some(45..60));
        // Should have only ONE entry with updated op.
        assert_eq!(l.files_touched.len(), 1);
        assert_eq!(l.files_touched[0].op, FileOp::Write);
        assert_eq!(l.files_touched[0].line_range, Some(45..60));
    }

    #[test]
    fn record_file_touch_different_paths() {
        let mut l = make_ledger();
        l.record_file_touch("src/auth.rs", FileOp::Read, None);
        l.record_file_touch("src/session.rs", FileOp::Read, None);
        assert_eq!(l.files_touched.len(), 2);
    }

    #[test]
    fn record_skeleton_entry_appends() {
        let mut l = make_ledger();
        l.record_skeleton_entry("src/auth.rs", "CallGraph", Some(0.91), 120);
        assert_eq!(l.skeleton_injections.len(), 1);
        assert!((l.skeleton_injections[0].bm25_score.unwrap() - 0.91).abs() < 1e-4);
    }

    #[test]
    fn record_memory_recall_truncates_long_snippets() {
        let mut l = make_ledger();
        let long = "x".repeat(200);
        l.record_memory_recall("key", long);
        // Snippet should be truncated — at most 79 'x' chars + "…" (3-byte UTF-8).
        let chars: Vec<char> = l.memory_recalls[0].snippet.chars().collect();
        assert!(chars.len() <= 80, "snippet should be truncated to ≤80 chars, got {}", chars.len());
    }

    #[test]
    fn record_phase_transition_appends() {
        let mut l = make_ledger();
        l.record_phase_transition(Phase::Exploration);
        l.record_phase_transition(Phase::Editing);
        assert_eq!(l.phase_transitions.len(), 2);
        assert_eq!(l.phase_transitions[0].0, Phase::Exploration);
    }

    #[test]
    fn summary_counts_correctly() {
        let mut l = make_ledger();
        l.record_file_touch("a.rs", FileOp::Read, None);
        l.record_file_touch("b.rs", FileOp::Write, None);
        l.record_file_touch("c.sh", FileOp::Exec, None);
        l.record_tool_call("read_file", 10, false, None);
        l.record_tool_call("shell", 5, false, Some("err".to_string()));
        l.record_skeleton_entry("a.rs", "Flat", None, 100);
        l.record_memory_recall("k", "v");

        let s = l.summary();
        assert_eq!(s.files_touched, 3);
        assert_eq!(s.read_ops, 1);
        assert_eq!(s.write_ops, 1);
        assert_eq!(s.exec_ops, 1);
        assert_eq!(s.tool_invocations, 2);
        assert_eq!(s.errors, 1);
        assert_eq!(s.skeleton_entries, 1);
        assert_eq!(s.memory_recalls, 1);
    }

    #[test]
    fn top_skeleton_sorts_by_bm25_descending() {
        let mut l = make_ledger();
        l.record_skeleton_entry("low.rs", "BM25", Some(0.35), 50);
        l.record_skeleton_entry("high.rs", "BM25", Some(0.91), 100);
        l.record_skeleton_entry("mid.rs", "BM25", Some(0.74), 80);
        let top = l.top_skeleton(2);
        assert_eq!(top[0].path, "high.rs");
        assert_eq!(top[1].path, "mid.rs");
    }

    #[test]
    fn tool_op_classifies_correctly() {
        assert_eq!(tool_op("read_file"), Some(FileOp::Read));
        assert_eq!(tool_op("write_file"), Some(FileOp::Write));
        assert_eq!(tool_op("edit_file"), Some(FileOp::Write));
        assert_eq!(tool_op("shell"), Some(FileOp::Exec));
        assert_eq!(tool_op("grep"), Some(FileOp::Read));
        assert_eq!(tool_op("git_commit"), Some(FileOp::Exec));
        assert_eq!(tool_op("lsp_hover"), Some(FileOp::Read));
    }

    #[test]
    fn extract_path_finds_common_keys() {
        let args = serde_json::json!({ "path": "src/main.rs" });
        assert_eq!(extract_path(&args), Some("src/main.rs".to_string()));

        let args2 = serde_json::json!({ "file_path": "src/lib.rs" });
        assert_eq!(extract_path(&args2), Some("src/lib.rs".to_string()));

        let args3 = serde_json::json!({ "command": "cargo test" });
        assert_eq!(extract_path(&args3), None);
    }

    #[test]
    fn integration_fake_agent_event_sequence() {
        // Simulate a minimal turn: TurnStart → ToolCallStart (read) →
        // ToolCallEnd → ToolCallStart (write) → ToolCallEnd → Done.
        let mut ledger = TurnLedger::new(0);
        ledger.record_phase_transition(Phase::Exploration);

        // Turn 1: read_file
        let args_read = serde_json::json!({ "path": "src/auth.rs" });
        if let Some(op) = tool_op("read_file") {
            let path = extract_path(&args_read).unwrap_or_default();
            ledger.record_file_touch(&path, op, None);
        }
        ledger.record_tool_call("read_file", 120, false, None);

        // Phase transition
        ledger.record_phase_transition(Phase::Editing);

        // Turn 2: edit_file
        let args_write = serde_json::json!({ "path": "src/auth.rs", "start_line": 45, "end_line": 60 });
        if let Some(op) = tool_op("edit_file") {
            let path = extract_path(&args_write).unwrap_or_default();
            ledger.record_file_touch(&path, op, Some(45..60));
        }
        ledger.record_tool_call("edit_file", 800, false, None);

        let s = ledger.summary();
        // auth.rs touched twice but deduplicated → 1 file
        assert_eq!(s.files_touched, 1);
        assert_eq!(s.write_ops, 1); // last op is Write
        assert_eq!(s.tool_invocations, 2);
        assert_eq!(ledger.phase_transitions.len(), 2);
    }
}
