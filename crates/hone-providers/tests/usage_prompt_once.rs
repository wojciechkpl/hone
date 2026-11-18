#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Haiku 5.5 long-prompt tier, adapter invariant (spec H5b): a request's prompt (input, cache
//! read, cache write) is reported in exactly ONE usage event; every other usage event of the
//! request carries output only. The agent relies on it: a usage event with prompt tokens
//! starts a new request, an output-only event is priced with the request's prompt.

use futures::StreamExt;
use hone_proto::{Message, ModelId, ProviderEvent};
use hone_providers::{AnthropicProvider, CompletionRequest, ModelProvider, OpenAiProvider};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// (input, output, cache_read, cache_creation) of every usage event, in order.
fn usages(events: &[ProviderEvent]) -> Vec<(u32, u32, u32, u32)> {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Usage(u) => Some((u.input, u.output, u.cache_read, u.cache_creation)),
            _ => None,
        })
        .collect()
}

fn with_prompt(u: &(u32, u32, u32, u32)) -> bool {
    u.0 + u.2 + u.3 > 0
}

async fn serve(path_: &str, body: String) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path(path_))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body),
        )
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn test_h5b_anthropic_should_report_the_prompt_once_even_if_message_delta_repeats_input() {
    // The API may put a cumulative input_tokens (and cache counts) in message_delta's usage.
    let sse = concat!(
        "event: message_start\n",
        r#"data: {"type":"message_start","message":{"id":"m","model":"claude-haiku-5-5","usage":{"input_tokens":90000,"cache_read_input_tokens":20000,"cache_creation_input_tokens":500,"output_tokens":1}}}"#,
        "\n\n",
        "event: content_block_start\n",
        r#"data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        "\n\n",
        "event: content_block_delta\n",
        r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
        "\n\n",
        "event: content_block_stop\n",
        r#"data: {"type":"content_block_stop","index":0}"#,
        "\n\n",
        "event: message_delta\n",
        r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":90000,"cache_read_input_tokens":20000,"cache_creation_input_tokens":500,"output_tokens":1000}}"#,
        "\n\n",
        "event: message_stop\n",
        r#"data: {"type":"message_stop"}"#,
        "\n\n",
    );
    let server = serve("/v1/messages", sse.to_owned()).await;
    let provider = AnthropicProvider::with_base_url("test-key", server.uri());
    let req = CompletionRequest::new(
        ModelId::new("anthropic:claude-haiku-5-5"),
        vec![Message::user("hello")],
    );

    let events: Vec<ProviderEvent> = provider.complete(req).await.unwrap().collect().await;

    let u = usages(&events);
    let prompts: Vec<_> = u.iter().filter(|x| with_prompt(x)).collect();
    assert_eq!(
        prompts.len(),
        1,
        "exactly one usage event carries the prompt: {u:?}"
    );
    assert_eq!(
        (prompts[0].0, prompts[0].2, prompts[0].3),
        (90_000, 20_000, 500),
        "{u:?}"
    );
    let out: u32 = u.iter().map(|x| x.1).sum();
    assert_eq!(
        out, 1_000,
        "output is reported by the message_delta event: {u:?}"
    );
    let last = u.last().expect("a usage event after the prompt one");
    assert!(
        !with_prompt(last),
        "message_delta's event carries output only: {u:?}"
    );
}

#[tokio::test]
async fn test_h5b_openai_should_report_the_prompt_once_when_both_the_finish_and_trailing_chunks_have_usage()
 {
    let text = r#"{"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}"#;
    let finish = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":110000,"completion_tokens":7}}"#;
    let trailing = r#"{"choices":[],"usage":{"prompt_tokens":110000,"completion_tokens":7}}"#;
    let mut body = String::new();
    for c in [text, finish, trailing] {
        body.push_str(&format!("data: {c}\n\n"));
    }
    body.push_str("data: [DONE]\n\n");
    let server = serve("/v1/chat/completions", body).await;
    let provider = OpenAiProvider::with_base_url("test-key", server.uri());
    let req = CompletionRequest::new(ModelId::new("openai:gpt-4o"), vec![Message::user("hi")]);

    let events: Vec<ProviderEvent> = provider.complete(req).await.unwrap().collect().await;

    let u = usages(&events);
    assert_eq!(u.iter().filter(|x| with_prompt(x)).count(), 1, "{u:?}");
}
