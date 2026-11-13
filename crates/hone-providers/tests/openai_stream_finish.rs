#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U8 and U9 of `docs/plans/usage-and-pricing-spec.md` at the OpenAI-compatible adapter, with a
//! raw TCP server so the stream's timing is under the test's control (chunked body, delays,
//! a body that stays open):
//!
//! - U8: after `finish_reason` the adapter waits for a trailing usage chunk for at most ~2 s
//!   (or `[DONE]`, end of stream, or the usage chunk), then emits `Done`; a server that keeps
//!   the body open without `[DONE]` must not hold the turn for the idle timeout; a usage chunk
//!   arriving 1 s after the finish chunk is still counted; an error chunk after
//!   `finish_reason` does not fail the completed turn.
//! - U9: usage reported more than once in one response (inline then trailing, or cumulative per
//!   chunk) is counted once: the last report wins.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use futures::StreamExt;
use hone_proto::{Message, ModelId, ProviderEvent};
use hone_providers::{CompletionRequest, ModelProvider, OpenAiProvider};

/// One step of the server's script.
enum Step {
    Data(&'static str),
    Sleep(Duration),
}

/// `hold`: after the steps keep the connection open (no `[DONE]`, no end of body) for 30 s;
/// otherwise send `[DONE]` and end the body.
fn serve_once(steps: Vec<Step>, hold: bool) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else { return };
        drain_request(&mut stream);
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
        if stream.write_all(head.as_bytes()).is_err() {
            return;
        }
        let chunk = |s: &mut TcpStream, text: &str| {
            let _ = s.write_all(format!("{:x}\r\n{text}\r\n", text.len()).as_bytes());
            let _ = s.flush();
        };
        for step in steps {
            match step {
                Step::Data(json) => chunk(&mut stream, &format!("data: {json}\n\n")),
                Step::Sleep(d) => std::thread::sleep(d),
            }
        }
        if hold {
            std::thread::sleep(Duration::from_secs(30));
        } else {
            chunk(&mut stream, "data: [DONE]\n\n");
            let _ = stream.write_all(b"0\r\n\r\n");
        }
    });
    base
}

fn drain_request(stream: &mut TcpStream) {
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
        .unwrap_or(0);
    while buf.len() < header_end + len {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

const TEXT: &str = r#"{"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":null}]}"#;
const STOP: &str = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;

/// Collects events until the first `Done` or `limit` passes. Returns the events and how long
/// it took.
async fn collect(base: &str, limit: Duration) -> (Vec<ProviderEvent>, Duration) {
    let provider = OpenAiProvider::with_base_url("test-key", base.to_owned());
    let req = CompletionRequest::new(ModelId::new("openai:gpt-4o"), vec![Message::user("hi")]);
    let started = Instant::now();
    let mut stream = provider.complete(req).await.unwrap();
    let mut events = Vec::new();
    let _ = tokio::time::timeout(limit, async {
        while let Some(e) = stream.next().await {
            let done = matches!(e, ProviderEvent::Done);
            events.push(e);
            if done {
                break;
            }
        }
    })
    .await;
    (events, started.elapsed())
}

fn total_usage(events: &[ProviderEvent]) -> (u32, u32) {
    events.iter().fold((0, 0), |(i, o), e| match e {
        ProviderEvent::Usage(u) => (i + u.input, o + u.output),
        _ => (i, o),
    })
}

fn has_done(events: &[ProviderEvent]) -> bool {
    events.iter().any(|e| matches!(e, ProviderEvent::Done))
}

// ---- U8 ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_u8_a_held_open_body_after_finish_and_usage_should_not_delay_the_turn() {
    let usage = r#"{"choices":[],"usage":{"prompt_tokens":11,"completion_tokens":7}}"#;
    let base = serve_once(vec![Step::Data(TEXT), Step::Data(STOP), Step::Data(usage)], true);

    let (events, took) = collect(&base, Duration::from_secs(10)).await;

    assert!(has_done(&events), "the turn completes: {events:?}");
    assert_eq!(total_usage(&events), (11, 7), "{events:?}");
    assert!(took < Duration::from_secs(4), "the held body delayed the turn by {took:?}");
}

#[tokio::test]
async fn test_u8_a_held_open_body_after_finish_without_usage_should_wait_at_most_about_two_seconds() {
    let base = serve_once(vec![Step::Data(TEXT), Step::Data(STOP)], true);

    let (events, took) = collect(&base, Duration::from_secs(10)).await;

    assert!(has_done(&events), "{events:?}");
    assert!(took < Duration::from_secs(4), "waited {took:?} for a usage chunk that never came");
}

#[tokio::test]
async fn test_u8_a_usage_chunk_arriving_a_second_after_the_finish_chunk_should_still_be_counted() {
    let usage = r#"{"choices":[],"usage":{"prompt_tokens":21,"completion_tokens":9}}"#;
    let base = serve_once(
        vec![Step::Data(TEXT), Step::Data(STOP), Step::Sleep(Duration::from_secs(1)), Step::Data(usage)],
        true,
    );

    let (events, took) = collect(&base, Duration::from_secs(10)).await;

    assert_eq!(total_usage(&events), (21, 9), "{events:?}");
    assert!(has_done(&events), "{events:?}");
    assert!(took < Duration::from_secs(5), "{took:?}");
}

#[tokio::test]
async fn test_u8_an_error_chunk_after_finish_should_not_fail_the_completed_turn() {
    let error = r#"{"error":{"message":"upstream hiccup","type":"server_error"}}"#;
    let base = serve_once(vec![Step::Data(TEXT), Step::Data(STOP), Step::Data(error)], false);

    let (events, _) = collect(&base, Duration::from_secs(10)).await;

    assert!(
        !events.iter().any(|e| matches!(e, ProviderEvent::Error { .. })),
        "the completion stands: {events:?}"
    );
    assert!(has_done(&events), "{events:?}");
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "hello");
}

#[tokio::test]
async fn test_u8_an_error_chunk_before_finish_should_still_be_an_error() {
    let error = r#"{"error":{"message":"overloaded","type":"server_error"}}"#;
    let base = serve_once(vec![Step::Data(TEXT), Step::Data(error)], false);

    let (events, _) = collect(&base, Duration::from_secs(10)).await;

    assert!(
        events.iter().any(|e| matches!(e, ProviderEvent::Error { .. })),
        "an error before the finish is still an error: {events:?}"
    );
}

// ---- U9 ------------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_u9_inline_and_trailing_usage_should_be_counted_once_the_last_wins() {
    let inline = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
    let trailing = r#"{"choices":[],"usage":{"prompt_tokens":12,"completion_tokens":7}}"#;
    let base = serve_once(vec![Step::Data(TEXT), Step::Data(inline), Step::Data(trailing)], false);

    let (events, _) = collect(&base, Duration::from_secs(10)).await;

    assert_eq!(total_usage(&events), (12, 7), "not 22/12: {events:?}");
}

#[tokio::test]
async fn test_u9_cumulative_usage_on_every_chunk_should_be_counted_once() {
    let c1 = r#"{"choices":[{"index":0,"delta":{"content":"he"},"finish_reason":null}],"usage":{"prompt_tokens":10,"completion_tokens":1}}"#;
    let c2 = r#"{"choices":[{"index":0,"delta":{"content":"llo"},"finish_reason":null}],"usage":{"prompt_tokens":10,"completion_tokens":3}}"#;
    let c3 = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
    let base = serve_once(vec![Step::Data(c1), Step::Data(c2), Step::Data(c3)], false);

    let (events, _) = collect(&base, Duration::from_secs(10)).await;

    assert_eq!(total_usage(&events), (10, 5), "{events:?}");
}
