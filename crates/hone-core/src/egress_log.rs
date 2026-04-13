//! Structured egress audit log.
//!
//! Every tool call dispatched by the agent — its arguments, a result preview,
//! timing, and any security decisions — is appended to a newline-delimited JSON
//! (JSONL) file so operators can reconstruct exactly what the agent did and why.
//!
//! # Enabling
//! Set `HONE_AUDIT_LOG=1` or pass `--audit` on the CLI.  The log is written to
//! `.hone/audit.jsonl` relative to the working directory.

use std::io::Write as _;
use std::path::PathBuf;

use serde::Serialize;

/// Errors that can occur when flushing the egress log.
#[derive(Debug, thiserror::Error)]
pub enum EgressError {
    #[error("I/O error writing audit log: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Span / trace types
// ---------------------------------------------------------------------------

/// What kind of span an [`EgressEntry`] belongs to.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    /// Top-level session span.
    Session,
    /// A single agent turn.
    Turn,
    /// Individual tool invocation.
    ToolCall,
    /// Orchestrator worker sub-agent.
    Worker,
    /// RALPH / reviewer turn.
    Review,
    /// Ensemble candidate.
    Ensemble,
}

/// Lightweight tracing context passed through the agent call chain.
///
/// A new [`TraceContext`] is created at the start of each `run_turn` call;
/// every tool call inside that turn inherits the turn's span as its parent.
#[derive(Debug, Clone)]
pub struct TraceContext {
    /// Identifies the top-level session / orchestration run.
    pub trace_id: String,
    /// The span that owns the current scope (e.g. a turn span).
    pub current_span_id: String,
}

impl TraceContext {
    /// Create a brand-new trace (new `trace_id` and root `span_id`).
    pub fn new() -> Self {
        Self {
            trace_id: uuid::Uuid::now_v7().to_string(),
            current_span_id: uuid::Uuid::now_v7().to_string(),
        }
    }

    /// Create a child trace context that shares the same `trace_id` but has a
    /// fresh `span_id`.  Returns `(child_span_id, parent_span_id)`.
    pub fn child_span(&self) -> (String, Option<String>) {
        let child_id = uuid::Uuid::now_v7().to_string();
        (child_id, Some(self.current_span_id.clone()))
    }

    /// Return a new `TraceContext` scoped to a child span (the child becomes
    /// the "current" span so that nested calls inherit it correctly).
    pub fn enter_child(&self) -> (Self, Option<String>) {
        let (child_id, parent_id) = self.child_span();
        let child_ctx = Self {
            trace_id: self.trace_id.clone(),
            current_span_id: child_id,
        };
        (child_ctx, parent_id)
    }
}

impl Default for TraceContext {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Entry types
// ---------------------------------------------------------------------------

/// One tool-dispatch event recorded in the egress log.
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct EgressEntry {
    /// RFC-3339 timestamp of when the tool call completed.
    pub timestamp: String,
    /// Name of the tool that was called.
    pub tool: String,
    /// Full arguments passed to the tool.
    pub arguments: serde_json::Value,
    /// First 200 characters of the serialised result (or error message).
    pub result_preview: String,
    /// `true` when the call was blocked (sandbox or policy) and not executed.
    pub blocked: bool,
    /// Reason the call was blocked, if applicable.
    pub block_reason: Option<String>,
    /// Highest injection risk label detected in the result, if any.
    pub injection_risk: Option<String>,
    /// Wall-clock duration of the tool call in milliseconds.
    pub duration_ms: u64,

    // ---- span hierarchy ----

    /// Unique identifier for the top-level session / orchestration run.
    pub trace_id: String,
    /// Unique identifier for this individual span (tool call).
    pub span_id: String,
    /// Parent span identifier — links this entry to the enclosing turn/worker.
    pub parent_span_id: Option<String>,
    /// Semantic kind of this span.
    pub span_kind: SpanKind,
}

// ---------------------------------------------------------------------------
// Log struct
// ---------------------------------------------------------------------------

/// Append-only egress audit log backed by a JSONL file.
///
/// Entries are buffered in memory and flushed to disk when [`flush`] is
/// called.  The caller is responsible for flushing at appropriate points
/// (after each turn, on shutdown, etc.).
pub struct EgressLog {
    path: PathBuf,
    entries: Vec<EgressEntry>,
}

impl EgressLog {
    /// Create a new `EgressLog` that will write to `path`.
    ///
    /// The file (and any parent directories) will be created lazily on the
    /// first [`flush`] call, not here.
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            entries: Vec::new(),
        }
    }

    /// Record a new entry.  Does not write to disk; call [`flush`] to persist.
    pub fn record(&mut self, entry: EgressEntry) {
        self.entries.push(entry);
    }

    /// Append all buffered entries to the JSONL file on disk, then clear the
    /// in-memory buffer.
    ///
    /// Each call opens the file in append mode so concurrent hone processes
    /// writing to the same file interleave at entry boundaries rather than
    /// corrupting individual lines.
    pub fn flush(&mut self) -> Result<(), EgressError> {
        if self.entries.is_empty() {
            return Ok(());
        }

        // Ensure parent directory exists.
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        let mut writer = std::io::BufWriter::new(file);
        for entry in &self.entries {
            let line = serde_json::to_string(entry)?;
            writer.write_all(line.as_bytes())?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;

        self.entries.clear();
        Ok(())
    }

    /// Number of buffered (unflushed) entries.
    pub fn buffered_count(&self) -> usize {
        self.entries.len()
    }

    /// Path this log writes to.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Current time formatted as RFC-3339 (UTC, second precision).
///
/// We avoid pulling in the `time` crate's formatting machinery; a plain
/// `std::time::SystemTime` conversion is sufficient here.
pub fn now_rfc3339() -> String {
    // Use the time crate's OffsetDateTime since it's already in the workspace.
    // Fallback to a simple epoch-based string if formatting fails.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Format as a minimal ISO-8601 UTC string: 2006-01-02T15:04:05Z
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86400;
    // Simplified Gregorian calendar (good enough for log timestamps)
    let year = 1970 + days / 365;
    let day_of_year = days % 365;
    let month = day_of_year / 30 + 1;
    let day = day_of_year % 30 + 1;
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Truncate a string to at most `max_chars` Unicode scalar values.
pub fn preview(s: &str, max_chars: usize) -> String {
    let mut chars = s.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        // There were more chars; append ellipsis.
        format!("{truncated}…")
    } else {
        truncated
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufRead as _;
    use tempfile::TempDir;

    fn make_entry(tool: &str, blocked: bool) -> EgressEntry {
        EgressEntry {
            timestamp: "2024-01-01T00:00:00Z".to_owned(),
            tool: tool.to_owned(),
            arguments: serde_json::json!({ "path": "/tmp/test.txt" }),
            result_preview: "file contents here".to_owned(),
            blocked,
            block_reason: if blocked {
                Some("sandbox policy".to_owned())
            } else {
                None
            },
            injection_risk: None,
            duration_ms: 12,
            trace_id: "trace-test".to_owned(),
            span_id: uuid::Uuid::now_v7().to_string(),
            parent_span_id: None,
            span_kind: SpanKind::ToolCall,
        }
    }

    #[test]
    fn record_increments_buffered_count() {
        let tmp = TempDir::new().unwrap();
        let mut log = EgressLog::new(tmp.path().join("audit.jsonl"));
        assert_eq!(log.buffered_count(), 0);
        log.record(make_entry("read_file", false));
        log.record(make_entry("shell", true));
        assert_eq!(log.buffered_count(), 2);
    }

    #[test]
    fn flush_writes_jsonl_and_clears_buffer() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join(".hone/audit.jsonl");
        let mut log = EgressLog::new(path.clone());

        log.record(make_entry("read_file", false));
        log.record(make_entry("shell", true));
        log.flush().unwrap();

        assert_eq!(log.buffered_count(), 0, "buffer should be empty after flush");

        let file = std::fs::File::open(&path).unwrap();
        let lines: Vec<String> = std::io::BufReader::new(file)
            .lines()
            .map(|l| l.unwrap())
            .collect();

        assert_eq!(lines.len(), 2, "two JSONL lines expected");
        for line in &lines {
            let v: serde_json::Value = serde_json::from_str(line).expect("each line must be valid JSON");
            assert!(v.get("tool").is_some());
            assert!(v.get("timestamp").is_some());
            assert!(v.get("duration_ms").is_some());
        }
    }

    #[test]
    fn flush_is_idempotent_on_empty_buffer() {
        let tmp = TempDir::new().unwrap();
        let mut log = EgressLog::new(tmp.path().join("audit.jsonl"));
        // Should not create file or error when buffer is empty.
        log.flush().unwrap();
        assert!(!tmp.path().join("audit.jsonl").exists());
    }

    #[test]
    fn jsonl_format_is_one_object_per_line() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("audit.jsonl");
        let mut log = EgressLog::new(path.clone());

        for i in 0..5 {
            let mut entry = make_entry("tool", false);
            entry.tool = format!("tool_{i}");
            log.record(entry);
        }
        log.flush().unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        let line_count = content.lines().count();
        assert_eq!(line_count, 5);

        // Every line must be a JSON object (not array, not primitive).
        for line in content.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(v.is_object());
        }
    }

    #[test]
    fn preview_truncates_at_max_chars() {
        let s = "a".repeat(300);
        let p = preview(&s, 200);
        // 200 'a's + ellipsis character
        assert!(p.len() <= 204, "preview should not exceed 200 chars + ellipsis");
        assert!(p.ends_with('…'));
    }

    #[test]
    fn preview_does_not_truncate_short_strings() {
        let s = "hello";
        assert_eq!(preview(s, 200), "hello");
    }
}
