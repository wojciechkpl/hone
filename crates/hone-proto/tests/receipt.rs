#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! CR2 (hashing) and the type contract of `docs/plans/context-receipts-spec.md`:
//! the receipt types, their wire shape, and the two canonical hashes.
//!
//! API required (new): module `hone_proto::receipt`, everything re-exported at the
//! crate root:
//!
//! ```ignore
//! #[serde(rename_all = "snake_case")]
//! pub enum Slot { SystemPrompt, Recall, Skills, Injection, History, Retrieval }
//!
//! pub struct StrategyId { pub slot: Slot, pub name: String, pub version: String, pub params_sha256: String }
//!
//! #[serde(tag = "kind", rename_all = "snake_case")]
//! pub enum SourceRef {
//!     Memory { id: String, version: Option<u64> },
//!     File { path: PathBuf, sha256: Option<String>, lines: Option<(u32, u32)> },
//!     Message { index: usize, sha256: String },
//!     ToolResult { call_id: String, sha256: String },
//!     Skill { name: String },
//!     Command { argv: Vec<String>, sha256: String },
//!     Chunk { index: String, chunk_id: String, sha256: String },
//!     Opaque { plugin: String, note: String },
//! }
//! pub struct ScoredSource { pub source: SourceRef, pub score: Option<f32> }
//! pub struct BlockReceipt { pub slot: Slot, pub strategy: StrategyId, pub tokens: u32,
//!                           pub sources: Vec<ScoredSource>, pub duration_ms: u32 }
//! pub struct CompactionStep { pub strategy: StrategyId, pub replaced: Vec<SourceRef>,
//!                             pub derived: Option<SourceRef> }
//! pub struct ContextReceipt { pub turn: u32, pub call: u32, pub model: String,
//!     pub request_sha256: String, pub blocks: Vec<BlockReceipt>, pub dropped: Vec<SourceRef>,
//!     pub compaction: Vec<CompactionStep>, pub tools_offered: Vec<String>,
//!     pub history_len: u32,        // CR15: non-system messages in the request
//!     pub history_root: String }   // CR15: sha256_hex of the newline-joined message hashes
//! // all: Debug + Clone + PartialEq + Serialize + Deserialize
//!
//! AgentEvent::ContextAssembled { receipt: ContextReceipt }   // wire: {"type":"context_assembled","receipt":{..}}
//!
//! pub fn sha256_hex(bytes: &[u8]) -> String;                       // lowercase hex
//! /// sha256_hex of the compact JSON of `{"messages":[<each message as JSON>],"tools":[names]}`
//! /// with object keys in sorted order (serde_json::Value's default map).
//! pub fn request_sha256(messages: &[Message], tool_names: &[String]) -> String;
//! /// sha256_hex of the compact, key-sorted JSON of the message
//! /// (role, content, tool_calls, tool_call_id; empty fields are omitted as in serde).
//! pub fn message_sha256(message: &Message) -> String;
//! ```

use std::path::PathBuf;

use hone_proto::{
    AgentEvent, BlockReceipt, CompactionStep, ContextReceipt, Message, ScoredSource, Slot,
    SourceRef, StrategyId, message_sha256, request_sha256, sha256_hex,
};
use serde_json::json;

fn strategy(slot: Slot, name: &str) -> StrategyId {
    StrategyId {
        slot,
        name: name.to_owned(),
        version: "0.1.0".to_owned(),
        params_sha256: "ab".repeat(32),
    }
}

fn sample_receipt() -> ContextReceipt {
    ContextReceipt {
        turn: 2,
        call: 3,
        model: "mock:test".to_owned(),
        request_sha256: "cd".repeat(32),
        blocks: vec![
            BlockReceipt {
                slot: Slot::Recall,
                strategy: strategy(Slot::Recall, "bm25"),
                tokens: 42,
                sources: vec![
                    ScoredSource {
                        source: SourceRef::Memory {
                            id: "m-1".into(),
                            version: None,
                        },
                        score: Some(1.5),
                    },
                    ScoredSource {
                        source: SourceRef::File {
                            path: PathBuf::from("src/auth.rs"),
                            sha256: None,
                            lines: Some((3, 9)),
                        },
                        score: None,
                    },
                ],
                duration_ms: 7,
            },
            BlockReceipt {
                slot: Slot::History,
                strategy: strategy(Slot::History, "history"),
                tokens: 100,
                sources: vec![ScoredSource {
                    source: SourceRef::Message {
                        index: 1,
                        sha256: "ef".repeat(32),
                    },
                    score: None,
                }],
                duration_ms: 0,
            },
        ],
        dropped: vec![SourceRef::Memory {
            id: "m-9".into(),
            version: Some(4),
        }],
        compaction: vec![CompactionStep {
            strategy: strategy(Slot::History, "graduated"),
            replaced: vec![SourceRef::Message {
                index: 4,
                sha256: "01".repeat(32),
            }],
            derived: Some(SourceRef::Message {
                index: 4,
                sha256: "02".repeat(32),
            }),
        }],
        tools_offered: vec!["read_file".into(), "shell".into()],
        history_len: 7,
        history_root: "ef".repeat(32),
    }
}

// ---- wire shape -----------------------------------------------------------------

#[test]
fn test_cr_receipt_should_round_trip_through_json() {
    let r = sample_receipt();
    let text = serde_json::to_string(&r).unwrap();
    let back: ContextReceipt = serde_json::from_str(&text).unwrap();
    assert_eq!(back, r);
}

#[test]
fn test_cr_every_source_ref_variant_should_round_trip_with_a_snake_case_kind_tag() {
    let table: Vec<(SourceRef, &str)> = vec![
        (
            SourceRef::Memory {
                id: "a".into(),
                version: Some(3),
            },
            "memory",
        ),
        (
            SourceRef::File {
                path: PathBuf::from("a/b.rs"),
                sha256: Some("00".into()),
                lines: Some((1, 2)),
            },
            "file",
        ),
        (
            SourceRef::Message {
                index: 0,
                sha256: "00".into(),
            },
            "message",
        ),
        (
            SourceRef::ToolResult {
                call_id: "c1".into(),
                sha256: "00".into(),
            },
            "tool_result",
        ),
        (SourceRef::Skill { name: "git".into() }, "skill"),
        (
            SourceRef::Command {
                argv: vec!["env".into()],
                sha256: "00".into(),
            },
            "command",
        ),
        (
            SourceRef::Chunk {
                index: "docs".into(),
                chunk_id: "c9".into(),
                sha256: "00".into(),
            },
            "chunk",
        ),
        (
            SourceRef::Opaque {
                plugin: "wiki".into(),
                note: "n".into(),
            },
            "opaque",
        ),
    ];
    for (source, kind) in table {
        let v = serde_json::to_value(&source).unwrap();
        assert_eq!(v["kind"], kind, "{v}");
        let back: SourceRef = serde_json::from_value(v).unwrap();
        assert_eq!(back, source);
    }
}

#[test]
fn test_cr_memory_source_should_have_the_documented_wire_shape() {
    let v = serde_json::to_value(SourceRef::Memory {
        id: "m-1".into(),
        version: None,
    })
    .unwrap();
    assert_eq!(v, json!({ "kind": "memory", "id": "m-1", "version": null }));
}

#[test]
fn test_cr_slots_should_be_snake_case_on_the_wire() {
    let table = [
        (Slot::SystemPrompt, "system_prompt"),
        (Slot::Recall, "recall"),
        (Slot::Skills, "skills"),
        (Slot::Injection, "injection"),
        (Slot::History, "history"),
        (Slot::Retrieval, "retrieval"),
    ];
    for (slot, wire) in table {
        assert_eq!(serde_json::to_value(slot).unwrap(), json!(wire));
    }
}

#[test]
fn test_cr_context_assembled_event_should_be_tagged_and_round_trip() {
    let event = AgentEvent::ContextAssembled {
        receipt: sample_receipt(),
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["type"], "context_assembled");
    assert_eq!(v["receipt"]["turn"], 2);
    assert_eq!(v["receipt"]["blocks"][0]["slot"], "recall");
    let back: AgentEvent = serde_json::from_value(v).unwrap();
    match back {
        AgentEvent::ContextAssembled { receipt } => assert_eq!(receipt, sample_receipt()),
        other => panic!("{other:?}"),
    }
}

// ---- CR2: the canonical hashes -----------------------------------------------------

#[test]
fn test_cr2_sha256_hex_should_be_lowercase_hex() {
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn test_cr2_empty_request_should_hash_the_canonical_empty_object() {
    assert_eq!(
        request_sha256(&[], &[]),
        "1205e40e10295fd368f7a3e7de2fbf563275633c2549863b034417ed0df76e72"
    );
}

#[test]
fn test_cr2_request_hash_should_cover_messages_and_tool_names_in_order() {
    let messages = vec![Message::user("hi")];
    let tools = vec!["a".to_owned(), "b".to_owned()];
    assert_eq!(
        request_sha256(&messages, &tools),
        "ef9b76a2c54c16988126a2f871df751676b440fb4c0aea3346ee210d794e8312",
        "sha256 of the compact key-sorted JSON object with `messages` and `tools`"
    );
}

#[test]
fn test_cr2_identical_requests_should_hash_equal_and_any_change_should_differ() {
    let m = vec![Message::user("hi"), Message::assistant("hello")];
    let t = vec!["a".to_owned()];
    let base = request_sha256(&m, &t);
    assert_eq!(base, request_sha256(&m.clone(), &t.clone()));
    assert_ne!(
        base,
        request_sha256(&[Message::user("hi"), Message::assistant("hello!")], &t)
    );
    assert_ne!(base, request_sha256(&m, &["b".to_owned()]));
    assert_ne!(base, request_sha256(&m, &[]));
    assert_ne!(
        base,
        request_sha256(&[m[1].clone(), m[0].clone()], &t),
        "order matters"
    );
    assert_ne!(
        request_sha256(&m, &["a".into(), "b".into()]),
        request_sha256(&m, &["b".into(), "a".into()]),
        "tool order matters"
    );
}

#[test]
fn test_cr7_message_hash_should_be_the_sha256_of_its_canonical_json() {
    assert_eq!(
        message_sha256(&Message::user("hi")),
        "9017285104d1b249960a30732b8e92f6e2fb3acf8d8e4b2a16c116ad0c1ed211",
        "sha256 of the compact key-sorted JSON of the message"
    );
}

#[test]
fn test_cr7_message_hash_should_cover_role_text_tool_calls_and_call_id() {
    let base = message_sha256(&Message::user("x"));
    assert_ne!(base, message_sha256(&Message::assistant("x")));
    assert_ne!(base, message_sha256(&Message::user("y")));
    assert_ne!(
        message_sha256(&Message::tool_result("c1".into(), "out")),
        message_sha256(&Message::tool_result("c2".into(), "out")),
        "the tool-call id is hashed"
    );
    let mut with_call = Message::assistant("x");
    with_call.tool_calls.push(hone_proto::ToolCallRequest {
        id: "c1".into(),
        name: "read_file".into(),
        arguments: json!({ "path": "a" }),
    });
    assert_ne!(
        message_sha256(&with_call),
        message_sha256(&Message::assistant("x"))
    );
}

#[test]
fn test_cr15_receipt_should_serialise_history_len_and_history_root_under_those_names() {
    let v = serde_json::to_value(sample_receipt()).unwrap();
    assert_eq!(v["history_len"], 7);
    assert_eq!(v["history_root"], "ef".repeat(32));
}
