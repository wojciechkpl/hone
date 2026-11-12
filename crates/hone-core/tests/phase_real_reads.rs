#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! K12, K13 and K14 of `docs/plans/context-phase-events-spec.md` (review round 2),
//! built from the REAL `read_file` tool: results come from `hone_tools::ReadFileTool`
//! run on real temp files (`{"content","start_line","end_line","total_lines","truncated"}`,
//! content lines prefixed `"   42 | fn main() {"`, no `path`), stored as the agent
//! stores them (`serde_json::to_string` of the value). The path of a summary can only
//! come from the matching tool call's arguments, so every result is added after an
//! assistant message carrying that call.
//!
//! Real argument shapes: `multi_edit` -> `{"edits":[{"path","old_str","new_str"}]}`,
//! `apply_patch` -> `{"diff": "--- a/..\n+++ b/.."}` (`patch` stays an alias).
//!
//! No new API. Phase compaction is enabled per manager with `with_phase_compaction(true)`
//! (K11); the age path needs no switch. Default config: `recent_window = 3`,
//! structural summary after 5 assistant turns.
//!
//! Format pinned (K12): kept declarations as `<n>: <declaration>` with `<n>` the
//! printed line number; `lines <start>–<end> of <total>` (en dash); `(truncated)`;
//! `first 40 of N declarations`; `1 declaration` (singular); the summary ends with
//! `(re-read before editing)`; no `" | "` line-number prefix survives.

use std::path::Path;
use std::sync::Arc;

use hone_core::{ContextConfig, ContextManager};
use hone_proto::{Message, Role, ToolCallRequest};
use hone_sandbox::{NullSandbox, SandboxPolicy};
use hone_tools::{ReadFileTool, Tool, ToolContext, ToolInput};
use serde_json::{Value, json};

fn manager() -> ContextManager {
    ContextManager::with_config(ContextConfig::default()).with_phase_compaction(true)
}

fn is_summary(t: &str) -> bool {
    t.starts_with("[tool result summarized")
}

/// Runs the real `read_file` and returns the stored message text.
async fn real_read(dir: &Path, args: Value) -> String {
    let ctx = ToolContext {
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(SandboxPolicy::permissive()),
        working_dir: dir.to_path_buf(),
        read_filter: None,
    };
    let out = ReadFileTool.call(ToolInput(args), &ctx).await.unwrap();
    serde_json::to_string(&out.value).unwrap()
}

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

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// A Rust file of `n` top-level `pub fn`s (each 3 lines: decl, body, `}`) after a
/// 3-line header. Declaration `i` (0-based) sits on line `4 + 3 * i`.
fn rust_file(n: usize, marker: &str) -> String {
    let mut s = format!("// {marker}\n//! padding line to make the file longer\nuse std::io;\n");
    for i in 0..n {
        s.push_str(&format!(
            "pub fn func_{i:02}(x: u32) -> u32 {{\n    x + {i} // some body text\n}}\n"
        ));
    }
    s
}

fn decl_line(i: usize) -> usize {
    4 + 3 * i
}

/// Reads `rel` for real, then adds enough fillers (also real reads) and the edit so
/// that the first read is compacted by the phase path.
async fn compacted_by_phase(dir: &Path, first_args: Value, edit_args: Value, edit: &str) -> String {
    write(dir, "other/x.rs", &rust_file(5, "filler"));
    let mut ctx = manager();
    let first = real_read(dir, first_args.clone()).await;
    add_call(&mut ctx, "r1", "read_file", first_args, first);
    for i in 2..=4 {
        let args = json!({ "path": "other/x.rs" });
        let r = real_read(dir, args.clone()).await;
        add_call(&mut ctx, &format!("r{i}"), "read_file", args, r);
    }
    ctx.notify_tool_use(edit, &edit_args);
    tool_text(&ctx, "r1")
}

/// Same, through the age-based path (six more assistant turns, no write).
async fn compacted_by_age(dir: &Path, first_args: Value) -> String {
    let mut ctx = manager();
    ctx.add_message(Message::user("go"));
    let first = real_read(dir, first_args.clone()).await;
    add_call(&mut ctx, "r1", "read_file", first_args, first);
    for i in 0..6 {
        add_call(&mut ctx, &format!("x{i}"), "grep", json!({}), "short".to_owned());
    }
    tool_text(&ctx, "r1")
}

fn assert_real_read_summary(s: &str, path: &str, total: usize, decls: &[usize]) {
    assert!(is_summary(s), "{s}");
    assert!(s.contains(path), "names the path from the call: {s}");
    assert!(s.contains(&format!("lines 1–{total} of {total}")), "range: {s}");
    assert!(!s.contains("(truncated)"), "{s}");
    for &i in decls {
        let want = format!("{}: pub fn func_{i:02}(x: u32) -> u32 {{", decl_line(i));
        assert!(s.contains(&want), "missing {want:?} in {s}");
    }
    assert!(!s.contains(" | "), "line-number prefixes are stripped: {s}");
    assert!(!s.contains("\\n"), "no escaped JSON: {s}");
    assert!(
        s.trim_end().trim_end_matches(']').trim_end().ends_with("(re-read before editing)"),
        "ends with the re-read hint: {s}"
    );
}

// ---- K12 ----------------------------------------------------------------------------------

#[tokio::test]
async fn test_k12_phase_summary_of_a_real_read_should_have_path_range_and_numbered_declarations() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "src/engine.rs", &rust_file(12, "engine"));
    let total = 3 + 12 * 3;

    let s = compacted_by_phase(
        tmp.path(),
        json!({ "path": "src/engine.rs" }),
        json!({ "path": "src/zzz.rs" }),
        "edit_file",
    )
    .await;

    assert_real_read_summary(&s, "src/engine.rs", total, &[0, 1, 11]);
    assert!(s.contains("12 declarations"), "{s}");
}

#[tokio::test]
async fn test_k12_age_summary_of_a_real_read_should_have_path_range_and_numbered_declarations() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "src/engine.rs", &rust_file(12, "engine"));

    let s = compacted_by_age(tmp.path(), json!({ "path": "src/engine.rs" })).await;

    assert_real_read_summary(&s, "src/engine.rs", 3 + 12 * 3, &[0, 1, 11]);
}

#[tokio::test]
async fn test_k12_a_ranged_truncated_read_should_report_its_range_and_truncation() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "src/engine.rs", &rust_file(20, "engine")); // 63 lines
    let args = json!({ "path": "src/engine.rs", "offset": 5, "limit": 20 });

    for s in [
        compacted_by_phase(tmp.path(), args.clone(), json!({ "path": "z.rs" }), "edit_file").await,
        compacted_by_age(tmp.path(), args).await,
    ] {
        assert!(s.contains("lines 5–24 of 63"), "{s}");
        assert!(s.contains("(truncated)"), "{s}");
        // Line 7 is declaration 1 (`4 + 3 * 1`), inside the range; line 4 is not.
        assert!(s.contains("7: pub fn func_01"), "absolute line numbers: {s}");
        assert!(!s.contains("func_00"), "{s}");
    }
}

#[tokio::test]
async fn test_k12_more_than_40_declarations_should_say_first_40_of_n() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "src/big.rs", &rust_file(55, "big"));

    for s in [
        compacted_by_phase(
            tmp.path(),
            json!({ "path": "src/big.rs" }),
            json!({ "path": "z.rs" }),
            "edit_file",
        )
        .await,
        compacted_by_age(tmp.path(), json!({ "path": "src/big.rs" })).await,
    ] {
        assert!(s.contains("first 40 of 55 declarations"), "{s}");
        assert!(s.contains("func_39"), "{s}");
        assert!(!s.contains("func_40"), "{s}");
    }
}

#[tokio::test]
async fn test_k12_one_declaration_should_be_singular() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "src/one.rs", &rust_file(1, "one"));

    let s = compacted_by_age(tmp.path(), json!({ "path": "src/one.rs" })).await;

    assert!(s.contains("1 declaration"), "{s}");
    assert!(!s.contains("1 declarations"), "{s}");
    assert!(s.contains("4: pub fn func_00"), "{s}");
}

#[tokio::test]
async fn test_k12_a_path_field_in_the_result_should_win_over_the_call_argument() {
    let result = serde_json::to_string(&json!({
        "content": format!("   1 | // x\n   2 | {}\n   3 | pub fn alpha() {{}}\n", "// pad ".repeat(40)),
        "path": "from/result.rs",
        "start_line": 1, "end_line": 3, "total_lines": 3, "truncated": false,
    }))
    .unwrap();
    let mut ctx = manager();
    add_call(&mut ctx, "r1", "read_file", json!({ "path": "from/args.rs" }), result);
    for i in 0..3 {
        add_call(&mut ctx, &format!("f{i}"), "read_file", json!({ "path": "o.rs" }), "short".to_owned());
    }
    ctx.notify_tool_use("edit_file", &json!({ "path": "z.rs" }));

    let s = tool_text(&ctx, "r1");
    assert!(s.contains("from/result.rs"), "{s}");
    assert!(!s.contains("from/args.rs"), "{s}");
    assert!(s.contains("3: pub fn alpha"), "{s}");
}

// ---- K13/K14: protection with real shapes ----------------------------------------------------

/// Six real reads `src/f1.rs`..`src/f6.rs` (call args as given by `read_arg`), then
/// `edit(name, args)`. Returns, per file, whether its result is still verbatim.
async fn protected_after(
    read_arg: impl Fn(&Path, usize) -> String,
    edit: &str,
    edit_args: Value,
) -> Vec<bool> {
    let tmp = tempfile::tempdir().unwrap();
    let dir = std::fs::canonicalize(tmp.path()).unwrap();
    let mut ctx = manager();
    for i in 1..=6 {
        let rel = format!("src/f{i}.rs");
        write(&dir, &rel, &rust_file(6, &format!("marker-f{i}")));
        let r = real_read(&dir, json!({ "path": rel })).await;
        add_call(&mut ctx, &format!("r{i}"), "read_file", json!({ "path": read_arg(&dir, i) }), r);
    }
    ctx.notify_tool_use(edit, &edit_args);
    (1..=6)
        .map(|i| {
            let t = tool_text(&ctx, &format!("r{i}"));
            !is_summary(&t) && t.contains(&format!("marker-f{i}"))
        })
        .collect()
}

fn rel(_: &Path, i: usize) -> String {
    format!("src/f{i}.rs")
}

// f4, f5, f6 are the newest three results (window 3); f1..f3 are old.

#[tokio::test]
async fn test_k13_multi_edit_should_protect_every_edits_path() {
    let edits = json!({ "edits": [
        { "path": "src/f1.rs", "old_str": "a", "new_str": "b" },
        { "path": "src/f2.rs", "old_str": "a", "new_str": "b" },
    ]});
    let kept = protected_after(rel, "multi_edit", edits).await;
    assert_eq!(kept, [true, true, false, true, true, true]);
}

#[tokio::test]
async fn test_k13_multi_edit_should_accept_file_path_in_its_edits() {
    let edits = json!({ "edits": [
        { "file_path": "src/f1.rs", "old_str": "a", "new_str": "b" },
        { "file_path": "src/f3.rs", "old_str": "a", "new_str": "b" },
    ]});
    let kept = protected_after(rel, "multi_edit", edits).await;
    assert_eq!(kept, [true, false, true, true, true, true]);
}

fn diff_for(paths: &[&str]) -> String {
    paths
        .iter()
        .map(|p| format!("--- a/{p}\n+++ b/{p}\n@@ -1 +1 @@\n-x\n+y\n"))
        .collect()
}

#[tokio::test]
async fn test_k13_apply_patch_should_read_its_diff_argument() {
    let args = json!({ "diff": diff_for(&["src/f1.rs", "src/f2.rs"]) });
    let kept = protected_after(rel, "apply_patch", args).await;
    assert_eq!(kept, [true, true, false, true, true, true]);
    // `patch` stays an alias of `diff`.
    let alias = json!({ "patch": diff_for(&["src/f2.rs"]) });
    let kept = protected_after(rel, "apply_patch", alias).await;
    assert_eq!(kept, [false, true, false, true, true, true], "patch alias");
}

#[tokio::test]
async fn test_k14_an_absolute_read_path_should_be_protected_by_a_relative_edit() {
    // `glob` returns absolute paths; the model may read those and edit by relative path.
    let abs = |dir: &Path, i: usize| format!("{}/src/f{i}.rs", dir.display());
    let kept = protected_after(abs, "edit_file", json!({ "path": "src/f1.rs" })).await;
    assert_eq!(kept, [true, false, false, true, true, true]);
}

#[tokio::test]
async fn test_k14_a_relative_read_path_should_be_protected_by_an_absolute_edit() {
    // The edit path is absolute (as `glob` returns); the read was relative.
    let kept = protected_after(rel, "edit_file", json!({ "file_path": "/anywhere/repo/src/f2.rs" })).await;
    assert_eq!(kept, [false, true, false, true, true, true]);
}

#[tokio::test]
async fn test_k14_a_shorter_path_should_match_whole_trailing_components() {
    let kept = protected_after(rel, "edit_file", json!({ "path": "f1.rs" })).await;
    assert_eq!(kept, [true, false, false, true, true, true], "f1.rs is src/f1.rs");
    // A leading `./` is ignored.
    let kept = protected_after(rel, "edit_file", json!({ "path": "./src/f1.rs" })).await;
    assert_eq!(kept, [true, false, false, true, true, true], "./src/f1.rs");
    // `rc/f1.rs` is a suffix of `src/f1.rs` as a string, but not component-wise.
    for edit in ["rc/f1.rs", "xsrc/f1.rs", "src/f1.r", "1.rs"] {
        let kept = protected_after(rel, "edit_file", json!({ "path": edit })).await;
        assert_eq!(kept, [false, false, false, true, true, true], "{edit}");
    }
}
