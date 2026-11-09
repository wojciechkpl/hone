#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Binary-level red-phase tests for `docs/plans/config-apply-spec.md` section 5 (C1-C15).
//!
//! Each test spawns the built `hone` with a temp HOME and working directory, a cleared
//! environment (no provider variables, no `claude` / `gemini` on PATH unless a test adds a fake),
//! `HONE_DISABLE_PERSISTENT_STORES=1`, and a local mock OpenAI-compatible server (a std
//! `TcpListener` thread that records requests and streams one SSE reply).
//!
//! Credential-shaped fixtures are built at runtime. Every child process has a timeout.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// -- mock OpenAI-compatible server ---------------------------------------------------------------

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
    /// `prompt_tokens` is reported in the usage chunk (to make a run cost money).
    fn start(prompt_tokens: u64) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let log = Arc::clone(&log);
                std::thread::spawn(move || serve(stream, &log, prompt_tokens));
            }
        });
        Mock { base_url, requests }
    }

    fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }
}

fn serve(mut stream: TcpStream, log: &Mutex<Vec<Recorded>>, prompt_tokens: u64) {
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
    let c2 = format!(
        r#"{{"choices":[{{"delta":{{}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":{prompt_tokens},"completion_tokens":1}}}}"#
    );
    let payload = format!("data: {c1}\n\ndata: {c2}\n\ndata: [DONE]\n\n");
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{payload}",
        payload.len()
    );
    let _ = stream.write_all(resp.as_bytes());
    let _ = stream.flush();
}

// -- harness ---------------------------------------------------------------------------------------

struct Env {
    _tmp: tempfile::TempDir,
    bin_dir: PathBuf,
    home: PathBuf,
    wd: PathBuf,
}

impl Env {
    fn user_config(&self) -> PathBuf {
        self.home.join(".config/hone/config.toml")
    }
    fn project_config(&self) -> PathBuf {
        self.wd.join(".hone/config.toml")
    }
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (bin_dir, home, wd) = (root.join("bin"), root.join("home"), root.join("wd"));
    for d in [&bin_dir, &home, &wd] {
        std::fs::create_dir_all(d).unwrap();
    }
    Env {
        _tmp: tmp,
        bin_dir,
        home,
        wd,
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

fn hone_with_stdin(e: &Env, extra_env: &[(&str, &str)], args: &[&str], stdin: &str) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hone"));
    cmd.env_clear()
        .env("HOME", &e.home)
        .env("PATH", format!("{}:/usr/bin:/bin", e.bin_dir.display()))
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .current_dir(&e.wd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("spawn hone");
    let pid = child.id();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
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
            panic!("hone timed out (args {args:?})");
        }
    }
}

fn hone(e: &Env, extra_env: &[(&str, &str)], args: &[&str]) -> Out {
    hone_with_stdin(e, extra_env, args, "")
}

/// One-shot, structured output: `hone --no-clarify --no-inject --json <prompt>`.
fn hone_json(e: &Env, extra_env: &[(&str, &str)], model: Option<&str>) -> Out {
    let mut args = vec!["--no-clarify", "--no-inject", "--json"];
    if let Some(m) = model {
        args.push("--model");
        args.push(m);
    }
    args.push("say hi");
    hone(e, extra_env, &args)
}

fn secret(prefix: &str) -> String {
    format!("{prefix}-{}", "fixture0123456789")
}

// -- C1 + C2 + C4 + C3(user) ------------------------------------------------------------------------

#[test]
fn test_c1_default_model_with_user_base_url_and_api_key_env_should_reach_the_mock() {
    let e = env();
    let mock = Mock::start(10);
    let k = secret("sk-team");
    write(
        &e.user_config(),
        &format!(
            "default_model = \"openai:cfg-model\"\n[providers.openai]\nbase_url = {:?}\napi_key_env = \"MY_TEAM_KEY\"\n",
            mock.base_url
        ),
    );

    let o = hone_json(&e, &[("MY_TEAM_KEY", &k)], None);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    let reqs = mock.requests();
    assert_eq!(
        reqs.len(),
        1,
        "the mock must get exactly one request; stderr: {}",
        o.stderr
    );
    assert_eq!(reqs[0].model(), "cfg-model");
    assert_eq!(reqs[0].authorization, format!("Bearer {k}"));
    assert_eq!(reqs[0].path, "/v1/chat/completions");
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("--json stdout is JSON");
    assert_eq!(v["success"], true);
    assert_eq!(v["model"], "openai:cfg-model");
}

#[test]
fn test_c2_providers_openai_model_is_used_when_detection_picks_openai() {
    let e = env();
    let mock = Mock::start(10);
    let k = secret("sk");
    write(
        &e.user_config(),
        &format!(
            "[providers.openai]\nbase_url = {:?}\nmodel = \"cfg-detect-model\"\n",
            mock.base_url
        ),
    );

    let o = hone_json(&e, &[("OPENAI_API_KEY", &k)], None);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1, "stderr: {}", o.stderr);
    assert_eq!(reqs[0].model(), "cfg-detect-model");
}

#[test]
fn test_c3_environment_base_url_beats_the_file_base_url() {
    let e = env();
    let file_mock = Mock::start(10);
    let env_mock = Mock::start(10);
    write(
        &e.user_config(),
        &format!("[providers.openai]\nbase_url = {:?}\n", file_mock.base_url),
    );

    let o = hone_json(
        &e,
        &[
            ("OPENAI_API_KEY", &secret("sk")),
            ("HONE_OPENAI_BASE_URL", &env_mock.base_url),
        ],
        None,
    );

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(env_mock.requests().len(), 1, "stderr: {}", o.stderr);
    assert!(file_mock.requests().is_empty());
}

// -- C3 trust model + C14 -------------------------------------------------------------------------------

#[test]
fn test_c3_project_base_url_is_ignored_with_a_stderr_warning_and_json_stdout_stays_clean() {
    let e = env();
    let user_mock = Mock::start(10);
    let evil_mock = Mock::start(10);
    let k = secret("sk");
    write(
        &e.user_config(),
        &format!(
            "default_model = \"openai:m\"\n[providers.openai]\nbase_url = {:?}\n",
            user_mock.base_url
        ),
    );
    write(
        &e.project_config(),
        &format!("[providers.openai]\nbase_url = {:?}\n", evil_mock.base_url),
    );

    let o = hone_json(&e, &[("OPENAI_API_KEY", &k)], None);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(
        user_mock.requests().len(),
        1,
        "the trusted endpoint gets the request: {}",
        o.stderr
    );
    assert!(
        evil_mock.requests().is_empty(),
        "the repository's endpoint must never see the credential"
    );
    assert!(o.stderr.contains("warning"), "{}", o.stderr);
    assert!(
        o.stderr.contains(".hone/config.toml") && o.stderr.contains("base_url"),
        "{}",
        o.stderr
    );
    serde_json::from_str::<serde_json::Value>(&o.stdout)
        .unwrap_or_else(|err| panic!("stdout must stay valid JSON ({err}): {}", o.stdout));
    assert!(!o.stdout.contains("warning"), "{}", o.stdout);
}

#[test]
fn test_c8_unapplied_sections_warn_on_stderr_only() {
    let e = env();
    let mock = Mock::start(10);
    write(
        &e.user_config(),
        &format!(
            "default_model = \"openai:m\"\n[providers.openai]\nbase_url = {:?}\n[sandbox]\nallowed_commands = [\"ls\"]\n",
            mock.base_url
        ),
    );

    let o = hone_json(&e, &[("OPENAI_API_KEY", &secret("sk"))], None);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(
        o.stderr.contains("warning") && o.stderr.contains("sandbox"),
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains("permissions.toml"), "{}", o.stderr);
    assert_eq!(
        o.stderr.matches("config.toml: ").count(),
        1,
        "one warning, once per start: {}",
        o.stderr
    );
    serde_json::from_str::<serde_json::Value>(&o.stdout).expect("clean JSON on stdout");
}

// -- C12 / C13 ----------------------------------------------------------------------------------------------

#[test]
fn test_c12_a_broken_user_file_stops_with_exit_2_naming_the_path_and_sends_no_request() {
    for (label, raw) in [
        ("syntax", "default_model = = oops"),
        ("unknown key", "totally_unknown = 1"),
        ("wrong type", "[budget]\nsession_limit_usd = \"lots\""),
        ("unknown provider", "[providers.skynet]\nmodel = \"x\""),
        ("bad default_model", "default_model = \"gpt-4o\""),
        ("negative limit", "[budget]\nsession_limit_usd = -1.0"),
    ] {
        let e = env();
        let mock = Mock::start(10);
        write(&e.user_config(), raw);

        // The environment would make the run succeed against the mock if the file were skipped.
        let o = hone_json(
            &e,
            &[
                ("OPENAI_API_KEY", &secret("sk")),
                ("HONE_OPENAI_BASE_URL", &mock.base_url),
            ],
            None,
        );

        assert_eq!(o.code, Some(2), "{label}: stderr: {}", o.stderr);
        assert!(o.stderr.contains("config error in"), "{label}: {}", o.stderr);
        assert!(
            o.stderr.contains(".config/hone/config.toml"),
            "{label}: {}",
            o.stderr
        );
        assert!(mock.requests().is_empty(), "{label}: no request may be sent");
    }
}

#[test]
fn test_c12_a_broken_project_file_also_stops_with_exit_2() {
    let e = env();
    let mock = Mock::start(10);
    write(&e.project_config(), "[budget]\nsession_limit_usd = nope");

    let o = hone_json(
        &e,
        &[
            ("OPENAI_API_KEY", &secret("sk")),
            ("HONE_OPENAI_BASE_URL", &mock.base_url),
        ],
        None,
    );

    assert_eq!(o.code, Some(2), "stderr: {}", o.stderr);
    assert!(o.stderr.contains(".hone/config.toml"), "{}", o.stderr);
    assert!(mock.requests().is_empty());
}

#[test]
fn test_c12_plain_one_shot_is_also_covered() {
    let e = env();
    let mock = Mock::start(10);
    write(&e.user_config(), "totally_unknown = 1");
    let o = hone(
        &e,
        &[
            ("OPENAI_API_KEY", &secret("sk")),
            ("HONE_OPENAI_BASE_URL", &mock.base_url),
        ],
        &["--no-clarify", "--no-inject", "say hi"],
    );
    assert_eq!(o.code, Some(2), "{}", o.stderr);
    assert!(mock.requests().is_empty());
}

#[test]
fn test_c13_configure_still_runs_when_the_existing_file_is_broken_and_rewrites_it() {
    let e = env();
    write(&e.user_config(), "this is = = not toml");

    // provider 6 = Claude CLI (no key), model 1.
    let o = hone_with_stdin(&e, &[], &["configure"], "6\n1\n");

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    let written = std::fs::read_to_string(e.user_config()).unwrap();
    assert!(written.contains("claude-cli"), "{written}");
}

#[test]
fn test_c13_session_subcommands_do_not_read_config_toml() {
    let e = env();
    write(&e.user_config(), "this is = = not toml");
    let o = hone(&e, &[], &["session", "list"]);
    assert!(!o.stderr.contains("config error"), "{}", o.stderr);
    assert_ne!(o.code, Some(2), "{}", o.stderr);
}

// -- C9: explicit model builds its own provider --------------------------------------------------------------

#[test]
fn test_c9_cli_openai_model_without_a_key_exits_1_naming_model_and_variable() {
    let e = env();
    // A fake `claude` on PATH would be picked by auto-detection: it must NOT be.
    write_fake_claude(&e);
    let o = hone_json(&e, &[], Some("openai:gpt-4o"));
    assert_eq!(o.code, Some(1), "stderr: {}", o.stderr);
    assert!(o.stderr.contains("--model"), "{}", o.stderr);
    assert!(o.stderr.contains("OPENAI_API_KEY"), "{}", o.stderr);
    assert!(
        !e.bin_dir.join("claude-was-run").exists(),
        "fell back to auto-detection"
    );
}

#[test]
fn test_c9_cli_anthropic_model_without_a_key_exits_1() {
    let e = env();
    write_fake_claude(&e);
    let o = hone_json(&e, &[], Some("anthropic:claude-sonnet-4-5"));
    assert_eq!(o.code, Some(1), "stderr: {}", o.stderr);
    assert!(o.stderr.contains("ANTHROPIC_API_KEY"), "{}", o.stderr);
    assert!(!e.bin_dir.join("claude-was-run").exists());
}

#[test]
fn test_c9_default_model_without_a_key_exits_1_naming_the_file() {
    let e = env();
    write_fake_claude(&e);
    write(&e.user_config(), "default_model = \"openai:gpt-4o\"\n");
    let o = hone_json(&e, &[], None);
    assert_eq!(o.code, Some(1), "stderr: {}", o.stderr);
    assert!(
        o.stderr.contains(".config/hone/config.toml"),
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains("OPENAI_API_KEY"), "{}", o.stderr);
}

#[test]
fn test_c9_named_key_variable_unset_is_reported_by_name() {
    let e = env();
    let mock = Mock::start(10);
    write(
        &e.user_config(),
        &format!(
            "default_model = \"openai:m\"\n[providers.openai]\nbase_url = {:?}\napi_key_env = \"MY_TEAM_KEY\"\n",
            mock.base_url
        ),
    );
    // OPENAI_API_KEY is set, but the file names another variable: no silent fallback.
    let o = hone_json(&e, &[("OPENAI_API_KEY", &secret("sk"))], None);
    assert_eq!(o.code, Some(1), "stderr: {}", o.stderr);
    assert!(o.stderr.contains("MY_TEAM_KEY"), "{}", o.stderr);
    assert!(mock.requests().is_empty());
}

#[test]
fn test_c9_cli_claude_cli_model_uses_the_cli_adapter_even_when_an_openai_key_is_set() {
    let e = env();
    let mock = Mock::start(10);
    write_fake_claude(&e);

    let _ = hone_json(
        &e,
        &[
            ("OPENAI_API_KEY", &secret("sk")),
            ("HONE_OPENAI_BASE_URL", &mock.base_url),
        ],
        Some("claude-cli:claude-sonnet-4-5"),
    );

    assert!(
        mock.requests().is_empty(),
        "an explicit claude-cli model must not go to OpenAI"
    );
    assert!(
        e.bin_dir.join("claude-was-run").exists(),
        "the claude CLI adapter must be used"
    );
}

/// A fake `claude` that records it was started past the version probe.
fn write_fake_claude(e: &Env) {
    use std::os::unix::fs::PermissionsExt;
    let claude = e.bin_dir.join("claude");
    let marker = e.bin_dir.join("claude-was-run");
    std::fs::write(
        &claude,
        format!(
            "#!/bin/sh\ncase \"$1\" in --version|-v) echo 1.0.0; exit 0;; esac\ntouch '{}'\necho '{{\"type\":\"result\",\"result\":\"fake\"}}'\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
}

// -- C6 / C15: wizard files ---------------------------------------------------------------------------------------

#[test]
fn test_c6_hand_written_wizard_table_supplies_key_endpoint_and_implied_model() {
    let e = env();
    let mock = Mock::start(10);
    let k = secret("sk-wiz");
    write(
        &e.user_config(),
        &format!(
            "[provider]\nname = \"openai\"\napi_key = {k:?}\nmodel = \"wiz-model\"\nbase_url = {:?}\n",
            mock.base_url
        ),
    );

    let o = hone_json(&e, &[], None);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1, "stderr: {}", o.stderr);
    assert_eq!(reqs[0].model(), "wiz-model");
    assert_eq!(reqs[0].authorization, format!("Bearer {k}"));
    assert!(!o.stderr.contains("warning"), "{}", o.stderr);
}

#[test]
fn test_c15_a_file_written_by_configure_loads_cleanly_and_its_implied_model_is_used() {
    let e = env();
    let mock = Mock::start(10);
    let k = secret("sk-typed");

    // provider 2 = OpenAI, then the credential, then model 1 (gpt-4o).
    let w = hone_with_stdin(&e, &[], &["configure"], &format!("2\n{k}\n1\n"));
    assert_eq!(w.code, Some(0), "configure failed: {}", w.stderr);

    // The wizard's OpenAI entry has no base_url; the environment points at the mock.
    let o = hone_json(&e, &[("HONE_OPENAI_BASE_URL", &mock.base_url)], None);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(
        !o.stderr.contains("warning") && !o.stderr.contains("config error"),
        "{}",
        o.stderr
    );
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1, "stderr: {}", o.stderr);
    assert_eq!(reqs[0].model(), "gpt-4o");
    assert_eq!(reqs[0].authorization, format!("Bearer {k}"));
}

// -- C7 + C11: session_limit_usd reaches the agent -------------------------------------------------------------------------

/// `--recipe onboarding` runs several turns on one agent; the budget is checked at each turn start.
fn run_recipe(e: &Env, mock: &Mock) -> Out {
    hone(
        e,
        &[
            ("OPENAI_API_KEY", &secret("sk")),
            ("HONE_OPENAI_BASE_URL", &mock.base_url),
        ],
        &[
            "--no-clarify",
            "--no-inject",
            "--model",
            "openai:budget-model",
            "--recipe",
            "onboarding",
        ],
    )
}

#[test]
fn test_c11_session_limit_stops_a_multi_turn_run_with_the_budget_error() {
    let e = env();
    // 1M prompt tokens per call at the default $3/M => each turn costs ~$3.
    let mock = Mock::start(1_000_000);
    write(&e.user_config(), "[budget]\nsession_limit_usd = 0.01\n");

    let o = run_recipe(&e, &mock);

    let all = format!("{}\n{}", o.stdout, o.stderr);
    assert!(
        all.contains("budget exceeded"),
        "the cap must reach the agent: {all}"
    );
    assert_eq!(
        mock.requests().len(),
        1,
        "turn 2 must be refused before any model call: {all}"
    );
}

#[test]
fn test_c11_control_without_a_limit_runs_every_turn() {
    let e = env();
    let mock = Mock::start(1_000_000);

    let o = run_recipe(&e, &mock);

    let all = format!("{}\n{}", o.stdout, o.stderr);
    assert!(!all.contains("budget exceeded"), "{all}");
    assert!(mock.requests().len() > 1, "{all}");
}

#[test]
fn test_c7_a_project_file_can_only_lower_the_user_limit() {
    let e = env();
    let mock = Mock::start(1_000_000);
    write(&e.user_config(), "[budget]\nsession_limit_usd = 1000.0\n");
    write(&e.project_config(), "[budget]\nsession_limit_usd = 0.01\n");

    let o = run_recipe(&e, &mock);

    let all = format!("{}\n{}", o.stdout, o.stderr);
    assert!(all.contains("budget exceeded"), "{all}");
    assert_eq!(mock.requests().len(), 1, "{all}");
}
