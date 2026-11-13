#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U1-U3 of `docs/plans/usage-and-pricing-spec.md` end to end through the `hone`
//! binary: a mock OpenAI-compatible server that sends the usage the OpenAI way (a
//! separate last chunk with `choices: []`, after the `finish_reason` chunk), a temp
//! HOME and working directory, a cleared environment. `--json` is the observable.
//!
//! Contract pinned:
//! - the trailing usage is counted: `usage.input == 10`, `usage.output == 5`;
//! - cost comes from the model's entry: the default `PricingTable` for `openai:gpt-4o`
//!   (2.50 / 10.00 per million), or the user's `[pricing."<provider:model>"]` table
//!   (`input_per_million`, `output_per_million`, optional cache rates);
//! - an unpriced model costs 0, `--json` has `"cost_known": false`, and stderr has ONE line
//!   `hone: no price for <model>; its usage isn't counted towards cost or [budget]`;
//! - priced runs have `"cost_known": true` and no such line;
//! - a project file's `[pricing]` is ignored with a warning naming the file.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

fn serve(mut stream: TcpStream) {
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
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buf.len() < header_end + content_length {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let c1 = r#"{"choices":[{"delta":{"content":"mock reply"},"finish_reason":null}]}"#;
    let c2 = r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
    // The OpenAI way: usage in its own chunk, after the finish chunk, with no choices.
    let c3 = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
    let payload = format!("data: {c1}\n\ndata: {c2}\n\ndata: {c3}\n\ndata: [DONE]\n\n");
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

fn start_mock() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            std::thread::spawn(move || serve(stream));
        }
    });
    base_url
}

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    wd: PathBuf,
}

/// `user_extra` and `project` are appended to / written as the config files.
fn env(model: &str, user_extra: &str, project: Option<&str>) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, wd) = (root.join("home"), root.join("wd"));
    std::fs::create_dir_all(home.join(".config/hone")).unwrap();
    std::fs::create_dir_all(wd.join(".hone")).unwrap();
    let base = start_mock();
    std::fs::write(
        home.join(".config/hone/config.toml"),
        format!(
            "default_model = \"{model}\"\n[providers.openai]\nbase_url = \"{base}\"\napi_key_env = \"HONE_TEST_PROVIDER_VAR\"\n\n{user_extra}"
        ),
    )
    .unwrap();
    if let Some(p) = project {
        std::fs::write(wd.join(".hone/config.toml"), p).unwrap();
    }
    Env { _tmp: tmp, home, wd }
}

struct Out {
    code: Option<i32>,
    json: serde_json::Value,
    stderr: String,
}

fn run(e: &Env) -> Out {
    let child = Command::new(env!("CARGO_BIN_EXE_hone"))
        .env_clear()
        .env("HOME", &e.home)
        .env("PATH", "/usr/bin:/bin")
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .env("HONE_TEST_PROVIDER_VAR", "placeholder-value")
        .current_dir(&e.wd)
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
            json: serde_json::from_slice(&o.stdout).unwrap_or(serde_json::Value::Null),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        },
        _ => {
            let _ = Command::new("kill").arg("-9").arg(pid.to_string()).status();
            panic!("hone timed out");
        }
    }
}

fn cost(o: &Out) -> f64 {
    o.json["cost_usd"].as_f64().unwrap()
}

#[test]
fn test_u1_the_trailing_usage_chunk_should_be_counted_in_the_report() {
    let e = env("openai:dummy-model", "", None);

    let o = run(&e);

    assert_eq!(o.code, Some(0), "{}", o.stderr);
    assert_eq!(o.json["usage"]["input"], 10, "{}", o.json);
    assert_eq!(o.json["usage"]["output"], 5, "{}", o.json);
}

#[test]
fn test_u2_a_model_in_the_default_table_should_be_priced_by_its_entry() {
    let e = env("openai:gpt-4o", "", None);

    let o = run(&e);

    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let expected = (10.0 * 2.5 + 5.0 * 10.0) / 1e6;
    assert!((cost(&o) - expected).abs() < 1e-12, "{} vs {expected}", cost(&o));
    assert_eq!(o.json["cost_known"], true, "{}", o.json);
    assert!(!o.stderr.contains("no price for"), "{}", o.stderr);
}

#[test]
fn test_u2_a_user_pricing_table_should_price_the_model() {
    let e = env(
        "openai:dummy-model",
        "[pricing.\"openai:dummy-model\"]\ninput_per_million = 2.0\noutput_per_million = 4.0\n",
        None,
    );

    let o = run(&e);

    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let expected = (10.0 * 2.0 + 5.0 * 4.0) / 1e6;
    assert!((cost(&o) - expected).abs() < 1e-12, "{} vs {expected}", cost(&o));
    assert_eq!(o.json["cost_known"], true);
    assert!(!o.stderr.contains("no price for"), "{}", o.stderr);
}

#[test]
fn test_u2_a_user_entry_should_override_the_default_table() {
    let e = env(
        "openai:gpt-4o",
        "[pricing.\"openai:gpt-4o\"]\ninput_per_million = 1.0\noutput_per_million = 1.0\n",
        None,
    );

    let o = run(&e);

    let expected = (10.0 + 5.0) / 1e6;
    assert!((cost(&o) - expected).abs() < 1e-12, "{} vs {expected}", cost(&o));
}

#[test]
fn test_u2_a_project_pricing_table_should_be_ignored_with_a_warning() {
    let project = "[pricing.\"openai:dummy-model\"]\ninput_per_million = 2.0\noutput_per_million = 4.0\n";
    let e = env("openai:dummy-model", "", Some(project));

    let o = run(&e);

    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let project_path = e.wd.join(".hone/config.toml").display().to_string();
    assert!(
        o.stderr.contains(&project_path) && o.stderr.contains("pricing"),
        "a warning naming the project file: {}",
        o.stderr
    );
    assert_eq!(cost(&o), 0.0, "the project's price was not used");
    assert_eq!(o.json["cost_known"], false);
}

#[test]
fn test_u3_an_unpriced_model_should_cost_nothing_and_say_so_once() {
    let e = env("openai:dummy-model", "", None);

    let o = run(&e);

    assert_eq!(o.code, Some(0), "{}", o.stderr);
    assert_eq!(cost(&o), 0.0, "{}", o.json);
    assert_eq!(o.json["cost_known"], false, "{}", o.json);
    assert_eq!(o.json["usage"]["input"], 10, "usage is still counted: {}", o.json);
    let note = "hone: no price for openai:dummy-model; its usage isn't counted towards cost or [budget]";
    assert_eq!(o.stderr.matches(note).count(), 1, "exactly one note: {}", o.stderr);
}
