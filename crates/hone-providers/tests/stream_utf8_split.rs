#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Red-phase tests for `docs/plans/stream-utf8-split-spec.md` (U1-U7): multi-byte UTF-8
//! characters split across HTTP stream chunks in the OpenAI-compatible, Anthropic and Ollama
//! adapters.
//!
//! wiremock answers with the whole body in one piece, so these tests use a small raw HTTP/1.1
//! server (tokio `TcpListener` on 127.0.0.1:0). It reads the request (headers plus
//! `Content-Length` body) and replies `200` with `Transfer-Encoding: chunked`, writing every
//! piece as its own HTTP chunk (a short pause between pieces keeps them from coalescing), then
//! `0\r\n\r\n`. Token text is compared concatenated, never by token boundaries.
//! Every wait has a timeout.

use std::time::Duration;

use futures::StreamExt;
use hone_proto::{Message, ProviderEvent};
use hone_providers::{
    AnthropicProvider, CompletionRequest, ModelProvider, OllamaProvider, OpenAiProvider,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// -- raw chunked HTTP server -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Adapter {
    OpenAi,
    Anthropic,
    Ollama,
}

impl Adapter {
    fn content_type(self) -> &'static str {
        match self {
            Adapter::Ollama => "application/x-ndjson",
            _ => "text/event-stream",
        }
    }
    fn model(self) -> &'static str {
        match self {
            Adapter::OpenAi => "openai:gpt-4o",
            Adapter::Anthropic => "anthropic:claude-sonnet-4-5",
            Adapter::Ollama => "ollama:llama3.2",
        }
    }
}

async fn read_request(stream: &mut TcpStream) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let len = head
        .lines()
        .find_map(|l| {
            l.strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < header_end + len {
        let n = stream.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
}

async fn handle(mut stream: TcpStream, content_type: &'static str, pieces: Vec<Vec<u8>>) {
    stream.set_nodelay(true).ok();
    read_request(&mut stream).await;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    for p in &pieces {
        if p.is_empty() {
            continue;
        }
        let mut out = format!("{:x}\r\n", p.len()).into_bytes();
        out.extend_from_slice(p);
        out.extend_from_slice(b"\r\n");
        if stream.write_all(&out).await.is_err() || stream.flush().await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let _ = stream.write_all(b"0\r\n\r\n").await;
    let _ = stream.flush().await;
    let _ = stream.shutdown().await;
}

/// Serve `pieces` as separate HTTP chunks and return the base URL.
async fn serve(adapter: Adapter, pieces: Vec<Vec<u8>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let ct = adapter.content_type();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(handle(stream, ct, pieces.clone()));
        }
    });
    url
}

/// Run one completion against a server that sends `pieces`, collecting every event.
async fn run(adapter: Adapter, pieces: Vec<Vec<u8>>) -> Vec<ProviderEvent> {
    let url = serve(adapter, pieces).await;
    let provider: Box<dyn ModelProvider> = match adapter {
        Adapter::OpenAi => Box::new(OpenAiProvider::with_base_url("fixture-key", url)),
        Adapter::Anthropic => Box::new(AnthropicProvider::with_base_url("fixture-key", url)),
        Adapter::Ollama => Box::new(OllamaProvider::new(url)),
    };
    let req = CompletionRequest::new(adapter.model(), vec![Message::user("hi")]);
    tokio::time::timeout(Duration::from_secs(30), async {
        let stream = provider.complete(req).await.expect("request accepted");
        stream.collect::<Vec<_>>().await
    })
    .await
    .expect("stream hung")
}

// -- bodies --------------------------------------------------------------------------------------

fn s(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

/// Text events for `texts` (one event each) plus a normal end of stream.
fn text_body(adapter: Adapter, texts: &[&str]) -> Vec<u8> {
    let mut out = String::new();
    match adapter {
        Adapter::OpenAi => {
            for t in texts {
                out += &format!(
                    "data: {}\n\n",
                    s(&json!({"choices":[{"index":0,"delta":{"content":t},"finish_reason":null}]}))
                );
            }
            out += &format!(
                "data: {}\n\n",
                s(&json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}))
            );
            out += "data: [DONE]\n\n";
        }
        Adapter::Anthropic => {
            out += &format!(
                "event: message_start\ndata: {}\n\n",
                s(
                    &json!({"type":"message_start","message":{"id":"m","usage":{"input_tokens":1,"output_tokens":0}}})
                )
            );
            out += &format!(
                "event: content_block_start\ndata: {}\n\n",
                s(
                    &json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})
                )
            );
            for t in texts {
                out += &format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    s(
                        &json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":t}})
                    )
                );
            }
            out += &format!(
                "event: content_block_stop\ndata: {}\n\n",
                s(&json!({"type":"content_block_stop","index":0}))
            );
            out += &format!(
                "event: message_delta\ndata: {}\n\n",
                s(
                    &json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}})
                )
            );
            out += &format!(
                "event: message_stop\ndata: {}\n\n",
                s(&json!({"type":"message_stop"}))
            );
        }
        Adapter::Ollama => {
            for t in texts {
                out += &format!(
                    "{}\n",
                    s(
                        &json!({"model":"llama3.2","message":{"role":"assistant","content":t},"done":false})
                    )
                );
            }
            out += &format!(
                "{}\n",
                s(
                    &json!({"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":1,"eval_count":1})
                )
            );
        }
    }
    out.into_bytes()
}

/// A tool call `read_file` whose arguments JSON is `args` (non-ASCII), delivered as two
/// argument fragments, followed by a normal end. OpenAI and Anthropic only.
fn tool_body(adapter: Adapter, args: &str) -> Vec<u8> {
    // Split the argument text in the middle (on a char boundary).
    let mid = args.char_indices().nth(args.chars().count() / 2).unwrap().0;
    let (a, b) = args.split_at(mid);
    let mut out = String::new();
    match adapter {
        Adapter::OpenAi => {
            out += &format!(
                "data: {}\n\n",
                s(
                    &json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read_file","arguments":a}}]},"finish_reason":null}]})
                )
            );
            out += &format!(
                "data: {}\n\n",
                s(
                    &json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":b}}]},"finish_reason":null}]})
                )
            );
            out += &format!(
                "data: {}\n\n",
                s(&json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}))
            );
            out += "data: [DONE]\n\n";
        }
        Adapter::Anthropic => {
            out += &format!(
                "event: message_start\ndata: {}\n\n",
                s(
                    &json!({"type":"message_start","message":{"id":"m","usage":{"input_tokens":1,"output_tokens":0}}})
                )
            );
            out += &format!(
                "event: content_block_start\ndata: {}\n\n",
                s(
                    &json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_1","name":"read_file","input":{}}})
                )
            );
            for frag in [a, b] {
                out += &format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    s(
                        &json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":frag}})
                    )
                );
            }
            out += &format!(
                "event: content_block_stop\ndata: {}\n\n",
                s(&json!({"type":"content_block_stop","index":0}))
            );
            out += &format!(
                "event: message_delta\ndata: {}\n\n",
                s(
                    &json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":1}})
                )
            );
            out += &format!(
                "event: message_stop\ndata: {}\n\n",
                s(&json!({"type":"message_stop"}))
            );
        }
        Adapter::Ollama => panic!("tool bodies are OpenAI / Anthropic only"),
    }
    out.into_bytes()
}

// -- helpers ---------------------------------------------------------------------------------------

fn cut(body: &[u8], at: &[usize]) -> Vec<Vec<u8>> {
    let mut pieces = Vec::new();
    let mut prev = 0;
    for &i in at {
        pieces.push(body[prev..i].to_vec());
        prev = i;
    }
    pieces.push(body[prev..].to_vec());
    pieces
}

fn one_byte_pieces(body: &[u8]) -> Vec<Vec<u8>> {
    body.iter().map(|b| vec![*b]).collect()
}

/// Byte offsets that fall strictly inside a multi-byte character of a valid UTF-8 body.
fn inner_offsets(body: &[u8]) -> Vec<usize> {
    let text = std::str::from_utf8(body).unwrap();
    text.char_indices()
        .flat_map(|(i, c)| (1..c.len_utf8()).map(move |k| i + k))
        .collect()
}

fn text_of(events: &[ProviderEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn tool_calls(events: &[ProviderEvent]) -> Vec<(String, String, Value)> {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::ToolCall(c) => Some((c.id.clone(), c.name.clone(), c.arguments.clone())),
            _ => None,
        })
        .collect()
}

fn has_done(events: &[ProviderEvent]) -> bool {
    events.iter().any(|e| matches!(e, ProviderEvent::Done))
}

const CHARS: [&str; 4] = ["ą", "€", "中", "😀"];
const ADAPTERS: [Adapter; 3] = [Adapter::OpenAi, Adapter::Anthropic, Adapter::Ollama];

// -- U1 / U2 / U3: characters split across chunks ----------------------------------------------------

async fn assert_split_chars_survive(adapter: Adapter) {
    for ch in CHARS {
        let text = format!("ab{ch}cd");
        let body = text_body(adapter, &[&text, " tail"]);
        let char_pos = body
            .windows(ch.len())
            .position(|w| w == ch.as_bytes())
            .expect("char in body");
        for k in 1..ch.len() {
            let pieces = cut(&body, &[char_pos + k]);
            let events = run(adapter, pieces).await;
            assert_eq!(
                text_of(&events),
                format!("{text} tail"),
                "{adapter:?}: {ch:?} cut after {k} of {} bytes",
                ch.len()
            );
            assert!(
                has_done(&events),
                "{adapter:?}: {ch:?} cut {k}: Done missing: {events:?}"
            );
        }
    }
}

#[tokio::test]
async fn test_u1_openai_characters_split_across_chunks_arrive_intact() {
    assert_split_chars_survive(Adapter::OpenAi).await;
}

#[tokio::test]
async fn test_u2_anthropic_characters_split_across_chunks_arrive_intact() {
    assert_split_chars_survive(Adapter::Anthropic).await;
}

#[tokio::test]
async fn test_u3_ollama_characters_split_across_chunks_arrive_intact() {
    assert_split_chars_survive(Adapter::Ollama).await;
}

#[tokio::test]
async fn test_u1_u3_baseline_single_chunk_delivers_multibyte_text_for_every_adapter() {
    for adapter in ADAPTERS {
        let text = "Zażółć € 中 😀";
        let events = run(adapter, vec![text_body(adapter, &[text])]).await;
        assert_eq!(text_of(&events), text, "{adapter:?}");
        assert!(has_done(&events), "{adapter:?}");
    }
}

// -- U4: tool-call arguments ------------------------------------------------------------------------------

async fn assert_tool_args_survive(adapter: Adapter) {
    let args = r#"{"path":"zażółć-中-😀.txt"}"#;
    let expected: Value = serde_json::from_str(args).unwrap();
    let body = tool_body(adapter, args);
    let baseline = tool_calls(&run(adapter, vec![body.clone()]).await);
    assert_eq!(
        baseline.len(),
        1,
        "{adapter:?}: single-chunk baseline: {baseline:?}"
    );
    assert_eq!(baseline[0].2, expected, "{adapter:?}: baseline");

    for off in inner_offsets(&body) {
        let events = run(adapter, cut(&body, &[off])).await;
        let calls = tool_calls(&events);
        assert_eq!(calls.len(), 1, "{adapter:?}: cut at {off}: {events:?}");
        assert_eq!(
            calls[0].0, baseline[0].0,
            "{adapter:?}: call id, cut at {off}"
        );
        assert_eq!(calls[0].1, "read_file", "{adapter:?}: cut at {off}");
        assert_eq!(calls[0].2, expected, "{adapter:?}: arguments, cut at {off}");
    }
}

#[tokio::test]
async fn test_u4_openai_tool_call_arguments_split_mid_character_arrive_intact() {
    assert_tool_args_survive(Adapter::OpenAi).await;
}

#[tokio::test]
async fn test_u4_anthropic_tool_call_arguments_split_mid_character_arrive_intact() {
    assert_tool_args_survive(Adapter::Anthropic).await;
}

// -- U5: any chunking gives the same result ------------------------------------------------------------------

fn non_token_events(events: &[ProviderEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| !matches!(e, ProviderEvent::Token { .. }))
        .map(|e| format!("{e:?}"))
        .collect()
}

async fn assert_chunking_is_irrelevant(adapter: Adapter, body: Vec<u8>) {
    let whole = run(adapter, vec![body.clone()]).await;
    let bytewise = run(adapter, one_byte_pieces(&body)).await;
    assert_eq!(text_of(&bytewise), text_of(&whole), "{adapter:?}: text");
    assert!(
        !text_of(&whole).is_empty() || !tool_calls(&whole).is_empty(),
        "{adapter:?}: baseline empty"
    );
    assert_eq!(
        non_token_events(&bytewise),
        non_token_events(&whole),
        "{adapter:?}: other events"
    );
}

#[tokio::test]
async fn test_u5_one_byte_per_chunk_equals_single_chunk_for_text() {
    for adapter in ADAPTERS {
        let body = text_body(adapter, &["Zażółć gęślą jaźń ", "€ 中文 ", "😀🎉"]);
        assert_chunking_is_irrelevant(adapter, body).await;
    }
}

#[tokio::test]
async fn test_u5_one_byte_per_chunk_equals_single_chunk_for_a_tool_call() {
    for adapter in [Adapter::OpenAi, Adapter::Anthropic] {
        let body = tool_body(adapter, r#"{"path":"zażółć-中-😀.txt"}"#);
        assert_chunking_is_irrelevant(adapter, body).await;
    }
}

// -- U6: an invalid byte does not drop its chunk -----------------------------------------------------------------

/// Two text events; the first contains `ab 0xFF cd`.
fn invalid_byte_body(adapter: Adapter) -> Vec<u8> {
    let marker = "ab\u{1}cd"; // placeholder, replaced by the raw 0xFF below
    let valid = text_body(adapter, &[marker, "second"]);
    // The JSON encoder writes U+0001 as the 6-byte escape `\u0001`; swap it for a raw 0xFF.
    let escaped = b"ab\\u0001cd";
    let pos = valid
        .windows(escaped.len())
        .position(|w| w == escaped)
        .expect("escaped marker");
    let mut out = valid[..pos].to_vec();
    out.extend_from_slice(b"ab");
    out.push(0xFF);
    out.extend_from_slice(b"cd");
    out.extend_from_slice(&valid[pos + escaped.len()..]);
    out
}

#[tokio::test]
async fn test_u6_invalid_byte_becomes_replacement_char_and_keeps_the_rest_of_the_chunk() {
    for adapter in ADAPTERS {
        let body = invalid_byte_body(adapter);
        let events = run(adapter, vec![body]).await;
        assert_eq!(
            text_of(&events),
            "ab\u{FFFD}cdsecond",
            "{adapter:?}: single chunk with an invalid byte"
        );
        assert!(has_done(&events), "{adapter:?}");
    }
}

#[tokio::test]
async fn test_u6_invalid_byte_at_a_chunk_boundary_is_replaced_too() {
    for adapter in ADAPTERS {
        let body = invalid_byte_body(adapter);
        let ff = body.iter().position(|b| *b == 0xFF).unwrap();
        for at in [ff, ff + 1] {
            let events = run(adapter, cut(&body, &[at])).await;
            assert_eq!(
                text_of(&events),
                "ab\u{FFFD}cdsecond",
                "{adapter:?}: cut at {at}"
            );
        }
    }
}

// -- U7: end of stream with an incomplete sequence -------------------------------------------------------------------

#[tokio::test]
async fn test_u7_body_ending_in_a_lone_lead_byte_delivers_earlier_events_and_ends() {
    for adapter in ADAPTERS {
        let mut body = text_body(adapter, &["hello ", "wörld"]);
        body.push(0xE2); // first byte of a 3-byte character, never completed

        // Same chunk as the complete events, and as its own trailing chunk.
        for pieces in [vec![body.clone()], cut(&body, &[body.len() - 1])] {
            let events = run(adapter, pieces).await;
            assert_eq!(text_of(&events), "hello wörld", "{adapter:?}");
            assert!(has_done(&events), "{adapter:?}: {events:?}");
        }
    }
}
