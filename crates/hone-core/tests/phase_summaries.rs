#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! K8, K9 and K10 of `docs/plans/context-phase-events-spec.md` (review round 1),
//! on a real `ContextManager`.
//!
//! No new API. K11 makes phase compaction opt-in through `HONE_PHASE_COMPACTION`,
//! so every test here first turns it on, once per process (`std::sync::Once`,
//! before any manager is built). Assumption the implementer must honour: the
//! variable is read when the `ContextManager` is built (or later), and `1`,
//! `true` and `on` enable it.
//!
//! The default config has `recent_window = 3`, `tool_compress_age = 5` (structural
//! summary after 5 assistant turns) and a one-liner after 10.

use std::sync::Once;

use hone_core::{ContextConfig, ContextManager};
use hone_proto::{Message, Role, ToolCallRequest};
use serde_json::{Value, json};

static ENV: Once = Once::new();

fn manager() -> ContextManager {
    ENV.call_once(|| {
        // SAFETY: runs once before any manager is built; no other thread reads the env here.
        unsafe { std::env::set_var("HONE_PHASE_COMPACTION", "1") };
    });
    ContextManager::with_config(ContextConfig::default())
}

fn is_summary(text: &str) -> bool {
    text.starts_with("[tool result summarized")
}

/// Add `assistant(call)` then the tool result, like the agent does.
fn add_call(ctx: &mut ContextManager, id: &str, name: &str, args: Value, result: String) {
    let mut a = Message::assistant("");
    a.tool_calls = vec![ToolCallRequest {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: args,
    }];
    ctx.add_message(a);
    ctx.add_message(Message::tool_result(id.to_owned(), result));
}

fn tool_text(ctx: &ContextManager, id: &str) -> String {
    ctx.messages()
        .iter()
        .find(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some(id))
        .unwrap_or_else(|| panic!("no tool result {id}"))
        .content
        .as_text()
        .to_string()
}

fn read_json(path: &str, content: &str) -> String {
    serde_json::to_string(&json!({ "content": content, "path": path })).unwrap()
}

const POLISH: &str = "zażółć gęślą jaźń ";

/// Polish text starting after `pad` ASCII bytes.
fn polish(pad: usize, chars: usize) -> String {
    format!(
        "{}{}",
        "a".repeat(pad),
        POLISH.repeat(chars / POLISH.chars().count() + 1)
    )
}

// ---- K8: no summary splits a UTF-8 character ------------------------------------------

#[test]
fn test_k8_harness_polish_text_should_straddle_bytes_200_and_500() {
    for cut in [200usize, 500] {
        let straddles = (0..8).any(|pad| {
            let t = serde_json::to_string(&json!({ "content": polish(pad, 900), "path": "a.rs" }))
                .unwrap();
            !t.is_char_boundary(cut) || !polish(pad, 900).is_char_boundary(cut)
        });
        assert!(
            straddles,
            "test data must put a multi-byte char across byte {cut}"
        );
    }
}

#[test]
fn test_k8_phase_path_should_not_panic_on_a_read_file_result_with_polish_text() {
    // The reviewer's repro: a read_file JSON result across byte 500, one more short
    // read, then an edit.
    for pad in 0..8 {
        let mut ctx = manager();
        add_call(
            &mut ctx,
            "r1",
            "read_file",
            json!({ "path": "a.rs" }),
            read_json("a.rs", &polish(pad, 900)),
        );
        add_call(
            &mut ctx,
            "r2",
            "read_file",
            json!({ "path": "b.rs" }),
            "short".to_owned(),
        );
        add_call(
            &mut ctx,
            "r3",
            "read_file",
            json!({ "path": "c.rs" }),
            "short".to_owned(),
        );
        add_call(
            &mut ctx,
            "r4",
            "read_file",
            json!({ "path": "d.rs" }),
            "short".to_owned(),
        );
        ctx.notify_tool_use("edit_file", &json!({ "path": "z.rs" }));
        assert!(is_summary(&tool_text(&ctx, "r1")), "pad {pad}");
    }
}

#[test]
fn test_k8_phase_path_should_not_panic_on_plain_text_results_cut_at_200_or_500() {
    // Each variant reaches a different byte-sliced helper: the plain sample at 500,
    // the code "no signature" fallback at 200.
    let variants: Vec<(&str, Box<dyn Fn(usize) -> String>)> = vec![
        ("plain", Box::new(|pad| polish(pad, 900))),
        (
            // 'fn ' makes it look like code; no line starts with a declaration.
            "code without signatures",
            Box::new(|pad| format!("// see fn main\n{}", polish(pad, 900))),
        ),
    ];
    for (label, make) in &variants {
        for pad in 0..8 {
            let mut ctx = manager();
            add_call(&mut ctx, "r1", "grep", json!({}), make(pad));
            add_call(&mut ctx, "r2", "grep", json!({}), "short".to_owned());
            add_call(&mut ctx, "r3", "grep", json!({}), "short".to_owned());
            add_call(&mut ctx, "r4", "grep", json!({}), "short".to_owned());
            ctx.notify_tool_use("edit_file", &json!({ "path": "z.rs" }));
            let t = tool_text(&ctx, "r1");
            assert!(is_summary(&t), "{label} pad {pad}: {t}");
        }
    }
}

#[test]
fn test_k8_age_based_path_should_not_panic_on_polish_text_in_the_structural_tier() {
    // Panics on main today. 6 assistant turns after the result: structural summary.
    for pad in 0..8 {
        let mut ctx = manager();
        ctx.add_message(Message::user("go"));
        add_call(
            &mut ctx,
            "r1",
            "read_file",
            json!({ "path": "a.rs" }),
            read_json("a.rs", &polish(pad, 900)),
        );
        add_call(&mut ctx, "r2", "grep", json!({}), polish(pad, 900));
        for i in 0..6 {
            add_call(
                &mut ctx,
                &format!("x{i}"),
                "grep",
                json!({}),
                "short".to_owned(),
            );
        }
        assert!(is_summary(&tool_text(&ctx, "r1")), "pad {pad}");
        assert!(is_summary(&tool_text(&ctx, "r2")), "pad {pad}");
    }
}

#[test]
fn test_k8_age_based_path_should_not_panic_on_polish_text_in_the_oneliner_tier() {
    // 11+ assistant turns after the result: tool_oneliner slices [..500].
    for pad in 0..8 {
        let mut ctx = manager();
        ctx.add_message(Message::user("go"));
        add_call(
            &mut ctx,
            "r1",
            "read_file",
            json!({ "path": "a.rs" }),
            read_json("a.rs", &polish(pad, 900)),
        );
        add_call(&mut ctx, "r2", "grep", json!({}), polish(pad, 900));
        for i in 0..12 {
            add_call(
                &mut ctx,
                &format!("x{i}"),
                "grep",
                json!({}),
                "short".to_owned(),
            );
        }
        assert!(
            tool_text(&ctx, "r1").starts_with("[tool result:"),
            "pad {pad}"
        );
        assert!(
            tool_text(&ctx, "r2").starts_with("[tool result:"),
            "pad {pad}"
        );
    }
}

// ---- K9: a summarised file read keeps its path and structure --------------------------

/// A Rust-like file: `n_fns` `pub fn` lines, each followed by a body line, and a
/// `struct` at the end. Longer than 2 KB.
fn rust_file(n_fns: usize) -> String {
    let mut s = String::from("//! module docs\nuse std::io;\n\n");
    for i in 0..n_fns {
        s.push_str(&format!(
            "pub fn func_{i:02}(x: u32) -> u32 {{\n    // body of func {i}\n    x + {i}\n}}\n\n"
        ));
    }
    s.push_str("pub struct Tail {\n    field: u32,\n}\n");
    s
}

fn after_edit_summary(path: &str, content: &str) -> String {
    let mut ctx = manager();
    add_call(
        &mut ctx,
        "r1",
        "read_file",
        json!({ "path": path }),
        read_json(path, content),
    );
    for (i, p) in ["x.rs", "y.rs", "z.rs"].iter().enumerate() {
        add_call(
            &mut ctx,
            &format!("f{i}"),
            "read_file",
            json!({ "path": p }),
            read_json(p, &rust_file(3)),
        );
    }
    ctx.notify_tool_use("edit_file", &json!({ "path": "other.rs" }));
    tool_text(&ctx, "r1")
}

#[test]
fn test_k9_summary_of_a_file_read_should_name_the_path_and_line_count() {
    let content = rust_file(30);
    let lines = content.lines().count();
    let s = after_edit_summary("src/engine/lib.rs", &content);

    assert!(is_summary(&s), "{s}");
    assert!(s.contains("src/engine/lib.rs"), "names the path: {s}");
    assert!(s.contains(&lines.to_string()), "names {lines} lines: {s}");
}

#[test]
fn test_k9_summary_should_keep_structural_lines_not_escaped_json() {
    let s = after_edit_summary("src/lib.rs", &rust_file(30));

    assert!(s.contains("pub fn func_00(x: u32) -> u32 {"), "{s}");
    assert!(
        !s.contains("\\n") && !s.contains("{\"content\""),
        "no escaped JSON in the summary: {s}"
    );
}

#[test]
fn test_k9_summary_should_keep_at_most_40_structural_lines() {
    let s = after_edit_summary("src/lib.rs", &rust_file(60));

    let kept = s.matches("pub fn func_").count();
    assert_eq!(kept, 40, "keeps the first 40 structural lines: {s}");
    assert!(s.contains("func_00"));
    assert!(!s.contains("func_59"), "later declarations are cut: {s}");
}

#[test]
fn test_k9_summary_without_a_path_should_still_keep_structure() {
    // Structure starts beyond the first 200 characters, so the old "first 200
    // chars of JSON" summary cannot contain it.
    let content = format!("{}\n{}", "// preamble line\n".repeat(30), rust_file(5));
    let mut ctx = manager();
    add_call(
        &mut ctx,
        "r1",
        "read_file",
        json!({}),
        serde_json::to_string(&json!({ "content": content })).unwrap(),
    );
    for i in 0..3 {
        add_call(
            &mut ctx,
            &format!("f{i}"),
            "read_file",
            json!({}),
            read_json("x.rs", &rust_file(3)),
        );
    }
    ctx.notify_tool_use("edit_file", &json!({ "path": "other.rs" }));

    let s = tool_text(&ctx, "r1");
    assert!(is_summary(&s), "{s}");
    assert!(s.contains("pub fn func_00"), "{s}");
}

// ---- K10: compaction keeps what the edit needs ----------------------------------------

fn five_reads(ctx: &mut ContextManager) {
    for (i, p) in ["src/a.rs", "src/b.rs", "src/c.rs", "src/d.rs", "src/e.rs"]
        .iter()
        .enumerate()
    {
        let content = format!("// marker {p}\n{}", rust_file(8));
        add_call(
            ctx,
            &format!("r{}", i + 1),
            "read_file",
            json!({ "path": p }),
            read_json(p, &content),
        );
    }
}

fn verbatim(ctx: &ContextManager, id: &str, path: &str) -> bool {
    let t = tool_text(ctx, id);
    !is_summary(&t) && t.contains(&format!("marker {path}"))
}

#[test]
fn test_k10_recent_window_should_count_tool_results_not_messages() {
    // Three file reads, then an edit elsewhere: all three results are the newest
    // three tool results, so none is compacted. (By message count only the last
    // one is protected.)
    let mut ctx = manager();
    for (i, p) in ["src/a.rs", "src/b.rs", "src/c.rs"].iter().enumerate() {
        let content = format!("// marker {p}\n{}", rust_file(8));
        add_call(
            &mut ctx,
            &format!("r{}", i + 1),
            "read_file",
            json!({ "path": p }),
            read_json(p, &content),
        );
    }
    ctx.notify_tool_use("edit_file", &json!({ "path": "src/zzz.rs" }));

    for (id, p) in [("r1", "src/a.rs"), ("r2", "src/b.rs"), ("r3", "src/c.rs")] {
        assert!(
            verbatim(&ctx, id, p),
            "{id} is within the newest 3 tool results"
        );
    }
}

#[test]
fn test_k10_results_older_than_the_newest_three_should_be_compacted() {
    let mut ctx = manager();
    five_reads(&mut ctx);
    ctx.notify_tool_use("edit_file", &json!({ "path": "src/zzz.rs" }));

    assert!(is_summary(&tool_text(&ctx, "r1")));
    assert!(is_summary(&tool_text(&ctx, "r2")));
    for (id, p) in [("r3", "src/c.rs"), ("r4", "src/d.rs"), ("r5", "src/e.rs")] {
        assert!(verbatim(&ctx, id, p), "{id}");
    }
}

#[test]
fn test_k10_a_read_of_the_edit_target_should_not_be_compacted() {
    for key in ["path", "file_path"] {
        let mut ctx = manager();
        five_reads(&mut ctx);
        ctx.notify_tool_use("edit_file", &json!({ key: "src/a.rs", "old_string": "x" }));

        assert!(
            verbatim(&ctx, "r1", "src/a.rs"),
            "{key}: the target read stays verbatim"
        );
        assert!(
            is_summary(&tool_text(&ctx, "r2")),
            "{key}: other old reads are compacted"
        );
    }
}

#[test]
fn test_k10_apply_patch_should_protect_reads_of_every_path_in_its_headers() {
    // Assumption: header paths may carry the `a/` / `b/` prefix of a git-style diff.
    let patch = "--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1 +1 @@\n-x\n+y\n\
                 --- a/src/b.rs\n+++ b/src/b.rs\n@@ -1 +1 @@\n-x\n+y\n";
    let mut ctx = manager();
    five_reads(&mut ctx);
    ctx.notify_tool_use("apply_patch", &json!({ "patch": patch }));

    assert!(verbatim(&ctx, "r1", "src/a.rs"), "first patched file");
    assert!(verbatim(&ctx, "r2", "src/b.rs"), "second patched file");
    assert!(verbatim(&ctx, "r3", "src/c.rs"));
}
