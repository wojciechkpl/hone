#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Spec G1, G3 and G4 at the `ContextManager` level: fitting a request copy
//! must be linear, must be recorded as its own receipt step (not as repeated
//! `drop_oldest` of the stored history), and must not send orphan tool results.
//!
//! G1 approach: results are checked structurally (first message + a maximal
//! contiguous tail that fits), and the time of one fit of a 550K history is
//! compared with one plain tokenizer pass over the same messages, with a
//! generous factor of 5 (the quadratic path is ~70x slower).

use std::time::Instant;

use hone_core::{ContextConfig, ContextManager, estimate_text_tokens};
use hone_proto::{Message, Role, ToolCallRequest};

fn text_tokens(m: &Message) -> u32 {
    estimate_text_tokens(&m.content.as_text())
}

/// `n` distinct messages of roughly 6.5k tokens each, starting with a user message.
fn history(n: usize) -> Vec<Message> {
    (0..n)
        .map(|i| {
            let text: String = (0..1500).map(|j| format!("w{i}x{j} ")).collect();
            if i % 2 == 0 {
                Message::user(text)
            } else {
                Message::assistant(text)
            }
        })
        .collect()
}

fn opus_manager(msgs: &[Message]) -> ContextManager {
    let mut ctx = ContextManager::with_config(ContextConfig::for_model("claude-opus-4-7"));
    for m in msgs {
        ctx.add_message(m.clone());
    }
    ctx
}

// ---------------------------------------------------------------------------
// G1
// ---------------------------------------------------------------------------

#[test]
fn test_g1_fit_when_history_is_large_should_keep_first_plus_maximal_tail_that_fits() {
    let stored = history(85); // ~550K tokens, all kept under the 600K budget
    let mut ctx = opus_manager(&stored);
    assert_eq!(ctx.messages().len(), 85, "precondition: nothing trimmed");

    let (req, _) = ctx.build_messages_hashed_fitted("sys", 120_000);

    let stored_text: Vec<String> = stored.iter().map(|m| m.content.as_text()).collect();
    let kept: Vec<usize> = req
        .iter()
        .filter_map(|m| stored_text.iter().position(|t| *t == m.content.as_text()))
        .collect();
    assert_eq!(kept[0], 0, "first user message must be kept");
    let tail = &kept[1..];
    assert!(!tail.is_empty());
    assert!(
        tail.windows(2).all(|w| w[1] == w[0] + 1),
        "tail must be contiguous: {tail:?}"
    );
    assert_eq!(*tail.last().unwrap(), 84, "newest message must be kept");

    let kept_tokens: u32 = kept.iter().map(|&i| text_tokens(&stored[i])).sum();
    assert!(kept_tokens <= 120_000, "kept {kept_tokens} tokens");
    let next_older = tail[0] - 1;
    assert!(
        kept_tokens + text_tokens(&stored[next_older]) > 120_000 - 500,
        "over-trimmed: {kept_tokens} kept, next older message would still fit"
    );
}

#[test]
fn test_g1_fit_when_history_is_large_should_cost_about_one_tokenizer_pass() {
    let stored = history(85);
    let mut ctx = opus_manager(&stored);

    let t = Instant::now();
    let _baseline: u32 = stored.iter().map(text_tokens).sum();
    let one_pass = t.elapsed();

    let t = Instant::now();
    let (req, _) = ctx.build_messages_hashed_fitted("sys", 120_000);
    let fit = t.elapsed();

    assert!(req.len() < stored.len(), "precondition: something was cut");
    assert!(
        fit <= one_pass * 5 + std::time::Duration::from_millis(20),
        "fit took {fit:?}; one tokenizer pass takes {one_pass:?}"
    );
}

// ---------------------------------------------------------------------------
// G3
// ---------------------------------------------------------------------------

#[test]
fn test_g3_fit_when_called_twice_should_record_fit_request_and_never_drop_oldest() {
    let stored = history(40);
    let mut ctx = opus_manager(&stored);
    let _ = ctx.drain_compaction_log();

    for call in 1..=2 {
        let _ = ctx.build_messages_hashed_fitted("sys", 60_000);
        let log = ctx.drain_compaction_log();
        let names: Vec<&str> = log.iter().map(|s| s.strategy.name.as_str()).collect();
        assert_eq!(
            names.iter().filter(|n| **n == "fit_request").count(),
            1,
            "call {call}: steps {names:?}"
        );
        assert!(
            !names.contains(&"drop_oldest"),
            "call {call}: fitting a copy is not a change to the stored history: {names:?}"
        );
    }
    assert_eq!(ctx.messages().len(), 40, "stored history unchanged");
}

#[test]
fn test_g3_fit_when_history_fits_should_record_no_step() {
    let mut ctx = opus_manager(&history(4));
    let _ = ctx.drain_compaction_log();
    let _ = ctx.build_messages_hashed_fitted("sys", 600_000);
    assert!(ctx.drain_compaction_log().is_empty());
}

// ---------------------------------------------------------------------------
// G4
// ---------------------------------------------------------------------------

fn call_msg(id: &str, text: String) -> Message {
    let mut m = Message::assistant(text);
    m.tool_calls = vec![ToolCallRequest {
        id: id.to_owned(),
        name: "read_file".to_owned(),
        arguments: serde_json::json!({ "path": "a.rs" }),
    }];
    m
}

fn big(tag: &str, words: usize) -> String {
    (0..words).map(|j| format!("{tag}{j} ")).collect()
}

fn assert_paired(req: &[Message]) {
    for m in req.iter().filter(|m| m.role == Role::Tool) {
        let id = m.tool_call_id.as_deref().expect("tool result has an id");
        assert!(
            req.iter()
                .any(|a| a.role == Role::Assistant && a.tool_calls.iter().any(|c| c.id == id)),
            "orphan tool result {id} in the request"
        );
    }
    for a in req.iter().filter(|m| m.role == Role::Assistant) {
        for c in &a.tool_calls {
            assert!(
                req.iter()
                    .any(|t| t.role == Role::Tool && t.tool_call_id.as_deref() == Some(&c.id)),
                "tool call {} has no result in the request",
                c.id
            );
        }
    }
}

#[test]
fn test_g4_fit_when_cut_lands_between_call_and_result_should_drop_the_orphan_result() {
    // user0, [big assistant call c1], [small result c1], then ten ~1k-token turns.
    let mut stored = vec![
        Message::user("start"),
        call_msg("c1", big("call", 1500)), // ~6k tokens: the one the cut removes
        Message::tool_result("c1".to_owned(), "ok"),
    ];
    for i in 0..10 {
        stored.push(if i % 2 == 0 {
            Message::user(big(&format!("u{i}x"), 250))
        } else {
            Message::assistant(big(&format!("a{i}x"), 250))
        });
    }
    let mut ctx = opus_manager(&stored);
    let total = ctx.total_tokens();
    // Dropping the big call message alone gets under this budget.
    let budget = total - 3_000;

    let (req, _) = ctx.build_messages_hashed_fitted("sys", budget);

    assert!(
        !req.iter()
            .any(|m| m.tool_calls.iter().any(|c| c.id == "c1")),
        "precondition: the call was cut"
    );
    assert_paired(&req);
    assert_eq!(
        ctx.messages().len(),
        stored.len(),
        "stored history unchanged"
    );
}

#[test]
fn test_g4_fit_when_nothing_is_cut_should_keep_paired_tool_calls() {
    let stored = vec![
        Message::user("start"),
        call_msg("c1", "reading".to_owned()),
        Message::tool_result("c1".to_owned(), "ok"),
        Message::assistant("done"),
    ];
    let mut ctx = opus_manager(&stored);
    let (req, _) = ctx.build_messages_hashed_fitted("sys", 600_000);
    assert!(req.iter().any(|m| m.role == Role::Tool));
    assert_paired(&req);
}
