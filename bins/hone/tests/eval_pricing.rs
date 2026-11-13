#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! `hone eval` prices its runs per model, as a one-shot run does (usage-and-pricing spec U1-U3).
//! The real `hone` binary, a mock OpenAI-compatible server that reports 10 prompt and
//! 5 completion tokens per call, a temp HOME and a cleared environment.
//!
//! Contract pinned:
//! - a user `[pricing."openai:<model>"]` entry prices the run: `outcome.json`'s `cost_usd`
//!   is tokens x that entry's rates, per model call;
//! - a model in the default price table (`openai:gpt-4o`, 2.50 / 10.00 per million) is
//!   priced by it;
//! - `report.json`'s `mean_cost_usd` follows.
//!
//! Not pinned: how an unpriced model shows up. Eval outcomes have no `cost_known` field,
//! so only the priced cases are specified.

mod eval_support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use eval_support::task_toml;
use serde_json::Value;

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
    let c1 = r#"{"choices":[{"delta":{"content":"all done"},"finish_reason":null}]}"#;
    let c2 = r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
    let c3 = r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":5}}"#;
    let payload = format!("data: {c1}\n\ndata: {c2}\n\ndata: {c3}\n\ndata: [DONE]\n\n");
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(resp.as_bytes());
}

fn mock_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for s in listener.incoming().flatten() {
            std::thread::spawn(move || serve(s));
        }
    });
    url
}

/// Runs a one-task suite on `model`; returns (outcome.json, report.json).
fn eval(model: &str, user_extra: &str) -> (Value, Value) {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, suite) = (root.join("home"), root.join("suite"));
    std::fs::create_dir_all(home.join(".config/hone")).unwrap();
    std::fs::create_dir_all(suite.join("proj")).unwrap();
    std::fs::write(
        home.join(".config/hone/config.toml"),
        format!(
            "[providers.openai]\nbase_url = \"{}\"\napi_key_env = \"HONE_TEST_PROVIDER_VAR\"\n\n{user_extra}",
            mock_url()
        ),
    )
    .unwrap();
    std::fs::write(suite.join("tasks.toml"), format!("model = \"{model}\"\n\n{}", task_toml("t", "proj", "true"))).unwrap();

    let child = Command::new(env!("CARGO_BIN_EXE_hone"))
        .arg("eval")
        .arg(suite.join("tasks.toml"))
        .args(["-o", "out"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .env("HONE_TEST_PROVIDER_VAR", "placeholder-value")
        .current_dir(&suite)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hone");
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });
    let out = rx.recv_timeout(Duration::from_secs(120)).expect("hone eval timed out").unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let run: PathBuf = std::fs::read_dir(suite.join("out"))
        .unwrap_or_else(|e| panic!("no out dir: {e}\n{stderr}"))
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("report.json").exists())
        .unwrap_or_else(|| panic!("no run directory with a report.json\n{stderr}"));
    let outcome = find_outcome(&run).unwrap_or_else(|| panic!("no outcome.json under {run:?}\n{stderr}"));
    let report: Value = serde_json::from_slice(&std::fs::read(run.join("report.json")).unwrap()).unwrap();
    (outcome, report)
}

fn find_outcome(dir: &Path) -> Option<Value> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(v) = find_outcome(&p) {
                return Some(v);
            }
        } else if p.file_name().is_some_and(|n| n == "outcome.json") {
            return serde_json::from_slice(&std::fs::read(&p).ok()?).ok();
        }
    }
    None
}

#[test]
fn test_an_eval_run_should_be_priced_by_the_users_pricing_entry() {
    let (outcome, report) = eval(
        "openai:dummy-model",
        "[pricing.\"openai:dummy-model\"]\ninput_per_million = 2.0\noutput_per_million = 4.0\n",
    );

    assert_eq!(outcome["tokens_input"], 10 * outcome["model_calls"].as_u64().unwrap(), "harness: usage counted: {outcome}");
    let calls = outcome["model_calls"].as_f64().unwrap();
    let expected = calls * (10.0 * 2.0 + 5.0 * 4.0) / 1e6;
    let cost = outcome["cost_usd"].as_f64().unwrap();
    assert!(expected > 0.0 && (cost - expected).abs() < 1e-12, "cost_usd {cost} vs {expected}: {outcome}");
    let mean = report["profiles"][0]["mean_cost_usd"].as_f64().unwrap();
    assert!((mean - expected).abs() < 1e-12, "report mean_cost_usd {mean} vs {expected}");
}

#[test]
fn test_an_eval_run_on_a_model_in_the_default_table_should_be_priced_by_it() {
    let (outcome, _) = eval("openai:gpt-4o", "");

    let calls = outcome["model_calls"].as_f64().unwrap();
    let expected = calls * (10.0 * 2.5 + 5.0 * 10.0) / 1e6;
    let cost = outcome["cost_usd"].as_f64().unwrap();
    assert!((cost - expected).abs() < 1e-12, "cost_usd {cost} vs {expected}: {outcome}");
}
