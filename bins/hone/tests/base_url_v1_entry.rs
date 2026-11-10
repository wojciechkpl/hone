#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! B4 (`docs/plans/base-url-v1-spec.md`): end to end through the `hone` binary, a base URL that
//! ends in `/v1` (the OpenRouter wizard's `https://openrouter.ai/api/v1`, or the usual vLLM
//! `HONE_OPENAI_BASE_URL=http://localhost:8000/v1`) must reach the server at the right path,
//! never `/v1/v1/chat/completions`.
//!
//! Harness: temp HOME and working directory, cleared environment,
//! `HONE_DISABLE_PERSISTENT_STORES=1`, and a std `TcpListener` mock that records each request.
//! Credential-shaped fixtures are built at runtime. The child has a timeout.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    authorization: String,
    body: String,
}

impl Recorded {
    fn model(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|v| v["model"].as_str().map(str::to_owned))
            .unwrap_or_default()
    }
}

struct Mock {
    base_url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

impl Mock {
    fn start() -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log));
            }
        });
        Mock { base_url, requests }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<Recorded>>) {
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
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let content_length = head
        .lines()
        .find_map(|l| {
            let l = l.to_ascii_lowercase();
            l.strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let body = String::from_utf8_lossy(&buf[header_end..]).into_owned();
    let path = head
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_owned();
    let authorization = head
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
        .map(|l| l.split_once(':').unwrap().1.trim().to_owned())
        .unwrap_or_default();
    log.lock().unwrap().push(Recorded {
        path,
        authorization,
        body,
    });

    let c1 = r#"{"choices":[{"delta":{"content":"mock reply"},"finish_reason":null}]}"#;
    let c2 = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":1}}"#;
    let payload = format!("data: {c1}\n\ndata: {c2}\n\ndata: [DONE]\n\n");
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    wd: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, wd) = (root.join("home"), root.join("wd"));
    for d in [&home, &wd] {
        std::fs::create_dir_all(d).unwrap();
    }
    Env { _tmp: tmp, home, wd }
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn hone_json(e: &Env, extra_env: &[(&str, &str)]) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hone"));
    cmd.env_clear()
        .env("HOME", &e.home)
        .env("PATH", "/usr/bin:/bin")
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .current_dir(&e.wd)
        .args(["--no-clarify", "--no-inject", "--json", "say hi"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let child = cmd.spawn().expect("spawn hone");
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

fn token_value(prefix: &str) -> String {
    format!("{prefix}-{}", "fixture0123456789")
}

#[test]
fn test_b4_wizard_openrouter_file_with_a_v1_base_url_reaches_the_right_path() {
    let e = env();
    let mock = Mock::start();
    let k = token_value("or");
    let cfg = e.home.join(".config/hone/config.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    // The shape `hone configure` writes for OpenRouter (base URL ends in /api/v1).
    std::fs::write(
        &cfg,
        format!(
            "[provider]\nname = \"openrouter\"\napi_key = {k:?}\nmodel = \"vendor/or-model\"\nbase_url = \"{}/api/v1\"\n",
            mock.base_url
        ),
    )
    .unwrap();

    let o = hone_json(&e, &[]);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1, "stderr: {}", o.stderr);
    assert_eq!(reqs[0].path, "/api/v1/chat/completions");
    assert_eq!(reqs[0].model(), "vendor/or-model");
    assert_eq!(reqs[0].authorization, format!("Bearer {k}"));
    serde_json::from_str::<serde_json::Value>(&o.stdout).expect("--json stdout is JSON");
}

#[test]
fn test_b4_env_base_url_ending_in_v1_reaches_the_right_path() {
    for (suffix, expected) in [
        ("/v1", "/v1/chat/completions"),
        ("/v1/", "/v1/chat/completions"),
        ("", "/v1/chat/completions"),
    ] {
        let e = env();
        let mock = Mock::start();
        let base = format!("{}{suffix}", mock.base_url);

        let o = hone_json(
            &e,
            &[
                ("OPENAI_API_KEY", &token_value("sk")),
                ("HONE_OPENAI_BASE_URL", &base),
            ],
        );

        assert_eq!(o.code, Some(0), "suffix {suffix:?}: stderr: {}", o.stderr);
        let reqs = mock.requests();
        assert_eq!(reqs.len(), 1, "suffix {suffix:?}: stderr: {}", o.stderr);
        assert_eq!(reqs[0].path, expected, "suffix {suffix:?}");
    }
}
