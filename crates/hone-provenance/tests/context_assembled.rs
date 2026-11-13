#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR14 of `docs/plans/context-receipts-spec.md`:
//!
//! ```ignore
//! ProvenanceEvent::ContextAssembled {
//!     receipt_sha256: String,
//!     strategies: Vec<String>,              // block strategy names, in order
//!     sources: Vec<hone_proto::SourceRef>,  // every block's sources, in order
//! }
//! ```
//!
//! Wire: tagged `kind: "context_assembled"` like the other variants. A pure
//! addition: chains with and without it verify.

use hone_proto::SourceRef;
use hone_provenance::{ProvenanceEvent, ProvenanceLog};
use serde_json::json;

fn event() -> ProvenanceEvent {
    ProvenanceEvent::ContextAssembled {
        receipt_sha256: "ab".repeat(32),
        strategies: vec!["bm25".to_owned(), "skeleton:flat".to_owned()],
        sources: vec![
            SourceRef::Memory {
                id: "m-1".into(),
                version: None,
            },
            SourceRef::Skill { name: "git".into() },
        ],
    }
}

#[test]
fn test_cr14_event_should_have_the_documented_wire_shape_and_round_trip() {
    let v = serde_json::to_value(event()).unwrap();
    assert_eq!(v["kind"], "context_assembled");
    assert_eq!(v["receipt_sha256"], "ab".repeat(32));
    assert_eq!(v["strategies"], json!(["bm25", "skeleton:flat"]));
    assert_eq!(
        v["sources"][0],
        json!({ "kind": "memory", "id": "m-1", "version": null })
    );
    let back: ProvenanceEvent = serde_json::from_value(v).unwrap();
    assert_eq!(back, event());
}

#[test]
fn test_cr14_a_chain_with_the_variant_should_append_reopen_and_verify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chain.jsonl");
    {
        let mut log = ProvenanceLog::open(&path).unwrap();
        log.append(ProvenanceEvent::UserPrompt { text: "hi".into() })
            .unwrap();
        log.append(event()).unwrap();
        log.append(ProvenanceEvent::SessionEnd {
            reason: "done".into(),
        })
        .unwrap();
    }

    let log = ProvenanceLog::open(&path).expect("reopens and verifies");

    assert_eq!(log.len(), 3);
    assert_eq!(log.entries()[1].event, event());
    log.verify().unwrap();
}

#[test]
fn test_cr14_tampering_with_a_context_entry_should_break_verification() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("chain.jsonl");
    {
        let mut log = ProvenanceLog::open(&path).unwrap();
        log.append(ProvenanceEvent::UserPrompt { text: "hi".into() })
            .unwrap();
        log.append(event()).unwrap();
        log.append(ProvenanceEvent::SessionEnd {
            reason: "done".into(),
        })
        .unwrap();
    }
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replace("skeleton:flat", "skeleton:evil")).unwrap();

    assert!(
        ProvenanceLog::open(&path).is_err(),
        "an edited receipt entry must not verify"
    );
}
