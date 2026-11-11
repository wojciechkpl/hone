#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P7 of `docs/plans/mcp-parallel-connect-spec.md`: `hone session resume
//! <missing-name>` shuts its MCP servers down before exiting 1. Today it calls
//! `std::process::exit(1)` after `McpManager::from_config`, so a server that
//! ignores its stdin closing keeps running.
//!
//! Spawns the built `hone` with a cleared environment, a temp HOME and cwd, a
//! project `.hone/mcp.json` that starts a stubborn Python server, and a dummy
//! provider config. Nothing reaches the network: it exits before any request.

use std::path::Path;
use std::process::{Command, Stdio};
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

fn kill_server(pid_file: &Path) {
    if let Ok(pid) = std::fs::read_to_string(pid_file) {
        let _ = Command::new("kill")
            .args(["-9", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status();
    }
}

#[test]
fn test_p7_session_resume_of_a_missing_session_should_shut_mcp_servers_down_before_exit() {
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
    write(
        &home.join(".config/hone/config.toml"),
        "default_model = \"openai:dummy-model\"\n[providers.openai]\nbase_url = \"http://127.0.0.1:9\"\napi_key_env = \"HONE_TEST_PROVIDER_VAR\"\n",
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_hone"))
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin:/usr/local/bin")
        .env("HONE_TEST_PROVIDER_VAR", "placeholder-value")
        .current_dir(&wd)
        .args(["session", "resume", "no-such-session"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hone");

    let started = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if started.elapsed() > Duration::from_secs(60) {
            child.kill().ok();
            kill_server(&pid_file);
            panic!("hone did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    let pid = std::fs::read_to_string(&pid_file).unwrap_or_default();
    let pid = pid.trim().to_owned();
    let mut gone = pid.is_empty();
    for _ in 0..20 {
        if !pid.is_empty() && !alive(&pid) {
            gone = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    kill_server(&pid_file);

    assert_eq!(status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.contains("not found"), "stderr: {stderr}");
    assert!(
        !pid.is_empty(),
        "the MCP server was started (hone reached the resume step); stderr: {stderr}"
    );
    assert!(
        gone,
        "the MCP server (pid {pid}) is still running after hone exited"
    );
}
