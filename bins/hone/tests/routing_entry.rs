#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! G6, G7, G9 and G10 (`docs/plans/routing-fallback-spec.md`) through the `hone` binary.
//!
//! Two raw HTTP mocks: an OpenAI-compatible one (reached through `HONE_OPENAI_BASE_URL`) and an
//! Ollama one (NDJSON on `/api/chat`, reached through `[providers.ollama].base_url` in the user
//! config). Each counts and records its requests and scripts one reply per request (the last
//! entry repeats). HOME and the working directory are temp dirs; the user config is
//! `$HOME/.config/hone/config.toml`, the project config `<wd>/.hone/config.toml`.
//!
//! Prompts come from `crates/hone-core/src/complexity.rs`: "fix typo in README" is Simple,
//! "implement a new caching layer across modules" is Complex.
//! Credential-shaped fixtures are built at runtime. Every child has a timeout.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

const SIMPLE: &str = "fix typo in README";
const COMPLEX: &str = "implement a new caching layer across modules";

// -- mocks -----------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Kind {
    OpenAi,
    Ollama,
}

#[derive(Clone, Debug)]
enum Reply {
    Answer(&'static str),
    /// A failure before any output (retryable).
    FailBeforeOutput,
    /// (Ollama only) a tool call for a file that does not exist: the tool fails, the turn
    /// goes on with another model call.
    ToolCall,
}

#[derive(Clone, Debug)]
struct Recorded {
    path: String,
    body: Value,
}

struct Mock {
    url: String,
    seen: Arc<Mutex<Vec<Recorded>>>,
}

impl Mock {
    fn start(kind: Kind, script: Vec<Reply>) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let counter = Arc::new(AtomicUsize::new(0));
        let (s, c) = (Arc::clone(&seen), counter);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let n = c.fetch_add(1, Ordering::SeqCst);
                let reply = script[n.min(script.len() - 1)].clone();
                let s = Arc::clone(&s);
                std::thread::spawn(move || serve_one(stream, kind, reply, &s));
            }
        });
        Mock { url, seen }
    }

    fn count(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    fn models(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.body["model"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    fn bodies(&self) -> Vec<Value> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.body.clone())
            .collect()
    }
}

fn sse(v: Value) -> String {
    format!("data: {v}\n\n")
}

fn body_for(kind: Kind, reply: &Reply) -> String {
    match (kind, reply) {
        (Kind::OpenAi, Reply::Answer(t)) => {
            let text =
                sse(json!({"choices":[{"index":0,"delta":{"content":t},"finish_reason":null}]}));
            let fin = sse(
                json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}),
            );
            format!("{text}{fin}data: [DONE]\n\n")
        }
        (Kind::OpenAi, Reply::FailBeforeOutput) => {
            let e = sse(json!({"error":{"message":"openai mock overloaded","code":502}}));
            format!("{e}data: [DONE]\n\n")
        }
        (Kind::Ollama, Reply::Answer(t)) => format!(
            "{}\n{}\n",
            json!({"model":"m","message":{"role":"assistant","content":t},"done":false}),
            json!({"model":"m","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":3,"eval_count":2}),
        ),
        (Kind::OpenAi, Reply::ToolCall) => panic!("tool calls are scripted on Ollama only"),
        (Kind::Ollama, Reply::ToolCall) => format!(
            "{}\n{}\n",
            json!({"model":"m","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"read_file","arguments":{"path":"does-not-exist.txt"}}}]},"done":false}),
            json!({"model":"m","message":{"role":"assistant","content":""},"done":true,"prompt_eval_count":3,"eval_count":2}),
        ),
        (Kind::Ollama, Reply::FailBeforeOutput) => {
            format!("{}\n", json!({"error": "ollama mock exploded"}))
        }
    }
}

fn serve_one(mut stream: TcpStream, kind: Kind, reply: Reply, seen: &Mutex<Vec<Recorded>>) {
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
    let lower = head.to_ascii_lowercase();
    let len = lower
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
    let path = head
        .lines()
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .to_owned();
    let body: Value = serde_json::from_slice(&buf[header_end..]).unwrap_or(Value::Null);
    seen.lock().unwrap().push(Recorded { path, body });

    let payload = body_for(kind, &reply);
    let ct = match kind {
        Kind::OpenAi => "text/event-stream",
        Kind::Ollama => "application/x-ndjson",
    };
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

// -- harness -----------------------------------------------------------------------------------------

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    bin_dir: PathBuf,
    home: PathBuf,
    wd: PathBuf,
}

impl Env {
    fn new() -> Env {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let (bin_dir, home, wd) = (root.join("bin"), root.join("home"), root.join("wd"));
        for d in [&bin_dir, &home, &wd] {
            std::fs::create_dir_all(d).unwrap();
        }
        Env {
            _tmp: tmp,
            root,
            bin_dir,
            home,
            wd,
        }
    }
    fn user_config(&self) -> PathBuf {
        self.home.join(".config/hone/config.toml")
    }
    fn project_config(&self) -> PathBuf {
        self.wd.join(".hone/config.toml")
    }
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Out {
    fn json(&self) -> Value {
        serde_json::from_str(&self.stdout).unwrap_or_else(|e| {
            panic!(
                "--json stdout must be JSON ({e}): {}\nstderr: {}",
                self.stdout, self.stderr
            )
        })
    }
}

/// Run `hone --json` with the OpenAI-compatible mock wired through the environment.
fn hone_json(e: &Env, openai: Option<&Mock>, model: Option<&str>, prompt: &str) -> Out {
    run_hone(e, openai, model, prompt, true)
}

/// Run `hone` (one-shot, without `--json`) with the same wiring.
fn hone_plain(e: &Env, openai: Option<&Mock>, model: Option<&str>, prompt: &str) -> Out {
    run_hone(e, openai, model, prompt, false)
}

fn run_hone(e: &Env, openai: Option<&Mock>, model: Option<&str>, prompt: &str, json: bool) -> Out {
    let token = format!("sk-{}", "fixture0123456789");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hone"));
    cmd.env_clear()
        .env("HOME", &e.home)
        .env("PATH", format!("{}:/usr/bin:/bin", e.bin_dir.display()))
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .env("OPENAI_API_KEY", token)
        .current_dir(&e.wd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(m) = openai {
        cmd.env("HONE_OPENAI_BASE_URL", &m.url);
    }
    cmd.args(["--no-clarify", "--no-inject"]);
    if json {
        cmd.arg("--json");
    }
    if let Some(m) = model {
        cmd.args(["--model", m]);
    }
    cmd.arg(prompt);
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

/// User config: `default_model = "openai:main-model"` and the Ollama endpoint, plus `extra`.
fn user_config(ollama: &Mock, extra: &str) -> String {
    format!(
        "default_model = \"openai:main-model\"\n{extra}\n[providers.ollama]\nbase_url = {:?}\n",
        ollama.url
    )
}

fn answer(t: &'static str) -> Vec<Reply> {
    vec![Reply::Answer(t)]
}

fn write_fake_claude(e: &Env) -> PathBuf {
    let marker = e.bin_dir.join("claude-was-run");
    let bin = e.bin_dir.join("claude");
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\ncase \"$1\" in --version|-v) echo 1.0.0; exit 0;; esac\ntouch '{}'\necho '{{\"type\":\"result\",\"result\":\"fake\"}}'\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
    marker
}

// -- G9: nothing changes without the new config ---------------------------------------------------------

#[test]
fn test_g9_without_routing_or_fallback_the_request_and_json_output_are_the_established_ones() {
    let e = Env::new();
    let openai = Mock::start(Kind::OpenAi, answer("mock reply"));

    let o = hone_json(&e, Some(&openai), None, "say hi");

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(openai.count(), 1);
    // Pinned golden values of the request.
    let body = &openai.bodies()[0];
    assert_eq!(body["model"], "gpt-4o");
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    let mut keys: Vec<&str> = body
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        ["messages", "model", "stream", "stream_options", "tools"]
    );
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], "system");
    assert_eq!(
        messages.last().unwrap(),
        &json!({"role": "user", "content": "say hi"})
    );
    assert_eq!(openai.seen.lock().unwrap()[0].path, "/v1/chat/completions");
    // Pinned golden values of the --json output.
    let v = o.json();
    let mut out_keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    out_keys.sort_unstable();
    assert_eq!(
        out_keys,
        [
            "context",
            "cost_usd",
            "error",
            "fallbacks",
            "model",
            "response",
            "success",
            "tool_calls",
            "usage"
        ]
    );
    assert_eq!(v["model"], "openai:gpt-4o");
    assert_eq!(v["fallbacks"], json!([]), "N8: always present, empty");
    assert!(v["context"].is_array(), "CR9: context is always an array");
    assert_eq!(v["response"], "mock reply");
    assert_eq!(v["success"], true);
    assert_eq!(v["usage"]["input"], 3);
    assert_eq!(v["usage"]["output"], 2);
    assert!(
        !o.stderr.contains("using"),
        "no routing notice: {}",
        o.stderr
    );
}

#[test]
fn test_g9_empty_new_keys_change_nothing() {
    let plain_env = Env::new();
    let plain_mock = Mock::start(Kind::OpenAi, answer("mock reply"));
    let plain = hone_json(&plain_env, Some(&plain_mock), None, SIMPLE);

    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("mock reply"));
    write(
        &e.user_config(),
        &format!(
            "fallback_models = []\n[providers.ollama]\nbase_url = {:?}\n",
            ollama.url
        ),
    );

    let o = hone_json(&e, Some(&openai), None, SIMPLE);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(
        openai.bodies(),
        plain_mock.bodies(),
        "byte-for-byte the same request"
    );
    assert_eq!(o.json(), plain.json());
    assert_eq!(ollama.count(), 0);
}

// -- G10 (a): routing by tier -----------------------------------------------------------------------------

#[test]
fn test_g10a_a_simple_prompt_is_routed_to_ollama_and_a_complex_one_stays_on_the_default() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    write(
        &e.user_config(),
        &user_config(
            &ollama,
            "[[routing]]\ntask = \"simple\"\nmodel = \"ollama:qwen-test\"\n",
        ),
    );

    let o = hone_json(&e, Some(&openai), None, SIMPLE);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(
        (ollama.count(), openai.count()),
        (1, 0),
        "only the Ollama mock: {}",
        o.stderr
    );
    assert_eq!(ollama.models(), vec!["qwen-test".to_owned()]);
    let v = o.json();
    assert_eq!(v["model"], "ollama:qwen-test");
    assert_eq!(v["response"], "ollama says hi");
    assert!(
        o.stderr
            .contains("hone: using ollama:qwen-test for this simple request (from [[routing]] in "),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr.contains(".config/hone/config.toml)"),
        "{}",
        o.stderr
    );

    let o = hone_json(&e, Some(&openai), None, COMPLEX);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(
        (ollama.count(), openai.count()),
        (1, 1),
        "the complex prompt reaches only OpenAI"
    );
    assert_eq!(openai.models(), vec!["main-model".to_owned()]);
    assert_eq!(o.json()["model"], "openai:main-model");
    assert!(
        !o.stderr.contains("hone: using"),
        "no notice without routing: {}",
        o.stderr
    );
}

// -- G10 (b)-(d): fallback models ----------------------------------------------------------------------------

#[test]
fn test_g10b_a_provider_failing_before_output_falls_back_to_the_listed_model() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama rescued it"));
    let openai = Mock::start(Kind::OpenAi, vec![Reply::FailBeforeOutput]);
    write(
        &e.user_config(),
        &user_config(&ollama, "fallback_models = [\"ollama:fb-test\"]\n"),
    );

    let o = hone_json(&e, Some(&openai), None, "say hi");

    let v = o.json();
    assert_eq!(v["success"], true, "{v}\nstderr: {}", o.stderr);
    assert_eq!(v["response"], "ollama rescued it");
    assert_eq!(ollama.models(), vec!["fb-test".to_owned()]);
    assert!(openai.count() >= 1);
}

#[test]
fn test_g10c_an_unreachable_openai_endpoint_falls_back_to_ollama() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama rescued it"));
    // A port nobody listens on.
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    let dead = Mock {
        url: closed,
        seen: Arc::new(Mutex::new(Vec::new())),
    };
    write(
        &e.user_config(),
        &user_config(&ollama, "fallback_models = [\"ollama:fb-test\"]\n"),
    );

    let o = hone_json(&e, Some(&dead), None, "say hi");

    let v = o.json();
    assert_eq!(v["success"], true, "{v}\nstderr: {}", o.stderr);
    assert_eq!(v["response"], "ollama rescued it");
    assert_eq!(ollama.count(), 1);
}

#[test]
fn test_g10d_dash_dash_model_pins_exactly_that_model_with_no_fallback() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama rescued it"));
    let openai = Mock::start(Kind::OpenAi, vec![Reply::FailBeforeOutput]);
    write(
        &e.user_config(),
        &user_config(&ollama, "fallback_models = [\"ollama:fb-test\"]\n"),
    );

    let o = hone_json(&e, Some(&openai), Some("openai:pinned-model"), "say hi");

    let v = o.json();
    assert_eq!(v["success"], false, "{v}\nstderr: {}", o.stderr);
    assert_eq!(ollama.count(), 0, "no fallback after --model");
    assert!(openai.count() >= 1);
    assert_eq!(openai.models()[0], "pinned-model");
}

// -- G10 (e), (f): routing entries that cannot be used --------------------------------------------------------

#[test]
fn test_g10e_routing_to_a_provider_without_a_key_warns_and_uses_the_default_model() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    write(
        &e.user_config(),
        &user_config(
            &ollama,
            "[[routing]]\ntask = \"simple\"\nmodel = \"anthropic:claude-x\"\n",
        ),
    );

    let o = hone_json(&e, Some(&openai), None, SIMPLE);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(
        o.stderr.contains("warning") && o.stderr.to_lowercase().contains("anthropic"),
        "{}",
        o.stderr
    );
    assert_eq!(openai.models(), vec!["main-model".to_owned()]);
    assert_eq!(o.json()["model"], "openai:main-model");
    assert!(!o.stderr.contains("hone: using"), "{}", o.stderr);
}

#[test]
fn test_g10f_routing_to_an_agent_cli_model_warns_and_is_ignored() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    let marker = write_fake_claude(&e);
    write(
        &e.user_config(),
        &user_config(
            &ollama,
            "[[routing]]\ntask = \"simple\"\nmodel = \"claude-cli:claude-x\"\n",
        ),
    );

    let o = hone_json(&e, Some(&openai), None, SIMPLE);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(
        o.stderr.contains("warning") && o.stderr.contains("claude-cli"),
        "{}",
        o.stderr
    );
    assert!(!marker.exists(), "an agent CLI can never be routed to");
    assert_eq!(openai.count(), 1);
}

// -- G10 (g): project over user ---------------------------------------------------------------------------------

#[test]
fn test_g10g_project_routing_overrides_the_users_for_that_tier_only() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    write(
        &e.user_config(),
        &user_config(
            &ollama,
            "[[routing]]\ntask = \"simple\"\nmodel = \"ollama:user-simple\"\n[[routing]]\ntask = \"complex\"\nmodel = \"ollama:user-complex\"\n",
        ),
    );
    write(
        &e.project_config(),
        "[[routing]]\ntask = \"simple\"\nmodel = \"ollama:project-simple\"\n",
    );

    let simple = hone_json(&e, Some(&openai), None, SIMPLE);
    assert_eq!(simple.code, Some(0), "stderr: {}", simple.stderr);
    assert_eq!(simple.json()["model"], "ollama:project-simple");
    assert!(
        simple.stderr.contains(".hone/config.toml)"),
        "the notice names the project file: {}",
        simple.stderr
    );

    let complex = hone_json(&e, Some(&openai), None, COMPLEX);
    assert_eq!(complex.code, Some(0), "stderr: {}", complex.stderr);
    assert_eq!(complex.json()["model"], "ollama:user-complex");
    assert!(
        complex.stderr.contains(".config/hone/config.toml)"),
        "{}",
        complex.stderr
    );

    assert_eq!(
        ollama.models(),
        vec!["project-simple".to_owned(), "user-complex".to_owned()]
    );
    assert_eq!(openai.count(), 0);
}

// -- G6 / G7: pinned model and agent-CLI sessions ------------------------------------------------------------------

#[test]
fn test_g7_dash_dash_model_disables_routing() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    write(
        &e.user_config(),
        &user_config(
            &ollama,
            "[[routing]]\ntask = \"simple\"\nmodel = \"ollama:qwen-test\"\n",
        ),
    );

    let o = hone_json(&e, Some(&openai), Some("openai:pinned-model"), SIMPLE);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(ollama.count(), 0);
    assert_eq!(openai.models(), vec!["pinned-model".to_owned()]);
    assert_eq!(o.json()["model"], "openai:pinned-model");
    assert!(!o.stderr.contains("hone: using"), "{}", o.stderr);
}

#[test]
fn test_g6_routing_and_fallback_are_not_applied_with_an_agent_cli_model() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    let _marker = write_fake_claude(&e);
    write(
        &e.user_config(),
        &format!(
            "default_model = \"claude-cli:claude-x\"\nfallback_models = [\"ollama:fb\"]\n[[routing]]\ntask = \"simple\"\nmodel = \"ollama:qwen-test\"\n[providers.ollama]\nbase_url = {:?}\n",
            ollama.url
        ),
    );

    let o = hone_json(&e, Some(&openai), None, SIMPLE);

    assert_eq!(
        o.stderr
            .matches("not applied with an agent CLI model")
            .count(),
        2,
        "one warning each for [[routing]] and fallback_models: {}",
        o.stderr
    );
    assert_eq!(
        (ollama.count(), openai.count()),
        (0, 0),
        "nothing is routed away from the CLI"
    );
    let _ = e.root;
}

// ==============================================================================================
// Review round 1: B1 (project fallback_models ignored) and G11 (project routing announced)
// ==============================================================================================

#[test]
fn test_g2_a_project_fallback_list_cannot_send_the_session_to_another_provider() {
    // The reviewer's scenario: the repository picks a local-only Ollama model and a fallback
    // to OpenAI; the user's Ollama endpoint is down. Without the rule, the OpenAI key (and
    // the code) would go to OpenAI without the user choosing it.
    let e = Env::new();
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    let closed = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", l.local_addr().unwrap())
    };
    write(
        &e.user_config(),
        &format!("[providers.ollama]\nbase_url = {closed:?}\n"),
    );
    write(
        &e.project_config(),
        "default_model = \"ollama:local-only-model\"\nfallback_models = [\"openai:gpt-4o\"]\n",
    );

    let o = hone_json(&e, Some(&openai), None, "say hi");

    assert_eq!(
        openai.count(),
        0,
        "no request may reach the OpenAI mock: {}",
        o.stderr
    );
    assert_eq!(o.json()["success"], false, "{}", o.stderr);
    assert!(
        o.stderr.contains("warning")
            && o.stderr.contains("fallback_models")
            && o.stderr.contains(".hone/config.toml"),
        "{}",
        o.stderr
    );
}

#[test]
fn test_g2_the_users_fallback_list_still_applies_next_to_a_project_list() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama rescued it"));
    let openai = Mock::start(Kind::OpenAi, vec![Reply::FailBeforeOutput]);
    write(
        &e.user_config(),
        &user_config(&ollama, "fallback_models = [\"ollama:user-fb\"]\n"),
    );
    write(
        &e.project_config(),
        "fallback_models = [\"openai:project-fb\"]\n",
    );

    let o = hone_json(&e, Some(&openai), None, "say hi");

    let v = o.json();
    assert_eq!(v["success"], true, "{v}\nstderr: {}", o.stderr);
    assert_eq!(ollama.models(), vec!["user-fb".to_owned()]);
    assert!(
        !openai.models().iter().any(|m| m == "project-fb"),
        "the project's fallback is never used: {:?}",
        openai.models()
    );
}

/// The start-up announcement (G11), as opposed to the per-request line of G7 ("hone: using ...").
const ANNOUNCE: &str = "hone: routing ";

#[test]
fn test_g11_project_routing_is_announced_at_start_in_every_run() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    write(&e.user_config(), &user_config(&ollama, ""));
    write(
        &e.project_config(),
        "[[routing]]\ntask = \"simple\"\nmodel = \"ollama:proj-simple\"\n",
    );

    // A simple run: both the start-up line and the per-request line.
    let simple = hone_json(&e, Some(&openai), None, SIMPLE);
    assert_eq!(simple.code, Some(0), "stderr: {}", simple.stderr);
    assert_eq!(
        simple
            .stderr
            .matches("hone: routing simple requests to ollama:proj-simple (from ")
            .count(),
        1,
        "{}",
        simple.stderr
    );
    assert!(
        simple.stderr.contains(".hone/config.toml)"),
        "{}",
        simple.stderr
    );
    assert!(
        simple
            .stderr
            .contains("hone: using ollama:proj-simple for this simple request"),
        "the per-request line is separate: {}",
        simple.stderr
    );

    // A complex run is not routed, but the repository's routing is still announced.
    let complex = hone_json(&e, Some(&openai), None, COMPLEX);
    assert_eq!(complex.code, Some(0), "stderr: {}", complex.stderr);
    assert_eq!(
        complex
            .stderr
            .matches("hone: routing simple requests to ollama:proj-simple (from ")
            .count(),
        1,
        "{}",
        complex.stderr
    );
    assert!(
        !complex.stderr.contains("hone: using"),
        "{}",
        complex.stderr
    );
}

#[test]
fn test_g11_every_applied_project_entry_gets_its_own_line() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    write(&e.user_config(), &user_config(&ollama, ""));
    write(
        &e.project_config(),
        "[[routing]]\ntask = \"simple\"\nmodel = \"ollama:proj-simple\"\n[[routing]]\ntask = \"complex\"\nmodel = \"ollama:proj-complex\"\n",
    );

    let o = hone_json(&e, Some(&openai), None, "say hi");

    assert!(
        o.stderr
            .contains("hone: routing simple requests to ollama:proj-simple (from "),
        "{}",
        o.stderr
    );
    assert!(
        o.stderr
            .contains("hone: routing complex requests to ollama:proj-complex (from "),
        "{}",
        o.stderr
    );
}

#[test]
fn test_g11_user_file_routing_is_not_announced() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    write(
        &e.user_config(),
        &user_config(
            &ollama,
            "[[routing]]\ntask = \"simple\"\nmodel = \"ollama:user-simple\"\n",
        ),
    );

    let o = hone_json(&e, Some(&openai), None, SIMPLE);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(
        !o.stderr.contains(ANNOUNCE),
        "the user's own routing needs no announcement: {}",
        o.stderr
    );
    assert!(
        o.stderr
            .contains("hone: using ollama:user-simple for this simple request"),
        "{}",
        o.stderr
    );
}

#[test]
fn test_g11_a_project_entry_that_is_not_applied_is_not_announced() {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, answer("ollama says hi"));
    let openai = Mock::start(Kind::OpenAi, answer("openai says hi"));
    write(&e.user_config(), &user_config(&ollama, ""));
    // No Anthropic key: the entry is dropped with a warning, so nothing is routed.
    write(
        &e.project_config(),
        "[[routing]]\ntask = \"simple\"\nmodel = \"anthropic:claude-x\"\n",
    );

    let o = hone_json(&e, Some(&openai), None, SIMPLE);

    assert!(!o.stderr.contains(ANNOUNCE), "{}", o.stderr);
}

// ==============================================================================================
// Fallback notice (docs/plans/fallback-notice-spec.md): N7 (stderr line) and N8 (--json)
// ==============================================================================================

const NOTICE: &str =
    "hone: openai:main-model failed (openai mock overloaded); switched to fallback ollama:fb-test";

fn fallback_env(ollama_script: Vec<Reply>, fallback_list: &str) -> (Env, Mock, Mock) {
    let e = Env::new();
    let ollama = Mock::start(Kind::Ollama, ollama_script);
    let openai = Mock::start(Kind::OpenAi, vec![Reply::FailBeforeOutput]);
    write(
        &e.user_config(),
        &user_config(&ollama, &format!("fallback_models = {fallback_list}\n")),
    );
    (e, ollama, openai)
}

fn notice_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.contains("switched to fallback"))
        .collect()
}

#[test]
fn test_n7_a_fallback_prints_one_exact_stderr_line_and_leaves_stdout_alone() {
    let (e, ollama, openai) = fallback_env(answer("ollama rescued it"), "[\"ollama:fb-test\"]");

    let o = hone_plain(&e, Some(&openai), None, "say hi");

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(notice_lines(&o.stderr), vec![NOTICE], "{}", o.stderr);
    assert!(o.stdout.contains("ollama rescued it"), "{}", o.stdout);
    assert!(
        !o.stdout.contains("switched to fallback"),
        "stdout is the answer only: {}",
        o.stdout
    );
    assert_eq!(ollama.models(), vec!["fb-test".to_owned()]);
}

#[test]
fn test_n7_json_mode_prints_the_same_stderr_line_and_stdout_stays_one_json_document() {
    let (e, _ollama, openai) = fallback_env(answer("ollama rescued it"), "[\"ollama:fb-test\"]");

    let o = hone_json(&e, Some(&openai), None, "say hi");

    assert_eq!(notice_lines(&o.stderr), vec![NOTICE], "{}", o.stderr);
    assert!(!o.stdout.contains("switched to fallback"), "{}", o.stdout);
    assert_eq!(o.json()["response"], "ollama rescued it");
}

#[test]
fn test_n7_the_line_is_printed_once_even_when_the_fallback_answers_several_calls() {
    // Two model calls in one run (a tool call, then the answer): OpenAI fails both, Ollama
    // answers both.
    let (e, ollama, openai) = fallback_env(
        vec![Reply::ToolCall, Reply::Answer("all done")],
        "[\"ollama:fb-test\"]",
    );

    let o = hone_plain(&e, Some(&openai), None, "say hi");

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(ollama.count(), 2, "the fallback answered two calls");
    assert_eq!(
        notice_lines(&o.stderr),
        vec![NOTICE],
        "once per pair per run: {}",
        o.stderr
    );
}

#[test]
fn test_n7_no_line_when_the_requested_model_answers() {
    let (e, ollama, _) = fallback_env(answer("never"), "[\"ollama:fb-test\"]");
    let openai = Mock::start(Kind::OpenAi, answer("openai is fine"));

    let o = hone_plain(&e, Some(&openai), None, "say hi");

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(!o.stderr.contains("switched to fallback"), "{}", o.stderr);
    assert_eq!(ollama.count(), 0);
}

#[test]
fn test_n7_no_line_when_every_model_fails() {
    let (e, _ollama, openai) = fallback_env(vec![Reply::FailBeforeOutput], "[\"ollama:fb-test\"]");

    let o = hone_json(&e, Some(&openai), None, "say hi");

    assert_eq!(o.json()["success"], false, "{}", o.stderr);
    assert!(!o.stderr.contains("switched to fallback"), "{}", o.stderr);
    assert_eq!(o.json()["fallbacks"], json!([]));
}

#[test]
fn test_n8_json_lists_the_fallback_with_a_call_count_and_keeps_model_and_success() {
    let (e, _ollama, openai) = fallback_env(
        vec![Reply::ToolCall, Reply::Answer("all done")],
        "[\"ollama:fb-test\"]",
    );

    let o = hone_json(&e, Some(&openai), None, "say hi");

    let v = o.json();
    assert_eq!(v["success"], true, "{v}\nstderr: {}", o.stderr);
    assert_eq!(v["model"], "openai:main-model", "model keeps its meaning");
    assert_eq!(
        v["fallbacks"],
        json!([{
            "requested": "openai:main-model",
            "fallback": "ollama:fb-test",
            "reason": "openai mock overloaded",
            "calls": 2
        }])
    );
}

#[test]
fn test_n8_json_without_a_fallback_has_an_empty_array_and_no_other_key_changes() {
    let e = Env::new();
    let openai = Mock::start(Kind::OpenAi, answer("mock reply"));

    let o = hone_json(&e, Some(&openai), None, "say hi");

    let v = o.json();
    assert_eq!(v["fallbacks"], json!([]));
    assert!(v["context"].is_array(), "CR9: context is always an array");
    let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        [
            "context",
            "cost_usd",
            "error",
            "fallbacks",
            "model",
            "response",
            "success",
            "tool_calls",
            "usage"
        ]
    );
}

#[test]
fn test_n8_two_fallback_models_taking_over_on_different_calls_give_two_entries_in_first_seen_order()
{
    // Call 1: OpenAI fails, fb1 answers with a tool call. Call 2: OpenAI fails, fb1 fails
    // before output, fb2 answers. Reason of both pairs comes from member 0 (OpenAI).
    let (e, ollama, openai) = fallback_env(
        vec![
            Reply::ToolCall,
            Reply::FailBeforeOutput,
            Reply::Answer("fb2 answered"),
        ],
        "[\"ollama:fb1\", \"ollama:fb2\"]",
    );

    let o = hone_json(&e, Some(&openai), None, "say hi");

    let v = o.json();
    assert_eq!(v["success"], true, "{v}\nstderr: {}", o.stderr);
    assert_eq!(
        ollama.models(),
        vec!["fb1".to_owned(), "fb1".to_owned(), "fb2".to_owned()]
    );
    assert_eq!(
        v["fallbacks"],
        json!([
            {"requested": "openai:main-model", "fallback": "ollama:fb1",
             "reason": "openai mock overloaded", "calls": 1},
            {"requested": "openai:main-model", "fallback": "ollama:fb2",
             "reason": "openai mock overloaded", "calls": 1},
        ])
    );
    assert_eq!(
        notice_lines(&o.stderr),
        vec![
            "hone: openai:main-model failed (openai mock overloaded); switched to fallback ollama:fb1",
            "hone: openai:main-model failed (openai mock overloaded); switched to fallback ollama:fb2",
        ],
        "{}",
        o.stderr
    );
}
