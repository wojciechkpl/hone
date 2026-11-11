#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P10 of `docs/plans/mcp-parallel-connect-spec.md`: no hone exit leaves an MCP
//! server running. Each case runs the real `hone` binary with a project
//! `.hone/mcp.json` starting a stubborn Python server (it ignores its stdin
//! closing), lets hone end in a different way, and then requires the server to
//! be gone within ~2 s. Every case first asserts the server really started.
//!
//! Exit cases keep today's exit codes; the signal cases require hone to die
//! *by* the signal (PR #90 W37).

use std::net::TcpListener;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Answers the handshake, then ignores the end of its input. argv[1] = pid file.
const STUBBORN: &str = r#"
import json, os, sys, time
open(sys.argv[1], "w").write(str(os.getpid()))
while True:
    line = sys.stdin.readline()
    if not line:
        time.sleep(30)
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    result = {"tools": []} if req.get("method") == "tools/list" else {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

fn alive(pid: &str) -> bool {
    let out = Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

struct Setup {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    wd: PathBuf,
    pid_file: PathBuf,
}

impl Drop for Setup {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(&self.pid_file) {
            let _ = Command::new("kill")
                .args(["-9", pid.trim()])
                .stderr(Stdio::null())
                .status();
        }
    }
}

/// `base_url` is where the (dummy) OpenAI-compatible provider is said to live.
fn setup(base_url: &str, permissions: Option<&str>) -> Setup {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, wd) = (root.join("home"), root.join("wd"));
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&wd).unwrap();
    let script = root.join("stubborn.py");
    let pid_file = root.join("server.pid");
    write(&script, STUBBORN);
    write(
        &wd.join(".hone/mcp.json"),
        &serde_json::json!({ "mcpServers": { "stubborn": {
            "command": "python3",
            "args": [script.to_str().unwrap(), pid_file.to_str().unwrap()]
        } } })
        .to_string(),
    );
    if let Some(p) = permissions {
        write(&wd.join(".hone/permissions.toml"), p);
    }
    write(
        &home.join(".config/hone/config.toml"),
        &format!(
            "default_model = \"openai:dummy-model\"\n[providers.openai]\nbase_url = \"{base_url}\"\napi_key_env = \"HONE_TEST_PROVIDER_VAR\"\n"
        ),
    );
    Setup {
        _tmp: tmp,
        home,
        wd,
        pid_file,
    }
}

/// Resets SIGINT/SIGTERM/SIGHUP to their defaults, then execs hone (same pid),
/// so the signal tests do not depend on how `cargo test` was started (a
/// background job has SIGINT ignored, which hone must then keep ignoring).
const RESET_SIGNALS_AND_EXEC: &str = "import os, signal, sys\n\
    for s in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP):\n\
    \x20   signal.signal(s, signal.SIG_DFL)\n\
    os.execv(sys.argv[1], sys.argv[1:])";

fn spawn_hone(s: &Setup, args: &[&str]) -> Child {
    Command::new("python3")
        .arg("-c")
        .arg(RESET_SIGNALS_AND_EXEC)
        .arg(env!("CARGO_BIN_EXE_hone"))
        .env_clear()
        .env("HOME", &s.home)
        .env("PATH", "/usr/bin:/bin:/usr/local/bin")
        .env("HONE_TEST_PROVIDER_VAR", "placeholder-value")
        .current_dir(&s.wd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hone")
}

fn wait_exit(child: &mut Child, limit: Duration) -> std::process::ExitStatus {
    let start = Instant::now();
    loop {
        if let Some(s) = child.try_wait().unwrap() {
            return s;
        }
        if start.elapsed() > limit {
            child.kill().ok();
            panic!("hone did not exit within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_pid_file(s: &Setup) -> String {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(30) {
        if let Ok(pid) = std::fs::read_to_string(&s.pid_file) {
            if !pid.trim().is_empty() {
                return pid.trim().to_owned();
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("the MCP server never started (no pid file)");
}

fn assert_server_gone_soon(pid: &str, what: &str) {
    for _ in 0..20 {
        if !alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("{what}: the MCP server (pid {pid}) is still running after hone ended");
}

fn stderr_of(child: &mut Child) -> String {
    use std::io::Read;
    let mut text = String::new();
    if let Some(mut e) = child.stderr.take() {
        let _ = e.read_to_string(&mut text);
    }
    text
}

/// Run hone to completion with `args` and check code + server.
fn exit_case(args: &[&str], permissions: Option<&str>, expected_code: i32, label: &str) {
    let s = setup("http://127.0.0.1:9", permissions);
    let mut child = spawn_hone(&s, args);
    let status = wait_exit(&mut child, Duration::from_secs(60));
    let stderr = stderr_of(&mut child);

    let pid = wait_for_pid_file(&s);
    assert_eq!(status.code(), Some(expected_code), "{label}: {stderr}");
    assert_server_gone_soon(&pid, label);
}

#[test]
fn test_p10_unknown_approve_id_should_exit_2_and_stop_the_mcp_server() {
    exit_case(
        &["--approve", "no-such-rule", "hi"],
        None,
        2,
        "unknown --approve id",
    );
}

#[test]
fn test_p10_malformed_project_permissions_should_exit_2_and_stop_the_mcp_server() {
    exit_case(
        &["hi"],
        Some("this is [not toml"),
        2,
        "malformed permissions.toml",
    );
}

#[test]
fn test_p10_ensemble_above_the_limit_should_exit_1_and_stop_the_mcp_server() {
    exit_case(&["--ensemble", "11", "hi"], None, 1, "--ensemble 11");
}

/// A provider that accepts connections and never answers, so a one-shot `hone`
/// is still waiting on it when the signal arrives.
fn silent_provider() -> (
    String,
    std::thread::JoinHandle<()>,
    std::sync::mpsc::Sender<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    let handle = std::thread::spawn(move || {
        let mut held = vec![];
        while stop_rx.try_recv().is_err() {
            if let Ok((stream, _)) = listener.accept() {
                held.push(stream);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    });
    (url, handle, stop_tx)
}

fn signal_case(signal: i32, name: &str) {
    let (url, provider, stop) = silent_provider();
    let s = setup(&url, None);
    let mut child = spawn_hone(&s, &["--no-clarify", "--no-inject", "hi"]);

    let pid = wait_for_pid_file(&s);
    // Let hone get past startup and into the provider request.
    std::thread::sleep(Duration::from_millis(1500));
    assert!(
        child.try_wait().unwrap().is_none(),
        "hone should still be waiting on the provider; stderr: {}",
        stderr_of(&mut child)
    );
    let _ = Command::new("kill")
        .args([format!("-{signal}"), child.id().to_string()])
        .status();

    let status = wait_exit(&mut child, Duration::from_secs(30));
    let _ = stop.send(());
    let _ = provider.join();

    assert_eq!(
        status.signal(),
        Some(signal),
        "{name}: hone must die BY the signal (status {status:?})"
    );
    assert_server_gone_soon(&pid, name);
}

#[test]
fn test_p10_sigterm_to_a_one_shot_run_should_stop_the_mcp_server_and_die_by_sigterm() {
    signal_case(15, "SIGTERM");
}

#[test]
fn test_p10_sighup_to_a_one_shot_run_should_stop_the_mcp_server_and_die_by_sighup() {
    signal_case(1, "SIGHUP");
}

#[test]
fn test_p10_sigint_to_a_one_shot_run_should_stop_the_mcp_server_and_die_by_sigint() {
    signal_case(2, "SIGINT");
}
