#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! S7 (`docs/plans/stream-transport-error-spec.md`): through the `hone` binary, an
//! OpenAI-compatible response that is cut off (or ends without its completion marker, or carries
//! an in-band error) is reported as a provider error -- non-zero exit and an `error` in the
//! `--json` output -- not as a successful answer.
//!
//! Harness: temp HOME and working directory, cleared environment,
//! `HONE_DISABLE_PERSISTENT_STORES=1`, and a std `TcpListener` mock that answers with a chunked
//! SSE body and then ends it the way the test asks. Credential-shaped fixtures are built at
//! runtime. The child has a timeout.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

#[derive(Clone, Copy, Debug)]
enum Ending {
    /// Close the socket without the terminating chunk.
    CutOff,
    /// Terminating chunk, but no `finish_reason` / `[DONE]`.
    CleanNoMarker,
    /// A complete response: finish_reason + [DONE].
    Complete,
    /// An in-band error event after some text.
    InBandError,
}

fn sse(v: serde_json::Value) -> String {
    format!("data: {v}\n\n")
}

fn body_for(ending: Ending) -> String {
    let mut out = sse(
        serde_json::json!({"choices":[{"index":0,"delta":{"content":"partial answer"},"finish_reason":null}]}),
    );
    match ending {
        Ending::CutOff | Ending::CleanNoMarker => {}
        Ending::Complete => {
            out += &sse(
                serde_json::json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}),
            );
            out += "data: [DONE]\n\n";
        }
        Ending::InBandError => {
            out += &sse(
                serde_json::json!({"error":{"message":"Provider disconnected unexpectedly","code":502}}),
            );
            out += "data: [DONE]\n\n";
        }
    }
    out
}

fn serve_one(mut stream: TcpStream, ending: Ending) {
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    stream.set_nodelay(true).ok();
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

    let body = body_for(ending);
    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(format!("{:x}\r\n{body}\r\n", body.len()).as_bytes());
    let _ = stream.flush();
    std::thread::sleep(Duration::from_millis(50));
    if !matches!(ending, Ending::CutOff) {
        let _ = stream.write_all(b"0\r\n\r\n");
        let _ = stream.flush();
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}

fn start_mock(ending: Ending) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            std::thread::spawn(move || serve_one(stream, ending));
        }
    });
    url
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
    let api_key = format!("sk-{}", "fixture0123456789");
    let child = Command::new(env!("CARGO_BIN_EXE_hone"))
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .env("OPENAI_API_KEY", api_key)
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
        panic!(
            "--json stdout must be JSON ({e}): {}\nstderr: {}",
            o.stdout, o.stderr
        )
    })
}

#[test]
fn test_s7_a_cut_off_response_is_a_provider_error_not_a_successful_answer() {
    let o = hone_json_against(&start_mock(Ending::CutOff));

    let v = json_of(&o);
    assert_eq!(v["success"], false, "{v}\nstderr: {}", o.stderr);
    assert!(v["error"].is_string(), "an error is reported: {v}");
    assert_ne!(o.code, Some(0), "non-zero exit: stderr: {}", o.stderr);
}

#[test]
fn test_s7_a_clean_end_without_the_completion_marker_is_a_provider_error() {
    let o = hone_json_against(&start_mock(Ending::CleanNoMarker));

    let v = json_of(&o);
    assert_eq!(v["success"], false, "{v}\nstderr: {}", o.stderr);
    assert!(v["error"].is_string(), "{v}");
    assert_ne!(o.code, Some(0));
}

#[test]
fn test_s7_an_in_band_error_event_is_reported_with_the_providers_message() {
    let o = hone_json_against(&start_mock(Ending::InBandError));

    let v = json_of(&o);
    assert_eq!(v["success"], false, "{v}\nstderr: {}", o.stderr);
    assert!(
        v["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Provider disconnected unexpectedly"),
        "{v}"
    );
    assert_ne!(o.code, Some(0));
}

#[test]
fn test_s7_control_a_complete_response_still_succeeds() {
    let o = hone_json_against(&start_mock(Ending::Complete));

    let v = json_of(&o);
    assert_eq!(v["success"], true, "{v}\nstderr: {}", o.stderr);
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(
        v["response"]
            .as_str()
            .unwrap_or_default()
            .contains("partial answer"),
        "{v}"
    );
}
