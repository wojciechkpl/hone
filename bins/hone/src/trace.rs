//! `hone trace` subcommands — read and display hierarchical tracing data from
//! `.hone/audit.jsonl`.

use std::collections::HashMap;
use std::io::{BufRead as _, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use clap::Subcommand;
use hone_core::egress_log::EgressEntry;

// ---------------------------------------------------------------------------
// CLI subcommand definitions
// ---------------------------------------------------------------------------

/// Subcommands for `hone trace`.
#[derive(Subcommand, Debug)]
pub enum TraceCmd {
    /// List recent traces with a summary line each.
    List {
        /// Maximum number of traces to show (most-recent first).
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Show a trace as an indented tree view.
    ///
    /// The `id` argument supports prefix matching — pass as many characters
    /// as needed to uniquely identify the trace.
    Show {
        /// Trace ID or unique prefix.
        id: String,
    },
    /// Show cross-session analytics across all recorded traces.
    Stats,
}

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// Aggregated data for a single trace (one `trace_id` group).
struct TraceSummary {
    trace_id: String,
    /// Timestamp of the first entry (ISO-8601 string from the log).
    first_ts: String,
    entries: Vec<EgressEntry>,
}

impl TraceSummary {
    fn tool_count(&self) -> usize {
        self.entries.len()
    }

    fn total_duration_ms(&self) -> u64 {
        self.entries.iter().map(|e| e.duration_ms).sum()
    }

    /// Number of distinct `parent_span_id` values, used as a turn proxy.
    fn turn_count(&self) -> usize {
        let mut parents: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for e in &self.entries {
            if let Some(p) = &e.parent_span_id {
                parents.insert(p.as_str());
            }
        }
        parents.len().max(1)
    }
}

// ---------------------------------------------------------------------------
// JSONL loading
// ---------------------------------------------------------------------------

/// Load all entries from `audit.jsonl`, returning them in file order.
///
/// Lines that cannot be deserialized (e.g. written by an older version without
/// the span fields) are silently skipped; this keeps the CLI forward- and
/// backward-compatible.
fn load_entries(audit_path: &Path) -> Result<Vec<EgressEntry>> {
    if !audit_path.exists() {
        return Ok(Vec::new());
    }

    let file = std::fs::File::open(audit_path)
        .with_context(|| format!("opening {}", audit_path.display()))?;

    let reader = BufReader::new(file);
    let mut entries = Vec::new();

    for line in reader.lines() {
        let line = line.context("reading audit log line")?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<EgressEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(_) => {
                // Skip malformed / legacy entries.
            }
        }
    }

    Ok(entries)
}

/// Group entries by `trace_id`, preserving the order in which each trace was
/// first seen.  Entries with an empty `trace_id` are collected under `""`.
fn group_by_trace(entries: Vec<EgressEntry>) -> Vec<TraceSummary> {
    let mut order: Vec<String> = Vec::new();
    let mut map: HashMap<String, (String, Vec<EgressEntry>)> = HashMap::new();

    for entry in entries {
        let tid = entry.trace_id.clone();
        if !map.contains_key(&tid) {
            order.push(tid.clone());
            map.insert(tid.clone(), (entry.timestamp.clone(), Vec::new()));
        }
        map.get_mut(&tid).unwrap().1.push(entry);
    }

    order
        .into_iter()
        .map(|tid| {
            let (first_ts, entries) = map.remove(&tid).unwrap();
            TraceSummary {
                trace_id: tid,
                first_ts,
                entries,
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Display helpers
// ---------------------------------------------------------------------------

/// Return the display icon for a given tool name.
pub fn tool_icon(name: &str) -> &'static str {
    match name {
        "read_file" => "✦",
        "edit_file" | "write_file" | "multi_edit" => "✎",
        "shell" => "❯",
        "grep" => "✦",
        "glob" => "✦",
        "git_commit" => "⎇",
        "git_status" | "git_diff" => "⎇",
        "lsp_diagnostics" | "lsp_hover" | "lsp_code_actions" => "◈",
        "symbol_search" | "symbol_at" => "◈",
        _ => "·",
    }
}

/// Format a duration in milliseconds as a human-readable string.
fn fmt_duration(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", ms as f64 / 1_000.0)
    }
}

/// Shorten a UUID-v7 string to the first 8 hex characters for display.
fn short_id(id: &str) -> &str {
    let end = id.len().min(8);
    &id[..end]
}

/// Format a timestamp string `"2026-04-09T14:30:00Z"` as `"04-09 14:30"`.
fn fmt_ts_short(ts: &str) -> String {
    // ts is an RFC-3339-ish string: YYYY-MM-DDTHH:MM:SSZ
    if ts.len() >= 16 {
        // "MM-DD HH:MM"
        format!("{} {}", &ts[5..10], &ts[11..16])
    } else {
        ts.to_owned()
    }
}

// ---------------------------------------------------------------------------
// `hone trace list`
// ---------------------------------------------------------------------------

pub fn cmd_list(audit_path: &Path, limit: usize) -> Result<()> {
    let entries = load_entries(audit_path)?;

    if entries.is_empty() {
        println!("No audit log entries found at {}.", audit_path.display());
        println!("Enable with --audit or HONE_AUDIT_LOG=1.");
        return Ok(());
    }

    let mut traces = group_by_trace(entries);

    // Most-recent traces first.
    traces.reverse();
    traces.truncate(limit);

    println!(
        "{:<12}  {:<11}  {:>5}  {:>5}  {:>8}",
        "TRACE ID", "DATE", "TURNS", "TOOLS", "DURATION"
    );
    println!("{}", "-".repeat(52));

    for t in &traces {
        println!(
            "{:<12}  {:<11}  {:>5}  {:>5}  {:>8}",
            short_id(&t.trace_id),
            fmt_ts_short(&t.first_ts),
            t.turn_count(),
            t.tool_count(),
            fmt_duration(t.total_duration_ms()),
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// `hone trace show <id>`
// ---------------------------------------------------------------------------

pub fn cmd_show(audit_path: &Path, id_prefix: &str) -> Result<()> {
    let entries = load_entries(audit_path)?;

    if entries.is_empty() {
        println!("No audit log entries found.");
        return Ok(());
    }

    let traces = group_by_trace(entries);

    // Prefix-match on trace_id.
    let matched: Vec<&TraceSummary> = traces
        .iter()
        .filter(|t| t.trace_id.starts_with(id_prefix))
        .collect();

    let trace = match matched.len() {
        0 => {
            anyhow::bail!("No trace found with prefix '{id_prefix}'");
        }
        1 => matched[0],
        n => {
            anyhow::bail!(
                "{n} traces match prefix '{id_prefix}' — provide more characters to disambiguate"
            );
        }
    };

    print_trace_tree(trace);
    Ok(())
}

/// Render the trace as a tree, grouping tool calls by `parent_span_id` (turn).
fn print_trace_tree(trace: &TraceSummary) {
    let total_ms = trace.total_duration_ms();

    println!(
        "Session {} ({} tools, {})",
        short_id(&trace.trace_id),
        trace.tool_count(),
        fmt_duration(total_ms),
    );

    // Group entries by parent_span_id to reconstruct turns.
    // Entries without a parent_span_id get their own implicit group.
    let mut turn_order: Vec<Option<String>> = Vec::new();
    let mut turn_map: HashMap<Option<String>, Vec<&EgressEntry>> = HashMap::new();

    for entry in &trace.entries {
        let key = entry.parent_span_id.clone();
        if !turn_map.contains_key(&key) {
            turn_order.push(key.clone());
            turn_map.insert(key.clone(), Vec::new());
        }
        turn_map.get_mut(&key).unwrap().push(entry);
    }

    let n_turns = turn_order.len();

    for (turn_idx, parent_key) in turn_order.iter().enumerate() {
        let calls = &turn_map[parent_key];
        let turn_ms: u64 = calls.iter().map(|e| e.duration_ms).sum();
        let is_last_turn = turn_idx + 1 == n_turns;

        let turn_connector = if is_last_turn {
            "└──"
        } else {
            "├──"
        };
        let turn_child_prefix = if is_last_turn { "    " } else { "│   " };

        println!(
            "{} Turn {} ({})",
            turn_connector,
            turn_idx + 1,
            fmt_duration(turn_ms),
        );

        let n_calls = calls.len();
        for (call_idx, entry) in calls.iter().enumerate() {
            let is_last_call = call_idx + 1 == n_calls;
            let call_connector = if is_last_call {
                "└──"
            } else {
                "├──"
            };
            let icon = tool_icon(&entry.tool);
            println!(
                "{}{} {} {} ({})",
                turn_child_prefix,
                call_connector,
                icon,
                entry.tool,
                fmt_duration(entry.duration_ms),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// `hone trace stats`
// ---------------------------------------------------------------------------

pub fn cmd_stats(audit_path: &Path) -> Result<()> {
    let entries = load_entries(audit_path)?;

    if entries.is_empty() {
        println!("No audit log entries found.");
        return Ok(());
    }

    let traces = group_by_trace(entries.clone());
    let session_count = traces.len();
    let total_tools: usize = traces.iter().map(|t| t.tool_count()).sum();

    // Per-tool aggregation.
    #[derive(Default)]
    struct ToolStats {
        calls: u64,
        total_ms: u64,
    }

    let mut tool_stats: HashMap<String, ToolStats> = HashMap::new();

    for entry in &entries {
        let s = tool_stats.entry(entry.tool.clone()).or_default();
        s.calls += 1;
        s.total_ms += entry.duration_ms;
    }

    // Sort tools by call count descending.
    let mut tool_rows: Vec<(String, ToolStats)> = tool_stats.into_iter().collect();
    tool_rows.sort_by(|a, b| b.1.calls.cmp(&a.1.calls));

    println!("SESSIONS: {session_count}  |  TOTAL TOOLS: {total_tools}");
    println!();
    println!("Tool Usage:");

    let col_w = tool_rows
        .iter()
        .map(|(n, _)| n.len())
        .max()
        .unwrap_or(8)
        .max(8);

    println!(
        "  {:<col_w$}  {:>8}  {:>8}  {:>10}",
        "TOOL", "CALLS", "AVG", "TOTAL"
    );
    println!("  {}", "-".repeat(col_w + 32));

    for (name, stats) in &tool_rows {
        let avg_ms = if stats.calls > 0 {
            stats.total_ms / stats.calls
        } else {
            0
        };
        println!(
            "  {:<col_w$}  {:>8}  {:>8}  {:>10}",
            name,
            stats.calls,
            fmt_duration(avg_ms),
            fmt_duration(stats.total_ms),
        );
    }

    // Top traces by tool count (proxy for session size).
    println!();
    println!("Top Sessions by Tool Count:");

    let mut sorted_traces = traces;
    sorted_traces.sort_by(|a, b| b.tool_count().cmp(&a.tool_count()));
    sorted_traces.truncate(5);

    for t in &sorted_traces {
        println!(
            "  {}  {:>5} tools  {}",
            short_id(&t.trace_id),
            t.tool_count(),
            fmt_duration(t.total_duration_ms()),
        );
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hone_core::egress_log::SpanKind;
    use tempfile::TempDir;

    fn make_entry(tool: &str, trace_id: &str, parent: Option<&str>) -> EgressEntry {
        EgressEntry {
            timestamp: "2026-04-09T14:30:00Z".to_owned(),
            tool: tool.to_owned(),
            arguments: serde_json::json!({}),
            result_preview: String::new(),
            blocked: false,
            block_reason: None,
            injection_risk: None,
            duration_ms: 10,
            trace_id: trace_id.to_owned(),
            span_id: uuid::Uuid::now_v7().to_string(),
            parent_span_id: parent.map(str::to_owned),
            span_kind: SpanKind::ToolCall,
        }
    }

    fn write_jsonl(dir: &TempDir, entries: &[EgressEntry]) -> std::path::PathBuf {
        let path = dir.path().join("audit.jsonl");
        let mut out = String::new();
        for e in entries {
            out.push_str(&serde_json::to_string(e).unwrap());
            out.push('\n');
        }
        std::fs::write(&path, out).unwrap();
        path
    }

    #[test]
    fn group_by_trace_preserves_insertion_order() {
        let entries = vec![
            make_entry("grep", "trace-a", None),
            make_entry("read_file", "trace-b", None),
            make_entry("edit_file", "trace-a", None),
        ];
        let groups = group_by_trace(entries);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].trace_id, "trace-a");
        assert_eq!(groups[0].entries.len(), 2);
        assert_eq!(groups[1].trace_id, "trace-b");
    }

    #[test]
    fn load_entries_skips_malformed_lines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("audit.jsonl");
        let good = make_entry("shell", "t1", None);
        let good_line = serde_json::to_string(&good).unwrap();
        std::fs::write(&path, format!("{good_line}\nnot valid json\n{good_line}\n")).unwrap();
        let entries = load_entries(&path).unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn load_entries_returns_empty_when_file_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.jsonl");
        let entries = load_entries(&path).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn cmd_list_prints_without_panicking() {
        let tmp = TempDir::new().unwrap();
        let turn_span = "turn-span-1";
        let entries = vec![
            make_entry("grep", "trace-abc", Some(turn_span)),
            make_entry("edit_file", "trace-abc", Some(turn_span)),
            make_entry("shell", "trace-xyz", None),
        ];
        let path = write_jsonl(&tmp, &entries);
        // Should not return an error.
        cmd_list(&path, 20).unwrap();
    }

    #[test]
    fn cmd_show_prefix_match() {
        let tmp = TempDir::new().unwrap();
        let turn_span = "parent-span";
        let entries = vec![
            make_entry("read_file", "abcdef-trace", Some(turn_span)),
            make_entry("shell", "xyz-trace", None),
        ];
        let path = write_jsonl(&tmp, &entries);
        // Prefix "abc" should match exactly one trace.
        cmd_show(&path, "abc").unwrap();
    }

    #[test]
    fn cmd_show_ambiguous_prefix_errors() {
        let tmp = TempDir::new().unwrap();
        let entries = vec![
            make_entry("read_file", "abcdef1", None),
            make_entry("shell", "abcdef2", None),
        ];
        let path = write_jsonl(&tmp, &entries);
        let result = cmd_show(&path, "abc");
        assert!(result.is_err());
    }

    #[test]
    fn cmd_stats_does_not_panic() {
        let tmp = TempDir::new().unwrap();
        let entries = vec![
            make_entry("read_file", "t1", None),
            make_entry("read_file", "t1", None),
            make_entry("shell", "t2", None),
        ];
        let path = write_jsonl(&tmp, &entries);
        cmd_stats(&path).unwrap();
    }

    #[test]
    fn tool_icon_returns_expected_symbols() {
        assert_eq!(tool_icon("read_file"), "✦");
        assert_eq!(tool_icon("edit_file"), "✎");
        assert_eq!(tool_icon("shell"), "❯");
        assert_eq!(tool_icon("git_commit"), "⎇");
        assert_eq!(tool_icon("unknown_tool"), "·");
    }

    #[test]
    fn fmt_duration_formats_correctly() {
        assert_eq!(fmt_duration(0), "0ms");
        assert_eq!(fmt_duration(500), "500ms");
        assert_eq!(fmt_duration(999), "999ms");
        assert_eq!(fmt_duration(1000), "1.0s");
        assert_eq!(fmt_duration(45_000), "45.0s");
    }

    #[test]
    fn trace_summary_turn_count_uses_distinct_parents() {
        let t = TraceSummary {
            trace_id: "t".into(),
            first_ts: "2026-04-09T00:00:00Z".into(),
            entries: vec![
                make_entry("grep", "t", Some("turn-1")),
                make_entry("read_file", "t", Some("turn-1")),
                make_entry("shell", "t", Some("turn-2")),
            ],
        };
        assert_eq!(t.turn_count(), 2);
    }
}
