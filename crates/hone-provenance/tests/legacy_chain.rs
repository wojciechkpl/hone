#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR14: chains written before `ProvenanceEvent::ContextAssembled` existed still
//! parse and verify. Compiles and passes against today's code; it must keep
//! passing.

use hone_provenance::{ProvenanceEvent, ProvenanceLog};
use serde_json::json;

#[test]
fn test_cr14_a_chain_without_the_new_variant_should_reopen_and_verify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chain.jsonl");
    {
        let mut log = ProvenanceLog::open(&path).unwrap();
        log.append(ProvenanceEvent::UserPrompt { text: "hi".into() })
            .unwrap();
        log.append(ProvenanceEvent::ToolCall {
            name: "read_file".into(),
            arguments: json!({}),
        })
        .unwrap();
        log.append(ProvenanceEvent::SessionEnd {
            reason: "done".into(),
        })
        .unwrap();
    }

    let log = ProvenanceLog::open(&path).expect("reopens and verifies");

    assert_eq!(log.len(), 3);
    log.verify().unwrap();
}
