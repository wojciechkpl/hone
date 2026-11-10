#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Binary-level red-phase tests for review round 1 (`docs/plans/config-apply-spec.md` section 3b):
//! R1 (project file may not choose an agent CLI), R2 (notice when the project chose the model),
//! R3 (model-name validation at every source), R4 (loader hardening), R5 (project choice that
//! cannot be built falls back to auto-detection).
//!
//! Same harness as `config_apply.rs`: temp HOME and working directory, cleared environment,
//! `HONE_DISABLE_PERSISTENT_STORES=1`, a local mock OpenAI-compatible server, fake `claude` /
//! `gemini` scripts on a private PATH that leave a marker file when really started.
//! Credential-shaped fixtures are built at runtime. Every child process has a timeout.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// -- mock OpenAI-compatible server (same as config_apply.rs) -------------------------------------

#[derive(Clone, Debug)]
struct Recorded {
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
    log.lock().unwrap().push(Recorded { body });

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

// -- harness -------------------------------------------------------------------------------------

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    bin_dir: PathBuf,
    home: PathBuf,
    wd: PathBuf,
}

impl Env {
    fn user_config(&self) -> PathBuf {
        self.home.join(".config/hone/config.toml")
    }
    fn project_dir(&self) -> PathBuf {
        self.wd.join(".hone")
    }
    fn project_config(&self) -> PathBuf {
        self.project_dir().join("config.toml")
    }
    fn marker(&self, name: &str) -> PathBuf {
        self.bin_dir.join(format!("{name}-was-run"))
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
        root,
        bin_dir,
        home,
        wd,
    }
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

/// A fake agent CLI (`claude` or `gemini`) that leaves a marker once really started.
fn write_fake_cli(e: &Env, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let bin = e.bin_dir.join(name);
    std::fs::write(
        &bin,
        format!(
            "#!/bin/sh\ncase \"$1\" in --version|-v) echo 1.0.0; exit 0;; esac\ntouch '{}'\necho '{{\"type\":\"result\",\"result\":\"fake\"}}'\n",
            e.marker(name).display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
}

struct Out {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn hone(e: &Env, extra_env: &[(&str, &str)], args: &[&str]) -> Out {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hone"));
    cmd.env_clear()
        .env("HOME", &e.home)
        .env("PATH", format!("{}:/usr/bin:/bin", e.bin_dir.display()))
        .env("HONE_DISABLE_PERSISTENT_STORES", "1")
        .current_dir(&e.wd)
        .args(args)
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
            panic!("hone timed out (args {args:?})");
        }
    }
}

fn hone_json(e: &Env, extra_env: &[(&str, &str)], model: Option<&str>) -> Out {
    let mut args = vec!["--no-clarify", "--no-inject", "--json"];
    if let Some(m) = model {
        args.push("--model");
        args.push(m);
    }
    args.push("say hi");
    hone(e, extra_env, &args)
}

fn token_value(prefix: &str) -> String {
    format!("{prefix}-{}", "fixture0123456789")
}

/// Environment that makes an OpenAI-compatible run succeed against `mock`.
fn openai_env(mock: &Mock) -> Vec<(&'static str, String)> {
    vec![
        ("OPENAI_API_KEY", token_value("sk")),
        ("HONE_OPENAI_BASE_URL", mock.base_url.clone()),
    ]
}

fn as_refs<'a>(v: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    v.iter().map(|(k, s)| (*k, s.as_str())).collect()
}

// -- R1: a project file may not choose an agent CLI -------------------------------------------------

#[test]
fn test_r1_project_cli_default_model_does_not_spawn_the_cli_and_warns() {
    for (prefix, bin) in [("claude-cli", "claude"), ("gemini-cli", "gemini")] {
        let e = env();
        let mock = Mock::start();
        write_fake_cli(&e, bin);
        write(
            &e.project_config(),
            &format!("default_model = \"{prefix}:some-model\"\n"),
        );

        let env_vars = openai_env(&mock);
        let o = hone_json(&e, &as_refs(&env_vars), None);

        assert!(
            !e.marker(bin).exists(),
            "{prefix}: the repository must not be able to start an agent CLI; stderr: {}",
            o.stderr
        );
        assert_eq!(mock.requests().len(), 1, "{prefix}: the API provider answers: {}", o.stderr);
        assert!(o.stderr.contains("warning"), "{prefix}: {}", o.stderr);
        assert!(
            o.stderr.contains(".hone/config.toml") && o.stderr.contains("default_model"),
            "{prefix}: {}",
            o.stderr
        );
        serde_json::from_str::<serde_json::Value>(&o.stdout).expect("clean JSON on stdout");
    }
}

#[test]
fn test_r1_user_file_may_still_choose_an_agent_cli() {
    let e = env();
    let mock = Mock::start();
    write_fake_cli(&e, "claude");
    write(&e.user_config(), "default_model = \"claude-cli:claude-sonnet-4-5\"\n");

    let env_vars = openai_env(&mock);
    let _ = hone_json(&e, &as_refs(&env_vars), None);

    assert!(e.marker("claude").exists(), "the trusted user file may choose claude-cli");
    assert!(mock.requests().is_empty());
}

// -- R2: tell the user when a repository chose the model ---------------------------------------------------

#[test]
fn test_r2_project_default_model_prints_a_notice_once_and_keeps_stdout_clean() {
    let e = env();
    let mock = Mock::start();
    write(&e.project_config(), "default_model = \"openai:proj-model\"\n");

    let env_vars = openai_env(&mock);
    let o = hone_json(&e, &as_refs(&env_vars), None);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert_eq!(mock.requests()[0].model(), "proj-model");
    assert_eq!(
        o.stderr.matches("hone: using model openai:proj-model (from ").count(),
        1,
        "{}",
        o.stderr
    );
    assert!(o.stderr.contains(".hone/config.toml)"), "{}", o.stderr);
    serde_json::from_str::<serde_json::Value>(&o.stdout).expect("clean JSON on stdout");
    assert!(!o.stdout.contains("using model"));
}

#[test]
fn test_r2_no_notice_for_the_user_file_or_for_dash_dash_model() {
    let e = env();
    let mock = Mock::start();
    write(&e.user_config(), "default_model = \"openai:user-model\"\n");
    let env_vars = openai_env(&mock);

    let o = hone_json(&e, &as_refs(&env_vars), None);
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(!o.stderr.contains("using model"), "{}", o.stderr);

    let o = hone_json(&e, &as_refs(&env_vars), Some("openai:cli-model"));
    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    assert!(!o.stderr.contains("using model"), "{}", o.stderr);
}

// -- R3: model-name validation -----------------------------------------------------------------------------------

#[test]
fn test_r3_dash_dash_model_with_a_bad_name_exits_1_with_the_invalid_model_message() {
    for bad in ["openai:", "gemini-cli:--yolo", "ollama:has space", "openai:-x", "anthropic:"] {
        let e = env();
        let mock = Mock::start();
        write_fake_cli(&e, "gemini");

        let env_vars = openai_env(&mock);
        let o = hone_json(&e, &as_refs(&env_vars), Some(bad));

        assert_eq!(o.code, Some(1), "{bad:?}: stderr: {}", o.stderr);
        assert!(
            o.stderr.to_lowercase().contains("invalid model"),
            "{bad:?}: {}",
            o.stderr
        );
        assert!(mock.requests().is_empty(), "{bad:?}");
        assert!(!e.marker("gemini").exists(), "{bad:?}: argument injection reached the CLI");
    }
}

#[test]
fn test_r3_config_sources_with_a_bad_name_exit_2() {
    for (label, file, raw) in [
        ("user default_model", "user", "default_model = \"gemini-cli:--yolo\"\n"),
        ("user default_model empty", "user", "default_model = \"openai:\"\n"),
        ("project default_model", "project", "default_model = \"openai:-x\"\n"),
        ("providers model", "user", "[providers.openai]\nmodel = \"-x\"\n"),
        ("project providers model", "project", "[providers.ollama]\nmodel = \"a b\"\n"),
        ("wizard model", "user", "[provider]\nname = \"openai\"\nmodel = \"a b\"\n"),
    ] {
        let e = env();
        let mock = Mock::start();
        write_fake_cli(&e, "gemini");
        let path = if file == "user" { e.user_config() } else { e.project_config() };
        write(&path, raw);

        let env_vars = openai_env(&mock);
        let o = hone_json(&e, &as_refs(&env_vars), None);

        assert_eq!(o.code, Some(2), "{label}: stderr: {}", o.stderr);
        assert!(o.stderr.contains("config error in"), "{label}: {}", o.stderr);
        assert!(mock.requests().is_empty(), "{label}");
        assert!(!e.marker("gemini").exists(), "{label}");
    }
}

// -- R4: loader hardening ------------------------------------------------------------------------------------------

const LEAK: &str = "leak_marker_line_zzz";

fn assert_config_error_without_leak(label: &str, o: &Out, mock: &Mock) {
    assert_eq!(o.code, Some(2), "{label}: stderr: {}", o.stderr);
    assert!(o.stderr.contains("config error in"), "{label}: {}", o.stderr);
    assert!(!o.stderr.contains(LEAK), "{label}: the error echoed file content: {}", o.stderr);
    assert!(mock.requests().is_empty(), "{label}: no request may be sent");
}

#[test]
fn test_r4_symlinked_project_file_is_a_config_error_that_does_not_print_its_target() {
    let e = env();
    let mock = Mock::start();
    let outside = e.root.join("outside.txt");
    write(&outside, &format!("{LEAK} = = 1\n"));
    std::fs::create_dir_all(e.project_dir()).unwrap();
    symlink(&outside, e.project_config()).unwrap();

    let env_vars = openai_env(&mock);
    let o = hone_json(&e, &as_refs(&env_vars), None);

    assert_config_error_without_leak("symlinked file", &o, &mock);
}

#[test]
fn test_r4_symlinked_dot_hone_directory_is_a_config_error() {
    let e = env();
    let mock = Mock::start();
    let elsewhere = e.root.join("elsewhere");
    write(&elsewhere.join("config.toml"), &format!("{LEAK} = = 1\n"));
    symlink(&elsewhere, e.project_dir()).unwrap();

    let env_vars = openai_env(&mock);
    let o = hone_json(&e, &as_refs(&env_vars), None);

    assert_config_error_without_leak("symlinked .hone", &o, &mock);
}

#[test]
fn test_r4_parse_error_in_a_regular_project_file_does_not_echo_the_file() {
    let e = env();
    let mock = Mock::start();
    write(&e.project_config(), &format!("# ok\n{LEAK} = = 1\n"));

    let env_vars = openai_env(&mock);
    let o = hone_json(&e, &as_refs(&env_vars), None);

    assert_config_error_without_leak("parse error", &o, &mock);
    assert!(o.stderr.contains("line 2"), "{}", o.stderr);
}

#[test]
fn test_r4_oversized_and_non_utf8_files_are_config_errors() {
    let big = format!("{}\n", "# padding ".repeat(8000)); // ~80 KB of valid TOML comments
    assert!(big.len() > 64 * 1024);
    for (label, bytes) in [
        ("oversized", big.into_bytes()),
        ("not utf-8", vec![b'#', b' ', 0xff, 0xfe, b'\n']),
    ] {
        let e = env();
        let mock = Mock::start();
        std::fs::create_dir_all(e.project_dir()).unwrap();
        std::fs::write(e.project_config(), &bytes).unwrap();

        let env_vars = openai_env(&mock);
        let o = hone_json(&e, &as_refs(&env_vars), None);

        assert_config_error_without_leak(label, &o, &mock);
    }
}

// -- R5: a project choice that cannot be built falls back to auto-detection ---------------------------------------

#[test]
fn test_r5_project_default_model_without_a_key_falls_back_to_detection_with_its_own_model() {
    let e = env();
    let mock = Mock::start();
    write(&e.project_config(), "default_model = \"anthropic:proj-claude\"\n");

    // Only an OpenAI-compatible key exists: detection must run on OpenAI, with ITS model id.
    let env_vars = openai_env(&mock);
    let o = hone_json(&e, &as_refs(&env_vars), None);

    assert_eq!(o.code, Some(0), "stderr: {}", o.stderr);
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1, "stderr: {}", o.stderr);
    assert_eq!(reqs[0].model(), "gpt-4o", "detection's own model, not the project's id");
    assert!(o.stderr.contains("warning"), "{}", o.stderr);
    assert!(
        o.stderr.contains(".hone/config.toml") && o.stderr.to_lowercase().contains("anthropic"),
        "{}",
        o.stderr
    );
    let v: serde_json::Value = serde_json::from_str(&o.stdout).expect("clean JSON on stdout");
    assert_eq!(v["model"], "openai:gpt-4o");
}

#[test]
fn test_r5_user_file_and_dash_dash_model_keep_the_hard_error() {
    let e = env();
    let mock = Mock::start();
    write(&e.user_config(), "default_model = \"anthropic:user-claude\"\n");
    let env_vars = openai_env(&mock);

    let o = hone_json(&e, &as_refs(&env_vars), None);
    assert_eq!(o.code, Some(1), "user file: {}", o.stderr);
    assert!(o.stderr.contains("ANTHROPIC_API_KEY"), "{}", o.stderr);

    let e2 = env();
    let o = hone_json(&e2, &as_refs(&env_vars), Some("anthropic:cli-claude"));
    assert_eq!(o.code, Some(1), "--model: {}", o.stderr);
    assert!(mock.requests().is_empty());
}
