#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR13 of `docs/plans/context-receipts-spec.md`: with an egress log configured,
//! every receipt adds one audit line: `span_kind: "context"`, `tool:
//! "context_assembled"`, `arguments` = the receipt, `result_preview: ""`,
//! `blocked: false`, `duration_ms` = the sum of the blocks' `duration_ms`, the
//! turn's `trace_id`, and the turn span as `parent_span_id` (the same parent the
//! turn's tool-call entries have). `EgressEntry` gains no fields.
//!
//! API required (new): `hone_core::SpanKind::Context` (serialised `"context"`).

mod receipts_support;

use std::sync::Arc;

use hone_core::{EgressLog, SpanKind};
use hone_proto::{AgentEvent, ContextReceipt};
use receipts_support::*;
use serde_json::Value;

fn lines(path: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn with_audit(dir: &std::path::Path) -> (Opts, std::path::PathBuf) {
    let path = dir.join(".hone/audit.jsonl");
    let log = Arc::new(tokio::sync::Mutex::new(EgressLog::new(path.clone())));
    (
        Opts {
            egress_log: Some(log),
            ..Default::default()
        },
        path,
    )
}

fn receipts(events: &[AgentEvent]) -> Vec<ContextReceipt> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ContextAssembled { receipt } => Some(receipt.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn test_cr13_span_kind_context_should_serialise_as_context() {
    assert_eq!(serde_json::to_value(SpanKind::Context).unwrap(), "context");
}

#[tokio::test]
async fn test_cr13_each_receipt_should_add_one_context_audit_line() {
    let dir = tempfile::tempdir().unwrap();
    let (opts, path) = with_audit(dir.path());
    let mut r = rig(vec![vec![read("r1")]], opts);

    let events = r.turn("look around").await;

    let sent = receipts(&events);
    assert_eq!(sent.len(), 2);
    let all = lines(&path);
    let context: Vec<&Value> = all.iter().filter(|l| l["span_kind"] == "context").collect();
    assert_eq!(context.len(), 2, "one line per receipt: {all:?}");
    for (line, receipt) in context.iter().zip(&sent) {
        assert_eq!(line["tool"], "context_assembled");
        assert_eq!(line["arguments"], serde_json::to_value(receipt).unwrap());
        assert_eq!(line["result_preview"], "");
        assert_eq!(line["blocked"], false);
        let total: u64 = receipt
            .blocks
            .iter()
            .map(|b| u64::from(b.duration_ms))
            .sum();
        assert_eq!(line["duration_ms"], total);
    }
}

#[tokio::test]
async fn test_cr13_context_lines_should_share_the_turn_trace_and_parent_span() {
    let dir = tempfile::tempdir().unwrap();
    let (opts, path) = with_audit(dir.path());
    let mut r = rig(vec![vec![read("r1")]], opts);

    r.turn("look around").await;

    let all = lines(&path);
    let tool = all
        .iter()
        .find(|l| l["span_kind"] == "tool_call")
        .expect("the read_file call is audited too");
    let context: Vec<&Value> = all.iter().filter(|l| l["span_kind"] == "context").collect();
    assert_eq!(context.len(), 2);
    for line in &context {
        assert_eq!(line["trace_id"], tool["trace_id"], "the turn's trace id");
        assert!(line["parent_span_id"].is_string());
        assert_eq!(
            line["parent_span_id"], tool["parent_span_id"],
            "the turn span is the parent, as for tool calls"
        );
        assert_ne!(line["span_id"], tool["span_id"]);
    }
    assert_ne!(
        context[0]["span_id"], context[1]["span_id"],
        "each line is its own span"
    );
}

#[tokio::test]
async fn test_cr13_a_new_turn_should_use_a_new_trace_for_its_context_lines() {
    let dir = tempfile::tempdir().unwrap();
    let (opts, path) = with_audit(dir.path());
    let mut r = rig(vec![], opts);

    r.turn("one").await;
    r.turn("two").await;

    let all = lines(&path);
    let context: Vec<&Value> = all.iter().filter(|l| l["span_kind"] == "context").collect();
    assert_eq!(context.len(), 2);
    assert_ne!(context[0]["trace_id"], context[1]["trace_id"]);
}

#[tokio::test]
async fn test_cr13_without_an_egress_log_nothing_should_be_written() {
    let dir = tempfile::tempdir().unwrap();
    let mut r = rig(vec![], Opts::default());
    r.turn("one").await;
    assert!(!dir.path().join(".hone/audit.jsonl").exists());
}
