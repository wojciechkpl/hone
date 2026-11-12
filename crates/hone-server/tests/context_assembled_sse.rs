#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR9 of `docs/plans/context-receipts-spec.md`: the SSE encoder names a context
//! receipt `context_assembled` (not `unknown`) and carries the receipt as JSON.

use hone_proto::{AgentEvent, ContextReceipt};
use hone_server::sse::encode_event;

#[test]
fn test_cr9_encode_event_should_name_and_carry_a_context_receipt() {
    let receipt = ContextReceipt {
        turn: 2,
        call: 3,
        model: "mock:test".to_owned(),
        request_sha256: "ab".repeat(32),
        blocks: vec![],
        dropped: vec![],
        compaction: vec![],
        tools_offered: vec!["read_file".to_owned()],
        history_len: 4,
        history_root: "cd".repeat(32),
    };

    let frame = encode_event(&AgentEvent::ContextAssembled { receipt }, 7);

    assert!(frame.contains("event: context_assembled"), "{frame}");
    assert!(!frame.contains("event: unknown"), "{frame}");
    assert!(frame.contains("id: 7"), "{frame}");
    let data = frame
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .expect("a data line");
    let v: serde_json::Value = serde_json::from_str(data).unwrap();
    assert_eq!(v["type"], "context_assembled");
    assert_eq!(v["receipt"]["turn"], 2);
    assert_eq!(v["receipt"]["call"], 3);
}
