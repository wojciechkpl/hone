#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W7 of `docs/plans/memory-write-tool-spec.md`: the server's event-name maps send
//! `AgentEvent::MemoryWritten` as `memory_written` (SSE, execution and orchestration
//! streams), not `unknown`.

use hone_proto::AgentEvent;
use hone_server::sse::encode_event;

#[test]
fn test_w7_encode_event_should_name_and_carry_a_memory_written_event() {
    let event = AgentEvent::MemoryWritten {
        id: "0190-abc".to_owned(),
        kind: "feedback".to_owned(),
        status: "added".to_owned(),
    };

    let frame = encode_event(&event, 3);

    assert!(frame.contains("event: memory_written"), "{frame}");
    assert!(!frame.contains("event: unknown"), "{frame}");
    let data = frame.lines().find_map(|l| l.strip_prefix("data: ")).expect("a data line");
    let v: serde_json::Value = serde_json::from_str(data).unwrap();
    assert_eq!(v["type"], "memory_written");
    assert_eq!(v["id"], "0190-abc");
    assert_eq!(v["kind"], "feedback");
    assert_eq!(v["status"], "added");
}

#[test]
fn test_w7_the_event_should_serialise_with_snake_case_type_and_round_trip() {
    let event = AgentEvent::MemoryWritten {
        id: "x".to_owned(),
        kind: "user".to_owned(),
        status: "updated".to_owned(),
    };
    let v = serde_json::to_value(&event).unwrap();
    assert_eq!(v["type"], "memory_written");
    let back: AgentEvent = serde_json::from_value(v).unwrap();
    assert!(matches!(back, AgentEvent::MemoryWritten { ref status, .. } if status == "updated"));
}
