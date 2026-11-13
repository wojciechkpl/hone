#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic, dead_code)]
//! CR25 and CR26 of `docs/plans/context-receipts-spec.md` (review round 2 for PR C), the
//! renderer behind `hone context show` as a pure function (same linkage as
//! `context_show_render.rs`).
//!
//! Contract pinned:
//! - CR25: every string taken from a receipt (model, strategy names, file paths, command
//!   argv, skill names, memory ids, tool-call ids, chunk ids, opaque plugin and note) is
//!   shown with control characters (C0, DEL, C1) escaped as `\u{<hex>}` (lowercase hex, no
//!   padding: ESC is `\u{1b}`, BEL `\u{7}`, CR `\u{d}`, LF `\u{a}`). The only raw control
//!   character in the output is the renderer's own `\n`. `--json` is unchanged.
//! - CR26: each receipt prints a line `  history: <N> messages (<K> new listed), root
//!   <first 12 hex digits of history_root>` (N = `history_len`, K = the number of refs
//!   listed in the `History` block), followed by the listed refs.

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

fn scored(source: SourceRef) -> ScoredSource {
    ScoredSource {
        source,
        score: Some(0.5),
    }
}

/// One receipt exercising every string the renderer prints; `wrap` decorates each.
fn receipt_with(wrap: impl Fn(&str) -> String) -> ContextReceipt {
    ContextReceipt {
        turn: 1,
        call: 1,
        model: wrap("model"),
        request_sha256: "c".repeat(64),
        blocks: vec![
            BlockReceipt {
                slot: Slot::Recall,
                strategy: strategy(Slot::Recall, &wrap("recall")),
                tokens: 10,
                sources: vec![scored(SourceRef::Memory {
                    id: wrap("mem"),
                    version: None,
                })],
                duration_ms: 0,
            },
            BlockReceipt {
                slot: Slot::Injection,
                strategy: strategy(Slot::Injection, &wrap("skeleton")),
                tokens: 10,
                sources: vec![
                    scored(SourceRef::File {
                        path: wrap("src/x").into(),
                        sha256: None,
                        lines: None,
                    }),
                    scored(SourceRef::Command {
                        argv: vec![wrap("git"), wrap("status")],
                        sha256: "d".repeat(64),
                    }),
                    scored(SourceRef::Opaque {
                        plugin: wrap("plugin"),
                        note: wrap("note"),
                    }),
                    scored(SourceRef::ToolResult {
                        call_id: wrap("call"),
                        sha256: "e".repeat(64),
                    }),
                    scored(SourceRef::Chunk {
                        index: wrap("idx"),
                        chunk_id: wrap("chunk"),
                        sha256: "f".repeat(64),
                    }),
                ],
                duration_ms: 0,
            },
            BlockReceipt {
                slot: Slot::Skills,
                strategy: strategy(Slot::Skills, "skills:trigger"),
                tokens: 10,
                sources: vec![scored(SourceRef::Skill { name: wrap("skill") })],
                duration_ms: 0,
            },
        ],
        dropped: vec![SourceRef::Memory {
            id: wrap("dropped"),
            version: None,
        }],
        compaction: vec![CompactionStep {
            strategy: strategy(Slot::History, &wrap("graduated")),
            replaced: vec![SourceRef::Skill { name: wrap("old") }],
            derived: Some(SourceRef::Skill { name: wrap("new") }),
        }],
        tools_offered: vec![],
        history_len: 0,
        history_root: String::new(),
    }
}

fn stored(r: ContextReceipt) -> Vec<StoredReceipt> {
    vec![StoredReceipt { run: 1, seq: 1, receipt: r }]
}

fn text(r: ContextReceipt) -> String {
    context_cmd::render_context_show(&stored(r), None, None, false).unwrap()
}

/// Everything below 0x20 (but the renderer's `\n`), DEL, and the C1 range.
fn raw_control(out: &str) -> Vec<char> {
    out.chars()
        .filter(|c| (c.is_control() && *c != '\n') || *c == '\u{7f}')
        .collect()
}

// ---- CR25 ----------------------------------------------------------------------------------

const EVIL: &str = "\u{1b}]0;PWNED\u{7}\u{1b}[2K\rforged\nrun 9 turn 9 call 9  evil  request 000000000000";

#[test]
fn test_cr25_receipt_strings_should_not_put_raw_control_characters_in_the_text() {
    let out = text(receipt_with(|base| format!("{base}{EVIL}")));

    assert_eq!(raw_control(&out), Vec::<char>::new(), "raw control characters in: {out:?}");
    assert!(!out.contains('\u{1b}') && !out.contains('\u{7}') && !out.contains('\r'), "{out:?}");

    // `--json` is unchanged: serde escapes the strings, and they round-trip as stored.
    let r = receipt_with(|base| format!("{base}{EVIL}"));
    let json = context_cmd::render_context_show(&stored(r.clone()), None, None, true).unwrap();
    assert!(!json.contains('\u{1b}') && !json.contains('\u{7}') && !json.contains('\r'), "{json:?}");
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let back: ContextReceipt = serde_json::from_value(v[0]["receipt"].clone()).unwrap();
    assert_eq!(back, r, "the receipt is printed as stored, not escaped by the renderer");
}

#[test]
fn test_cr25_a_newline_in_a_string_should_not_forge_a_line_or_add_lines() {
    let clean = text(receipt_with(|base| base.to_owned()));
    let evil = text(receipt_with(|base| format!("{base}{EVIL}")));

    assert_eq!(evil.lines().count(), clean.lines().count(), "same shape, same line count: {evil:?}");
    assert!(
        !evil.lines().any(|l| l.starts_with("run 9")),
        "no forged header line: {evil:?}"
    );
}

#[test]
fn test_cr25_the_escaped_form_should_be_visible_for_every_kind_of_string() {
    let out = text(receipt_with(|base| format!("{base}\u{1b}!\u{7}?")));

    assert!(out.contains("\\u{1b}"), "{out:?}");
    assert!(out.contains("\\u{7}"), "{out:?}");
    // each kind of string carries the escaped marker
    for base in [
        "model", "recall", "mem", "src/x", "git", "status", "plugin", "note", "call", "chunk",
        "skeleton", "skill", "dropped", "graduated", "old", "new",
    ] {
        assert!(
            out.contains(&format!("{base}\\u{{1b}}!\\u{{7}}?")),
            "{base} escaped in {out:?}"
        );
    }

    // Ordinary text, including non-ASCII, is shown unchanged.
    let plain = text(receipt_with(|base| format!("{base} zażółć gęślą café 日本")));
    assert!(plain.contains("model zażółć gęślą café 日本"), "{plain}");
    assert!(!plain.contains("\\u{"), "nothing to escape: {plain}");
}

#[test]
fn test_cr25_other_control_characters_including_del_and_c1_should_be_escaped() {
    let out = text(receipt_with(|base| format!("{base}\u{0}\u{8}\u{b}\u{c}\u{7f}\u{85}\u{9b}")));

    assert_eq!(raw_control(&out), Vec::<char>::new(), "{out:?}");
    assert!(out.contains("\\u{0}") && out.contains("\\u{7f}") && out.contains("\\u{9b}"), "{out:?}");
}

// ---- CR26 -----------------------------------------------------------------------------------

fn history_receipt(len: u32, listed: usize, root: &str) -> ContextReceipt {
    let mut r = receipt_with(|b| b.to_owned());
    r.blocks.clear();
    r.dropped.clear();
    r.compaction.clear();
    r.history_len = len;
    r.history_root = root.to_owned();
    r.blocks.push(BlockReceipt {
        slot: Slot::History,
        strategy: strategy(Slot::History, "history"),
        tokens: 40,
        sources: (0..listed)
            .map(|i| ScoredSource {
                source: SourceRef::Message {
                    index: 3 + i,
                    sha256: format!("{i:02x}").repeat(32),
                },
                score: None,
            })
            .collect(),
        duration_ms: 0,
    });
    r
}

#[test]
fn test_cr26_the_history_line_should_show_the_length_the_new_count_and_a_root_prefix() {
    let root = format!("e09db7e7317a{}", "9".repeat(52));

    let out = text(history_receipt(5, 2, &root));

    assert!(
        out.lines().any(|l| l.trim() == "history: 5 messages (2 new listed), root e09db7e7317a"),
        "{out}"
    );
    assert!(!out.contains(&root), "never the full root: {out}");
    assert!(!out.contains("e09db7e7317a9"), "exactly 12 digits: {out}");
}

#[test]
fn test_cr26_the_listed_refs_should_follow_the_history_line() {
    let root = format!("e09db7e7317a{}", "9".repeat(52));

    let out = text(history_receipt(5, 2, &root));

    let lines: Vec<&str> = out.lines().collect();
    let at = lines.iter().position(|l| l.contains("history: 5 messages")).expect("history line");
    assert!(lines[at + 1..].iter().any(|l| l.contains("message #3")), "{out}");
    assert!(lines[at + 1..].iter().any(|l| l.contains("message #4")), "{out}");
}

#[test]
fn test_cr26_a_receipt_listing_nothing_new_should_still_show_the_history_length() {
    let root = format!("0a1b2c3d4e5f{}", "1".repeat(52));

    let out = text(history_receipt(30, 0, &root));

    assert!(out.contains("30 messages (0 new listed), root 0a1b2c3d4e5f"), "{out}");
}
