#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! U5, U7 and U10 of `docs/plans/usage-and-pricing-spec.md` (review round 1) through the `hone`
//! binary against a mock OpenAI-compatible server that counts its requests:
//!
//! - U5: `--orchestrate` and `--ensemble` with `openai:gpt-4o` (2.50 / 10.00 per million) report
//!   the real cost of every paid call (lead, workers, candidates, ranker), not 0;
//! - U7: with `[budget] session_limit_usd`, an unpriced session / routing / fallback model gets
//!   `hone: [budget] can't limit <model>: it has no price; add [pricing."<model>"] to your
//!   config` on stderr, once, before the first model call (the server here is unreachable, so
//!   nothing can have been called); priced models, user-priced models and runs without a limit
//!   get no such line;
//! - U10: a successful call that reported no usage marks the `--json` run `usage_known: false`
//!   (and `cost_known: false`); one that did keeps `usage_known: true`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const PLAN: &str = r#"{\"goal\":\"test\",\"subtasks\":[{\"id\":\"1\",\"description\":\"do thing\",\"agent\":\"debugger\",\"context\":\"\",\"depends_on\":[]}]}"#;

struct Mock {
    base_url: String,
    requests: Arc<AtomicUsize>,
}

/// Answers every request with `text`, and (when `usage`) a trailing usage chunk of 10 / 5.
fn start_mock(text: &'static str, usage: bool) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let count = Arc::clone(&requests);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let count = Arc::clone(&count);
            std::thread::spawn(move || serve(stream, text, usage, &count));
        }
    });
    Mock { base_url, requests }
}

fn serve(mut stream: TcpStream, text: &str, usage: bool, count: &AtomicUsize) {
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
    count.fetch_add(1, Ordering::SeqCst);
    let c1 = format!(r#"{{"choices":[{{"delta":{{"content":"{text}"}},"finish_reason":null}}]}}"#);
    let c2 = r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
    let mut payload = format!("data: {c1}\n\ndata: {c2}\n\n");
    if usage {
        payload.push_str("data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":5}}\n\n");
    }
    payload.push_str("data: [DONE]\n\n");
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

/// `top` is top-level TOML (before the tables), `tables` the tables after the provider table.
fn env(model: &str, base_url: &str, top: &str, tables: &str) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, wd) = (root.join("home"), root.join("wd"));
    std::fs::create_dir_all(home.join(".config/hone")).unwrap();
    std::fs::create_dir_all(wd.join(".hone")).unwrap();
    std::fs::write(
        home.join(".config/hone/config.toml"),
        format!(
            "default_model = \"{model}\"\n{top}\n[providers.openai]\nbase_url = \"{base_url}\"\napi_key_env = \"HONE_TEST_PROVIDER_VAR\"\n\n{tables}"
        ),
    )
    .unwrap();
    Env { _tmp: tmp, home, wd }
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run(e: &Env, args: &[&str]) -> Out {
    let child = Command::new(env!("CARGO_BIN_EXE_hone"))
        .env_clear()
        .env("HOME", &e.home)
        .env("PATH", "/usr/bin:/bin")
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .env("HONE_TEST_PROVIDER_VAR", "placeholder-value")
        .current_dir(&e.wd)
        .args(args)
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
    match rx.recv_timeout(Duration::from_secs(120)) {
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

const PER_CALL: f64 = (10.0 * 2.5 + 5.0 * 10.0) / 1e6;

// ---- U5 -----------------------------------------------------------------------------------------------------

#[test]
fn test_u5_orchestrate_should_report_the_cost_of_every_paid_call() {
    let mock = start_mock(PLAN, true);
    let e = env("openai:gpt-4o", &mock.base_url, "", "");

    let o = run(&e, &["--no-clarify", "--no-inject", "--orchestrate", "do thing"]);

    let calls = mock.requests.load(Ordering::SeqCst);
    assert!(calls >= 3, "decompose, worker and merge are paid calls ({calls}): {}", o.stderr);
    let total: f64 = o
        .stdout
        .lines()
        .find_map(|l| l.strip_prefix("Total cost: $"))
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("a 'Total cost' line:\n{}\n{}", o.stdout, o.stderr))
        .parse()
        .unwrap();
    assert!(
        (total - calls as f64 * PER_CALL).abs() < 1e-6,
        "{calls} calls at {PER_CALL} each should cost {}; reported {total}\n{}",
        calls as f64 * PER_CALL,
        o.stdout
    );
}

#[test]
fn test_u5_ensemble_should_report_the_cost_of_the_candidates_and_the_ranker() {
    let mock = start_mock("alpha", true);
    let e = env("openai:gpt-4o", &mock.base_url, "", "");

    let o = run(&e, &["--no-clarify", "--no-inject", "--ensemble", "2", "question"]);

    assert_eq!(mock.requests.load(Ordering::SeqCst), 3, "two candidates and the ranker: {}", o.stderr);
    assert!(
        o.stdout.contains("$0.0002 total"),
        "3 calls x $7.5e-5 = $0.000225: {}",
        o.stdout
    );
}

// ---- U7 -----------------------------------------------------------------------------------------------------

fn budget_warning(model: &str) -> String {
    format!("hone: [budget] can't limit {model}: it has no price; add [pricing.\"{model}\"] to your config")
}

const UNREACHABLE: &str = "http://127.0.0.1:9";
const LIMIT: &str = "[budget]\nsession_limit_usd = 5.0\n";

#[test]
fn test_u7_a_session_limit_should_warn_once_about_an_unpriced_session_model() {
    let e = env("openai:dummy-model", UNREACHABLE, "", LIMIT);

    let o = run(&e, &["--no-clarify", "--no-inject", "--json", "say hi"]);

    let w = budget_warning("openai:dummy-model");
    assert_eq!(o.stderr.matches(&w).count(), 1, "once: {}", o.stderr);
}

#[test]
fn test_u7_routing_and_fallback_models_should_be_checked_too() {
    let e = env(
        "openai:dummy-model",
        UNREACHABLE,
        "fallback_models = [\"openai:fallback-y\"]\n",
        &format!("{LIMIT}\n[[routing]]\ntask = \"simple\"\nmodel = \"openai:routed-x\"\n"),
    );

    let o = run(&e, &["--no-clarify", "--no-inject", "--json", "say hi"]);

    for m in ["openai:dummy-model", "openai:routed-x", "openai:fallback-y"] {
        assert_eq!(o.stderr.matches(&budget_warning(m)).count(), 1, "{m}: {}", o.stderr);
    }
}

#[test]
fn test_u7_priced_models_should_not_be_warned_about() {
    let e = env(
        "openai:gpt-4o",
        UNREACHABLE,
        "",
        &format!(
            "{LIMIT}\n[pricing.\"openai:user-priced\"]\ninput_per_million = 1.0\noutput_per_million = 2.0\n"
        ),
    );

    let o = run(&e, &["--no-clarify", "--no-inject", "--json", "say hi"]);

    assert!(!o.stderr.contains("[budget] can't limit"), "{}", o.stderr);
    let e = env(
        "openai:user-priced",
        UNREACHABLE,
        "",
        &format!(
            "{LIMIT}\n[pricing.\"openai:user-priced\"]\ninput_per_million = 1.0\noutput_per_million = 2.0\n"
        ),
    );
    let o = run(&e, &["--no-clarify", "--no-inject", "--json", "say hi"]);
    assert!(!o.stderr.contains("[budget] can't limit"), "{}", o.stderr);
}

#[test]
fn test_u7_without_a_session_limit_there_should_be_no_budget_warning() {
    let e = env("openai:dummy-model", UNREACHABLE, "", "");

    let o = run(&e, &["--no-clarify", "--no-inject", "--json", "say hi"]);

    assert!(!o.stderr.contains("[budget] can't limit"), "{}", o.stderr);
}

// ---- U10 ----------------------------------------------------------------------------------------------------

#[test]
fn test_u10_a_call_that_reported_no_usage_should_mark_the_run_usage_unknown() {
    let mock = start_mock("hello", false);
    let e = env("openai:gpt-4o", &mock.base_url, "", "");

    let o = run(&e, &["--no-clarify", "--no-inject", "--json", "say hi"]);

    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("JSON");
    assert_eq!(v["usage_known"], false, "{v}");
    assert_eq!(v["cost_known"], false, "no usage means the cost isn't known either: {v}");
}

#[test]
fn test_u10_a_call_that_reported_usage_should_keep_the_run_usage_known() {
    let mock = start_mock("hello", true);
    let e = env("openai:gpt-4o", &mock.base_url, "", "");

    let o = run(&e, &["--no-clarify", "--no-inject", "--json", "say hi"]);

    assert_eq!(o.code, Some(0), "{}", o.stderr);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("JSON");
    assert_eq!(v["usage_known"], true, "{v}");
    assert_eq!(v["cost_known"], true, "{v}");
}
