#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Shared harness for the stream-ending tests (`docs/plans/stream-transport-error-spec.md`):
//! a raw HTTP/1.1 server with scripted pacing and endings, plus per-adapter body builders.
//!
//! (`stream_utf8_split.rs` keeps its own copy of the simpler always-complete server.)

use std::time::Duration;

use hone_proto::{Message, ProviderEvent};
use hone_providers::{
    AnthropicProvider, CompletionRequest, ModelProvider, OllamaProvider, OpenAiProvider,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Adapter {
    OpenAi,
    Anthropic,
    Ollama,
}

pub const ADAPTERS: [Adapter; 3] = [Adapter::OpenAi, Adapter::Anthropic, Adapter::Ollama];

impl Adapter {
    pub fn content_type(self) -> &'static str {
        match self {
            Adapter::Ollama => "application/x-ndjson",
            _ => "text/event-stream",
        }
    }
    pub fn model(self) -> &'static str {
        match self {
            Adapter::OpenAi => "openai:gpt-4o",
            Adapter::Anthropic => "anthropic:claude-sonnet-4-5",
            Adapter::Ollama => "ollama:llama3.2",
        }
    }
    /// The provider name the Error message must mention.
    pub fn name(self) -> &'static str {
        match self {
            Adapter::OpenAi => "openai",
            Adapter::Anthropic => "anthropic",
            Adapter::Ollama => "ollama",
        }
    }
}

/// One scripted action of the server.
#[derive(Clone, Debug)]
pub enum Step {
    /// Send these bytes as one HTTP chunk.
    Piece(Vec<u8>),
    /// Wait before the next step.
    Delay(Duration),
}

pub fn piece(s: impl Into<String>) -> Step {
    Step::Piece(s.into().into_bytes())
}

/// How the response ends after the last step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Terminating `0\r\n\r\n` chunk: a normal end.
    Normal,
    /// Close the socket without the terminating chunk: a body cut before its end.
    CutOff,
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

async fn handle(mut stream: TcpStream, content_type: &'static str, steps: Vec<Step>, end: End) {
    stream.set_nodelay(true).ok();
    read_request(&mut stream).await;
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    if stream.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    for step in &steps {
        match step {
            Step::Delay(d) => tokio::time::sleep(*d).await,
            Step::Piece(p) => {
                if p.is_empty() {
                    continue;
                }
                let mut out = format!("{:x}\r\n", p.len()).into_bytes();
                out.extend_from_slice(p);
                out.extend_from_slice(b"\r\n");
                if stream.write_all(&out).await.is_err() || stream.flush().await.is_err() {
                    return;
                }
                // Keep consecutive pieces from coalescing into one read.
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        }
    }
    if end == End::Normal {
        let _ = stream.write_all(b"0\r\n\r\n").await;
        let _ = stream.flush().await;
    }
    // CutOff: no terminating chunk, just close.
    let _ = stream.shutdown().await;
}

/// Serve `steps` then `end`; returns the base URL.
pub async fn serve(adapter: Adapter, steps: Vec<Step>, end: End) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let ct = adapter.content_type();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(handle(stream, ct, steps.clone(), end));
        }
    });
    url
}

/// Build the provider for `adapter` against `url`, optionally with an idle limit.
pub fn provider(adapter: Adapter, url: String, idle: Option<Duration>) -> Box<dyn ModelProvider> {
    match (adapter, idle) {
        (Adapter::OpenAi, None) => Box::new(OpenAiProvider::with_base_url("fixture-key", url)),
        (Adapter::OpenAi, Some(d)) => {
            Box::new(OpenAiProvider::with_base_url("fixture-key", url).with_idle_timeout(d))
        }
        (Adapter::Anthropic, None) => {
            Box::new(AnthropicProvider::with_base_url("fixture-key", url))
        }
        (Adapter::Anthropic, Some(d)) => {
            Box::new(AnthropicProvider::with_base_url("fixture-key", url).with_idle_timeout(d))
        }
        (Adapter::Ollama, None) => Box::new(OllamaProvider::new(url)),
        (Adapter::Ollama, Some(d)) => Box::new(OllamaProvider::new(url).with_idle_timeout(d)),
    }
}

/// Run one completion and collect every event (generous outer timeout so a hang fails).
pub async fn run(
    adapter: Adapter,
    steps: Vec<Step>,
    end: End,
    idle: Option<Duration>,
) -> Vec<ProviderEvent> {
    use futures::StreamExt;
    let url = serve(adapter, steps, end).await;
    let p = provider(adapter, url, idle);
    let req = CompletionRequest::new(adapter.model(), vec![Message::user("hi")]);
    tokio::time::timeout(Duration::from_secs(30), async {
        let stream = p.complete(req).await.expect("request accepted");
        stream.collect::<Vec<_>>().await
    })
    .await
    .expect("stream hung")
}

// -- event collection helpers -------------------------------------------------------------------------

pub fn text_of(events: &[ProviderEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

pub fn errors(events: &[ProviderEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Error { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

pub fn has_done(events: &[ProviderEvent]) -> bool {
    events.iter().any(|e| matches!(e, ProviderEvent::Done))
}

// -- bodies ---------------------------------------------------------------------------------------------

fn s(v: &Value) -> String {
    serde_json::to_string(v).unwrap()
}

fn anth(event: &str, v: Value) -> String {
    format!("event: {event}\ndata: {}\n\n", s(&v))
}

/// Anything that must precede text events.
pub fn head(a: Adapter) -> String {
    match a {
        Adapter::OpenAi | Adapter::Ollama => String::new(),
        Adapter::Anthropic => {
            anth(
                "message_start",
                json!({"type":"message_start","message":{"id":"m","usage":{"input_tokens":1,"output_tokens":0}}}),
            ) + &anth(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            )
        }
    }
}

/// One complete text event.
pub fn text(a: Adapter, t: &str) -> String {
    match a {
        Adapter::OpenAi => format!(
            "data: {}\n\n",
            s(&json!({"choices":[{"index":0,"delta":{"content":t},"finish_reason":null}]}))
        ),
        Adapter::Anthropic => anth(
            "content_block_delta",
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":t}}),
        ),
        Adapter::Ollama => format!(
            "{}\n",
            s(&json!({"model":"llama3.2","message":{"role":"assistant","content":t},"done":false}))
        ),
    }
}

/// The provider's completion marker (OpenAI: the `finish_reason` chunk; Anthropic:
/// `message_stop`; Ollama: the `done: true` line) -- without OpenAI's trailing `[DONE]`.
pub fn marker(a: Adapter) -> String {
    match a {
        Adapter::OpenAi => format!(
            "data: {}\n\n",
            s(&json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}))
        ),
        Adapter::Anthropic => {
            anth(
                "content_block_stop",
                json!({"type":"content_block_stop","index":0}),
            ) + &anth(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}),
            ) + &anth("message_stop", json!({"type":"message_stop"}))
        }
        Adapter::Ollama => format!(
            "{}\n",
            s(
                &json!({"model":"llama3.2","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":1,"eval_count":1})
            )
        ),
    }
}

/// An in-band error event with `message`.
pub fn in_band_error(a: Adapter, message: &str) -> String {
    match a {
        Adapter::OpenAi => format!(
            "data: {}\n\n",
            s(&json!({"error":{"message":message,"code":502}}))
        ),
        Adapter::Anthropic => anth(
            "error",
            json!({"type":"error","error":{"type":"overloaded_error","message":message}}),
        ),
        Adapter::Ollama => format!("{}\n", s(&json!({"error":message}))),
    }
}

/// What follows the marker on the wire for OpenAI (`[DONE]`), nothing for the others.
pub fn trailer(a: Adapter) -> String {
    match a {
        Adapter::OpenAi => "data: [DONE]\n\n".to_owned(),
        _ => String::new(),
    }
}
