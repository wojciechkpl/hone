#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! T2 of `docs/plans/audit-timestamps-spec.md`: `hone trace list` and `stats` keep
//! reading audit files written by the old timestamp function (month 13, February 30):
//! the session is listed with its stored text and nothing aborts.

use std::process::{Command, Stdio};

fn line(trace: &str, ts: &str, n: usize) -> String {
    serde_json::json!({
        "timestamp": ts, "tool": "read_file", "arguments": {}, "result_preview": "",
        "blocked": false, "block_reason": null, "injection_risk": null, "duration_ms": 10,
        "trace_id": trace, "span_id": format!("{trace}-span-{n}"),
        "parent_span_id": format!("{trace}-turn"), "span_kind": "tool_call",
    })
    .to_string()
}

fn run(sub: &str) -> (i32, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    std::fs::create_dir_all(dir.join(".hone")).unwrap();
    let lines = [
        line("alpha-month13", "2026-13-05T10:00:00Z", 1),
        line("bravo-feb30", "2026-02-30T10:00:00Z", 2),
        line("charlie-real", "2026-11-12T10:00:00Z", 3),
    ];
    std::fs::write(dir.join(".hone/audit.jsonl"), lines.join("\n") + "\n").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_hone"))
        .args(["trace", sub])
        .env_clear()
        .env("HOME", &dir)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn test_t2_trace_list_should_show_sessions_with_old_bad_timestamps() {
    let (code, out, err) = run("list");

    assert_eq!(code, 0, "{err}");
    for id in ["alpha-mo", "bravo-fe", "charlie-"] {
        assert!(out.contains(id), "{id} listed:\n{out}");
    }
    assert!(out.contains("13-05"), "the stored text is shown:\n{out}");
    assert!(out.contains("02-30"), "{out}");
}

#[test]
fn test_t2_trace_stats_should_count_sessions_with_old_bad_timestamps() {
    let (code, out, err) = run("stats");

    assert_eq!(code, 0, "{err}");
    assert!(out.contains("SESSIONS: 3"), "{out}");
}
