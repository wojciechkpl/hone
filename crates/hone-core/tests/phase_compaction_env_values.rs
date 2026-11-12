#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! K11 of `docs/plans/context-phase-events-spec.md`: which values of
//! `HONE_PHASE_COMPACTION` enable phase compaction (`1`, `true`, `on`; anything
//! else, or unset, leaves it off), end to end on a real `ContextManager`.
//!
//! This binary has ONE test that sets and removes the process-global variable
//! sequentially, so nothing races. Assumption the implementer must honour: the
//! variable is read when the `ContextManager` is built (or later), not cached in
//! a process-wide static.

use std::sync::Arc;

use hone_core::{ContextConfig, ContextManager, ContextMetrics, TaskPhase};
use hone_proto::{Message, Role, ToolCallRequest};
use serde_json::json;

/// Five long reads, then an edit. Returns (r1 was summarised, transitions, phase).
fn session() -> (bool, u64, TaskPhase) {
    let metrics = Arc::new(ContextMetrics::new());
    let mut ctx =
        ContextManager::with_config(ContextConfig::default()).with_metrics(metrics.clone());
    for i in 1..=5 {
        let mut a = Message::assistant("");
        a.tool_calls = vec![ToolCallRequest {
            id: format!("r{i}"),
            name: "read_file".to_owned(),
            arguments: json!({ "path": format!("f{i}.txt") }),
        }];
        ctx.add_message(a);
        ctx.add_message(Message::tool_result(
            format!("r{i}"),
            format!("file {i}: {}", "lorem ipsum dolor sit amet ".repeat(20)),
        ));
    }
    ctx.notify_tool_use("edit_file", &json!({ "path": "other.rs" }));
    let r1 = ctx
        .messages()
        .iter()
        .find(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("r1"))
        .unwrap()
        .content
        .as_text()
        .to_string();
    (
        r1.starts_with("[tool result summarized"),
        metrics.snapshot().phase_transitions,
        ctx.current_phase(),
    )
}

#[test]
fn test_k11_only_1_true_and_on_should_enable_phase_compaction() {
    // SAFETY: the only test in this binary, so no other thread touches the env.
    unsafe { std::env::remove_var("HONE_PHASE_COMPACTION") };
    let (compacted, transitions, phase) = session();
    assert!(!compacted, "unset: off");
    assert_eq!(
        transitions, 1,
        "unset: the tracker still counts the transition"
    );
    assert_eq!(phase, TaskPhase::Editing, "unset: the tracker still runs");

    for on in ["1", "true", "on"] {
        unsafe { std::env::set_var("HONE_PHASE_COMPACTION", on) };
        let (compacted, transitions, _) = session();
        assert!(compacted, "HONE_PHASE_COMPACTION={on:?} enables compaction");
        assert_eq!(transitions, 1, "{on:?}");
    }
    for off in ["0", "false", "off", "", "yes", "2"] {
        unsafe { std::env::set_var("HONE_PHASE_COMPACTION", off) };
        let (compacted, transitions, phase) = session();
        assert!(!compacted, "HONE_PHASE_COMPACTION={off:?} leaves it off");
        assert_eq!(transitions, 1, "{off:?}");
        assert_eq!(phase, TaskPhase::Editing, "{off:?}");
    }
    unsafe { std::env::remove_var("HONE_PHASE_COMPACTION") };
}
