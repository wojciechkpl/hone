use hone_proto::{AgentEvent, TokenUsage};
use hone_server::sse::encode_event;

#[test]
fn test_encode_token_event() {
    let event = AgentEvent::Token {
        text: "hello".to_owned(),
    };
    let encoded = encode_event(&event, 1);

    assert!(encoded.starts_with("id: 1\n"), "missing id line");
    assert!(encoded.contains("event: token\n"), "missing event type");
    assert!(encoded.contains("data: "), "missing data line");
    assert!(encoded.ends_with("\n\n"), "missing trailing newlines");
    assert!(encoded.contains("hello"), "missing token text in data");
}

#[test]
fn test_encode_done_event() {
    let event = AgentEvent::Done {
        usage: TokenUsage {
            input: 10,
            output: 20,
            cache_read: 0,
        },
    };
    let encoded = encode_event(&event, 42);

    assert!(encoded.starts_with("id: 42\n"), "missing id line");
    assert!(encoded.contains("event: done\n"), "missing event type");
    assert!(encoded.contains("data: "), "missing data line");
    assert!(encoded.ends_with("\n\n"), "missing trailing newlines");
    // usage fields should appear in serialized JSON
    assert!(encoded.contains("\"input\""), "missing input field");
    assert!(encoded.contains("\"output\""), "missing output field");
}

#[test]
fn test_encode_error_event() {
    let event = AgentEvent::Error {
        message: "something went wrong".to_owned(),
    };
    let encoded = encode_event(&event, 7);

    assert!(encoded.starts_with("id: 7\n"), "missing id line");
    assert!(encoded.contains("event: error\n"), "missing event type");
    assert!(encoded.contains("data: "), "missing data line");
    assert!(encoded.ends_with("\n\n"), "missing trailing newlines");
    assert!(
        encoded.contains("something went wrong"),
        "missing error message"
    );
}
