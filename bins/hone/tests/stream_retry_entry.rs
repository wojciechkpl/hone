#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R11 (`docs/plans/stream-retry-fallback-spec.md`): through the `hone` binary (`--json`,
//! OpenAI-compatible mock, default `RetryConfig`), a response that fails before any text is
//! retried. The mock counts requests and scripts one response per request (the last script entry
//! repeats). A failure after output must make exactly one request.
//!
//! Credential-shaped fixtures are built at runtime. The child has a timeout.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum Reply {
    /// An in-band error before any content, then `[DONE]`.
    ErrorBeforeText,
    /// A normal answer.
    Answer,
    /// Some text, then an in-band error (a failure after output).
    ErrorAfterText,
}

fn sse(v: serde_json::Value) -> String {
    format!("data: {v}\n\n")
}

fn body_for(reply: Reply) -> String {
    let error = sse(
        serde_json::json!({"error":{"message":"Provider disconnected unexpectedly","code":502}}),
    );
    let text = sse(
        serde_json::json!({"choices":[{"index":0,"delta":{"content":"partial answer"},"finish_reason":null}]}),
    );
    let finish = sse(
        serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}),
    );
    match reply {
        Reply::ErrorBeforeText => format!("{error}data: [DONE]\n\n"),
        Reply::Answer => format!("{text}{finish}data: [DONE]\n\n"),
        Reply::ErrorAfterText => format!("{text}{error}data: [DONE]\n\n"),
    }
}

fn serve_one(mut stream: TcpStream, reply: Reply) {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
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
        .find_map(|l| {
            l.strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < header_end + len {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let body = body_for(reply);
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

/// A mock that answers request N with `script[min(N, len-1)]`; returns its URL and request counter.
fn start_mock(script: Vec<Reply>) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let counter = Arc::new(AtomicUsize::new(0));
    let c = Arc::clone(&counter);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let n = c.fetch_add(1, Ordering::SeqCst);
            let reply = script[n.min(script.len() - 1)];
            std::thread::spawn(move || serve_one(stream, reply));
        }
    });
    (url, counter)
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn hone_json_against(base_url: &str) -> Out {
    let tmp = tempfile::tempdir().unwrap();
    let root: PathBuf = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, wd) = (root.join("home"), root.join("wd"));
    for d in [&home, &wd] {
        std::fs::create_dir_all(d).unwrap();
    }
    let token = format!("sk-{}", "fixture0123456789");
    let child = Command::new(env!("CARGO_BIN_EXE_hone"))
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .env("OPENAI_API_KEY", token)
        .env("HONE_OPENAI_BASE_URL", base_url)
        .current_dir(&wd)
        .args(["--no-clarify", "--no-inject", "--json", "say hi"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hone");
    let pid = child.id();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    match rx.recv_timeout(Duration::from_secs(90)) {
        Ok(Ok(o)) => Out {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        _ => {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            panic!("hone timed out");
        }
    }
}

fn json_of(o: &Out) -> serde_json::Value {
    serde_json::from_str(&o.stdout).unwrap_or_else(|e| {
        panic!("--json stdout must be JSON ({e}): {}\nstderr: {}", o.stdout, o.stderr)
    })
}

#[test]
fn test_r11_a_failure_before_any_text_is_retried_and_the_run_succeeds() {
    let (url, requests) = start_mock(vec![Reply::ErrorBeforeText, Reply::Answer]);

    let o = hone_json_against(&url);

    let v = json_of(&o);
    assert_eq!(v["success"], true, "{v}\nstderr: {}", o.stderr);
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(v["response"].as_str().unwrap_or_default().contains("partial answer"), "{v}");
    assert_eq!(requests.load(Ordering::SeqCst), 2, "one failed attempt, one good one");
}

#[test]
fn test_r11_a_provider_that_always_fails_before_output_is_tried_three_times_then_reported() {
    let (url, requests) = start_mock(vec![Reply::ErrorBeforeText]);

    let o = hone_json_against(&url);

    let v = json_of(&o);
    assert_eq!(v["success"], false, "{v}\nstderr: {}", o.stderr);
    assert_ne!(o.code, Some(0));
    assert!(
        v["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Provider disconnected unexpectedly"),
        "{v}"
    );
    assert_eq!(requests.load(Ordering::SeqCst), 3, "default RetryConfig: three attempts");
}

#[test]
fn test_r11_a_failure_after_output_makes_exactly_one_request() {
    let (url, requests) = start_mock(vec![Reply::ErrorAfterText, Reply::Answer]);

    let o = hone_json_against(&url);

    let v = json_of(&o);
    assert_eq!(v["success"], false, "{v}\nstderr: {}", o.stderr);
    assert_eq!(requests.load(Ordering::SeqCst), 1, "a break after text is not retried");
}

#[test]
fn test_r11_control_a_good_response_makes_exactly_one_request() {
    let (url, requests) = start_mock(vec![Reply::Answer]);

    let o = hone_json_against(&url);

    assert_eq!(json_of(&o)["success"], true, "stderr: {}", o.stderr);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}
