#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR20 of `docs/plans/context-receipts-spec.md` at the binary: `hone trace
//! stats` and `hone trace list` don't count audit lines with
//! `span_kind: "context"` (CR13 writes one per model call) as tool calls, and
//! show them as a separate "model calls" count per session and in total.
//!
//! Fixture (hand-written `.hone/audit.jsonl` in a temp cwd):
//!   trace "alpha-trace": 2 `read_file` + 3 context lines  (old tool count: 5)
//!   trace "bravo-trace": 1 `shell`     + 6 context lines  (old tool count: 7)
//!   => 3 tools, 9 model calls in total (old total: 12).
//! The counts are chosen so that each is a distinct bare number in the output.

use std::path::PathBuf;
use std::process::{Command, Stdio};

fn line(trace: &str, tool: &str, kind: &str, n: usize) -> String {
    serde_json::json!({
        "timestamp": "2026-04-09T14:30:00Z",
        "tool": tool,
        "arguments": {},
        "result_preview": "",
        "blocked": false,
        "block_reason": null,
        "injection_risk": null,
        "duration_ms": 10,
        "trace_id": trace,
        "span_id": format!("{trace}-span-{n}"),
        "parent_span_id": format!("{trace}-turn"),
        "span_kind": kind,
    })
    .to_string()
}

struct Env {
    _tmp: tempfile::TempDir,
    dir: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    std::fs::create_dir_all(dir.join(".hone")).unwrap();
    let mut lines = Vec::new();
    let mut n = 0;
    for (trace, tool, tools, ctx) in [
        ("alpha-trace", "read_file", 2, 3),
        ("bravo-trace", "shell", 1, 6),
    ] {
        for _ in 0..ctx {
            n += 1;
            lines.push(line(trace, "context_assembled", "context", n));
        }
        for _ in 0..tools {
            n += 1;
            lines.push(line(trace, tool, "tool_call", n));
        }
    }
    std::fs::write(dir.join(".hone/audit.jsonl"), lines.join("\n") + "\n").unwrap();
    Env { _tmp: tmp, dir }
}

fn trace(e: &Env, sub: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_hone"))
        .args(["trace", sub])
        .env_clear()
        .env("HOME", &e.dir)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&e.dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn tokens(l: &str) -> Vec<&str> {
    l.split_whitespace().collect()
}

#[test]
fn test_cr20_stats_should_not_count_context_lines_as_tools() {
    let out = trace(&env(), "stats");

    let header = out.lines().find(|l| l.contains("TOTAL TOOLS")).expect(&out);
    assert!(header.contains("TOTAL TOOLS: 3"), "{header}\n{out}");
    assert!(
        !out.contains("context_assembled"),
        "context lines must not appear in the tool table:\n{out}"
    );
    let rows: Vec<&str> = out.lines().filter(|l| l.contains("read_file")).collect();
    assert_eq!(rows.len(), 1, "{out}");
    assert!(
        tokens(rows[0]).contains(&"2"),
        "read_file called twice: {}",
        rows[0]
    );
}

#[test]
fn test_cr20_stats_should_show_model_calls_in_total_and_per_session() {
    let out = trace(&env(), "stats");

    let totals: Vec<&str> = out
        .lines()
        .filter(|l| l.to_lowercase().contains("model calls") && tokens(l).contains(&"9"))
        .collect();
    assert!(
        !totals.is_empty(),
        "a 'model calls' line with the total 9:\n{out}"
    );
    // Per session (Top Sessions block): alpha 2 tools / 3 model calls, bravo 1 / 6.
    let alpha = out.lines().find(|l| l.contains("alpha-tr")).expect(&out);
    let bravo = out.lines().find(|l| l.contains("bravo-tr")).expect(&out);
    for (row, tools, calls, old) in [(alpha, "2", "3", "5"), (bravo, "1", "6", "7")] {
        let t = tokens(row);
        assert!(t.contains(&tools) && t.contains(&calls), "{row}");
        assert!(
            !t.contains(&old),
            "context lines still counted as tools: {row}"
        );
    }
}

#[test]
fn test_cr20_list_should_split_tools_from_model_calls() {
    let out = trace(&env(), "list");

    assert!(
        out.to_lowercase().contains("model calls"),
        "a 'model calls' column header:\n{out}"
    );
    let alpha = out.lines().find(|l| l.contains("alpha-tr")).expect(&out);
    let bravo = out.lines().find(|l| l.contains("bravo-tr")).expect(&out);
    for (row, tools, calls, old) in [(alpha, "2", "3", "5"), (bravo, "1", "6", "7")] {
        let t = tokens(row);
        assert!(t.contains(&tools) && t.contains(&calls), "{row}");
        assert!(
            !t.contains(&old),
            "context lines still counted as tools: {row}"
        );
    }
}
