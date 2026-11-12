#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, dead_code)]
//! CR12 of `docs/plans/context-receipts-spec.md`, the renderer behind `hone
//! context show`, as a pure function.
//!
//! API required (new module `bins/hone/src/context_cmd.rs`; it may use
//! `crate::session::StoredReceipt` only):
//!
//! ```ignore
//! /// `run`/`turn` are the `--run`/`--turn` filters; `json` is `--json`.
//! /// - default: every receipt, ordered by (run, seq);
//! /// - `turn` without `run` means the latest run in `stored`;
//! /// - an unknown run, or a turn not in the selected run, is an `Err` whose
//! ///   message names it ("run 9" / "turn 9");
//! /// - no receipts at all: `Ok` of ONE line saying there are no context receipts
//! ///   (case-insensitively contains "no context receipts");
//! /// - `json`: a JSON array of `{ "run": .., "seq": .., "receipt": {..} }`.
//! /// - text, per receipt: a header with `run R`, `turn T`, `call C`, the model, the
//! ///   FIRST 12 hex digits of `request_sha256` (never the full hash), then each block
//! ///   (slot, strategy name, tokens, sources with scores), dropped sources and
//! ///   compaction steps (strategy name and the replaced/derived refs).
//! pub fn render_context_show(stored: &[StoredReceipt], run: Option<u32>,
//!     turn: Option<u32>, json: bool) -> anyhow::Result<String>;
//! ```
//!
//! `bins/hone` has no lib target: the modules are linked with `#[path]`, as other
//! tests of this crate do.

#[path = "../src/context_cmd.rs"]
mod context_cmd;
#[path = "../src/session.rs"]
mod session;

use hone_proto::{
    BlockReceipt, CompactionStep, ContextReceipt, ScoredSource, Slot, SourceRef, StrategyId,
};
use session::StoredReceipt;

fn strategy(slot: Slot, name: &str) -> StrategyId {
    StrategyId {
        slot,
        name: name.to_owned(),
        version: "0.1.0".to_owned(),
        params_sha256: "ab".repeat(32),
    }
}

/// A receipt whose request hash starts with `prefix12` (12 hex digits) and is
/// 64 digits long.
fn receipt(turn: u32, call: u32, prefix12: &str) -> ContextReceipt {
    ContextReceipt {
        turn,
        call,
        model: "mock:test".to_owned(),
        request_sha256: format!("{prefix12}{}", "0".repeat(52)),
        blocks: vec![],
        dropped: vec![],
        compaction: vec![],
        tools_offered: vec![],
        history_len: 0,
        history_root: String::new(),
    }
}

fn stored(run: u32, seq: u32, receipt: ContextReceipt) -> StoredReceipt {
    StoredReceipt { run, seq, receipt }
}

/// run 1: (1,1) (1,2); run 2: (1,1) (2,1).
fn sample() -> Vec<StoredReceipt> {
    vec![
        stored(1, 1, receipt(1, 1, "aaaaaaaaaaaa")),
        stored(1, 2, receipt(1, 2, "bbbbbbbbbbbb")),
        stored(2, 1, receipt(1, 1, "cccccccccccc")),
        stored(2, 2, receipt(2, 1, "dddddddddddd")),
    ]
}

fn render(s: &[StoredReceipt], run: Option<u32>, turn: Option<u32>) -> anyhow::Result<String> {
    context_cmd::render_context_show(s, run, turn, false)
}

#[test]
fn test_cr12_default_should_show_every_run_with_the_hash_prefix_only() {
    let out = render(&sample(), None, None).unwrap();
    for prefix in ["aaaaaaaaaaaa", "bbbbbbbbbbbb", "cccccccccccc", "dddddddddddd"] {
        assert!(out.contains(prefix), "{prefix} missing:\n{out}");
    }
    assert!(
        !out.contains(&format!("aaaaaaaaaaaa{}", "0")),
        "only the first 12 digits:\n{out}"
    );
    assert!(out.contains("run 1") && out.contains("run 2"), "{out}");
    assert!(out.contains("turn 2") && out.contains("call 2"), "{out}");
    assert!(out.contains("mock:test"));
    // Ordered by (run, seq).
    let pos = |p: &str| out.find(p).unwrap();
    assert!(pos("aaaaaaaaaaaa") < pos("bbbbbbbbbbbb"));
    assert!(pos("bbbbbbbbbbbb") < pos("cccccccccccc"));
    assert!(pos("cccccccccccc") < pos("dddddddddddd"));
}

#[test]
fn test_cr12_run_filter_should_show_only_that_run() {
    let out = render(&sample(), Some(1), None).unwrap();
    assert!(out.contains("aaaaaaaaaaaa") && out.contains("bbbbbbbbbbbb"));
    assert!(
        !out.contains("cccccccccccc") && !out.contains("dddddddddddd"),
        "{out}"
    );
}

#[test]
fn test_cr12_turn_without_run_should_mean_the_latest_run() {
    let out = render(&sample(), None, Some(1)).unwrap();
    assert!(out.contains("cccccccccccc"), "run 2, turn 1:\n{out}");
    assert!(
        !out.contains("aaaaaaaaaaaa") && !out.contains("bbbbbbbbbbbb"),
        "{out}"
    );
    assert!(!out.contains("dddddddddddd"), "that is turn 2:\n{out}");
}

#[test]
fn test_cr12_run_and_turn_together_should_select_one_turn_of_one_run() {
    let out = render(&sample(), Some(1), Some(1)).unwrap();
    assert!(out.contains("aaaaaaaaaaaa") && out.contains("bbbbbbbbbbbb"));
    assert!(!out.contains("cccccccccccc"));
}

#[test]
fn test_cr12_unknown_run_or_turn_should_be_an_error_naming_it() {
    let e = render(&sample(), Some(9), None).unwrap_err().to_string();
    assert!(e.contains("run 9"), "{e}");
    let e = render(&sample(), None, Some(9)).unwrap_err().to_string();
    assert!(e.contains("turn 9"), "{e}");
    let e = render(&sample(), Some(1), Some(2)).unwrap_err().to_string();
    assert!(e.contains("turn 2"), "run 1 has no turn 2: {e}");
}

#[test]
fn test_cr12_no_receipts_should_print_a_one_line_note() {
    let out = render(&[], None, None).unwrap();
    assert_eq!(out.trim_end().lines().count(), 1, "{out:?}");
    assert!(out.to_lowercase().contains("no context receipts"), "{out}");
}

#[test]
fn test_cr12_blocks_dropped_sources_and_compaction_should_be_shown() {
    let mut r = receipt(1, 1, "eeeeeeeeeeee");
    r.blocks = vec![BlockReceipt {
        slot: Slot::Recall,
        strategy: strategy(Slot::Recall, "bm25"),
        tokens: 42,
        sources: vec![ScoredSource {
            source: SourceRef::Memory {
                id: "m-recalled".into(),
                version: None,
            },
            score: Some(1.5),
        }],
        duration_ms: 3,
    }];
    r.dropped = vec![SourceRef::Memory {
        id: "m-cut".into(),
        version: None,
    }];
    r.compaction = vec![CompactionStep {
        strategy: strategy(Slot::History, "graduated"),
        replaced: vec![SourceRef::Message {
            index: 4,
            sha256: "01".repeat(32),
        }],
        derived: Some(SourceRef::Message {
            index: 4,
            sha256: "02".repeat(32),
        }),
    }];

    let out = render(&[stored(1, 1, r)], None, None).unwrap();

    for needle in ["recall", "bm25", "42", "m-recalled", "1.5", "m-cut", "graduated"] {
        assert!(out.contains(needle), "{needle} missing:\n{out}");
    }
}

#[test]
fn test_cr12_json_should_be_an_array_of_runs_seqs_and_receipts() {
    let out = context_cmd::render_context_show(&sample(), None, Some(1), true).unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let arr = v.as_array().expect("an array");
    assert_eq!(arr.len(), 1, "latest run, turn 1");
    assert_eq!(arr[0]["run"], 2);
    assert_eq!(arr[0]["seq"], 1);
    assert_eq!(arr[0]["receipt"]["turn"], 1);
    assert_eq!(
        arr[0]["receipt"]["request_sha256"],
        format!("cccccccccccc{}", "0".repeat(52))
    );

    let all: serde_json::Value = serde_json::from_str(
        &context_cmd::render_context_show(&sample(), None, None, true).unwrap(),
    )
    .unwrap();
    assert_eq!(all.as_array().unwrap().len(), 4);
}

#[test]
fn test_cr12_json_without_receipts_should_be_an_empty_array() {
    let out = context_cmd::render_context_show(&[], None, None, true).unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&out).unwrap(),
        serde_json::json!([])
    );
}
