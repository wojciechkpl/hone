#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U1 of `docs/plans/usage-and-pricing-spec.md`: OpenAI-compatible servers send the usage
//! requested with `stream_options.include_usage` in a last chunk with `choices: []`, AFTER
//! the chunk that carries `finish_reason`. That usage must be reported (once), before the
//! stream's final `Done`; usage inline in the finish chunk and streams without usage keep
//! working; tool-call turns report usage the same way.

use futures::StreamExt;
use hone_proto::{Message, ModelId, ProviderEvent};
use hone_providers::{CompletionRequest, ModelProvider, OpenAiProvider};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn events_for(chunks: &[&str]) -> Vec<ProviderEvent> {
    let server = MockServer::start().await;
    let mut body = String::new();
    for c in chunks {
        body.push_str(&format!("data: {c}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    let provider = OpenAiProvider::with_base_url("test-key", server.uri());
    let req = CompletionRequest::new(ModelId::new("openai:gpt-4o"), vec![Message::user("hi")]);
    provider.complete(req).await.unwrap().collect().await
}

fn usages(events: &[ProviderEvent]) -> Vec<(u32, u32, u32)> {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Usage(u) => Some((u.input, u.output, u.cache_read)),
            _ => None,
        })
        .collect()
}

fn position(events: &[ProviderEvent], f: impl Fn(&ProviderEvent) -> bool) -> usize {
    events.iter().position(f).expect("event present")
}

/// The agent stops reading at the first `Done`, so the usage must come before it.
fn assert_usage_before_first_done(events: &[ProviderEvent]) {
    let usage_at = position(events, |e| matches!(e, ProviderEvent::Usage(_)));
    let first_done = position(events, |e| matches!(e, ProviderEvent::Done));
    assert!(usage_at < first_done, "usage precedes the first Done: {events:?}");
    assert!(matches!(events.last(), Some(ProviderEvent::Done)), "Done is last: {events:?}");
}

const TEXT: &str = r#"{"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}"#;
const STOP: &str = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
const TRAILING: &str = r#"{"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7,"total_tokens":18}}"#;

#[tokio::test]
async fn test_u1_usage_after_the_finish_chunk_should_be_reported_once_before_done() {
    let events = events_for(&[TEXT, STOP, TRAILING]).await;

    assert_eq!(usages(&events), [(11, 7, 0)], "{events:?}");
    assert_usage_before_first_done(&events);
}

#[tokio::test]
async fn test_u1_the_trailing_chunk_should_carry_cached_tokens_too() {
    let trailing = r#"{"choices":[],"usage":{"prompt_tokens":1500,"completion_tokens":3,"prompt_tokens_details":{"cached_tokens":1024}}}"#;

    let events = events_for(&[TEXT, STOP, trailing]).await;

    assert_eq!(usages(&events), [(1500, 3, 1024)], "{events:?}");
    assert_usage_before_first_done(&events);
}

#[tokio::test]
async fn test_u1_usage_inline_in_the_finish_chunk_should_still_work() {
    let inline = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":11,"completion_tokens":7}}"#;

    let events = events_for(&[TEXT, inline]).await;

    assert_eq!(usages(&events), [(11, 7, 0)], "{events:?}");
    assert_usage_before_first_done(&events);
}

#[tokio::test]
async fn test_u1_a_stream_without_usage_should_still_finish() {
    let events = events_for(&[TEXT, STOP]).await;

    assert!(usages(&events).is_empty(), "{events:?}");
    assert!(matches!(events.last(), Some(ProviderEvent::Done)), "{events:?}");
}

#[tokio::test]
async fn test_u1_a_tool_call_turn_should_report_trailing_usage_too() {
    let call = r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}}]},"finish_reason":null}]}"#;
    let finish = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#;

    let events = events_for(&[call, finish, TRAILING]).await;

    let calls: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::ToolCall(c) => Some(c.name.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(calls, ["read_file"], "{events:?}");
    assert_eq!(usages(&events), [(11, 7, 0)], "{events:?}");
    assert_usage_before_first_done(&events);
}
