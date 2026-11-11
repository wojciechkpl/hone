#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! P11 of `docs/plans/mcp-parallel-connect-spec.md`: killing a server kills what
//! it started. The server is launched through `sh -c "python3 <script> <pid>;
//! true"`; the trailing `; true` stops `sh` from exec'ing python, so python is a
//! grandchild. When hone kills the server (failed start, drop, exhausted
//! shutdown budget) the whole process group must go.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hone_config::mcp::McpConfig;
use hone_mcp::{McpClient, McpManager, StdioMcpClient};

/// Writes its pid, never answers anything.
const HUNG: &str = r#"
import os, sys, time
open(sys.argv[1], "w").write(str(os.getpid()))
while True:
    time.sleep(1)
"#;

/// Answers the handshake, then ignores the end of its input.
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

struct Fixture {
    _dir: tempfile::TempDir,
    launcher_cmd: String,
    pid_file: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(&self.pid_file) {
            let _ = std::process::Command::new("kill")
                .args(["-9", pid.trim()])
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

fn fixture(source: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    let pid_file = dir.path().join("server.pid");
    std::fs::write(&script, source).unwrap();
    let launcher_cmd = format!("python3 {} {}; true", script.display(), pid_file.display());
    Fixture {
        _dir: dir,
        launcher_cmd,
        pid_file,
    }
}

fn alive(pid: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

/// The python pid; waits for the file (python starts a moment after `sh`).
async fn python_pid(pid_file: &Path) -> String {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if let Ok(pid) = std::fs::read_to_string(pid_file) {
            if !pid.trim().is_empty() {
                return pid.trim().to_owned();
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the server never started (no pid file)");
}

async fn assert_gone_soon(pid: &str, what: &str) {
    for _ in 0..20 {
        if !alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("{what}: the real server process (pid {pid}) is still running");
}

#[tokio::test]
async fn test_p11_a_hung_handshake_through_a_launcher_should_not_leave_the_real_server() {
    let f = fixture(HUNG);
    let config: McpConfig = serde_json::from_value(serde_json::json!({
        "mcpServers": { "hung": {
            "command": "sh",
            "args": ["-c", f.launcher_cmd],
            "timeout_ms": 300,
            "handshake_timeout_ms": 300
        } }
    }))
    .unwrap();

    let manager = tokio::time::timeout(Duration::from_secs(8), McpManager::from_config(&config))
        .await
        .expect("startup hung");

    assert_eq!(manager.server_count(), 0);
    let pid = python_pid(&f.pid_file).await;
    assert_gone_soon(&pid, "after the timed-out start").await;
}

#[tokio::test]
async fn test_p11_dropping_a_connected_client_started_through_a_launcher_should_kill_the_real_server()
 {
    let f = fixture(STUBBORN);
    let mut client =
        StdioMcpClient::spawn_with_name("launched", "sh", &["-c", f.launcher_cmd.as_str()], &[])
            .await
            .unwrap();
    tokio::time::timeout(Duration::from_secs(10), client.connect())
        .await
        .expect("handshake hung")
        .expect("connects");
    let pid = python_pid(&f.pid_file).await;
    assert!(alive(&pid), "the server runs while the client is alive");

    drop(client);

    assert_gone_soon(&pid, "after dropping the client").await;
}

/// Waits out the 5 s shutdown budget (the server ignores its input closing).
#[tokio::test]
async fn test_p11_exhausted_shutdown_budget_should_kill_the_launched_servers_whole_group() {
    let f = fixture(STUBBORN);
    let config: McpConfig = serde_json::from_value(serde_json::json!({
        "mcpServers": { "launched": {
            "command": "sh",
            "args": ["-c", f.launcher_cmd],
            "timeout_ms": 10_000
        } }
    }))
    .unwrap();
    let manager = tokio::time::timeout(Duration::from_secs(10), McpManager::from_config(&config))
        .await
        .expect("startup hung");
    assert_eq!(manager.server_count(), 1);
    let pid = python_pid(&f.pid_file).await;

    tokio::time::timeout(Duration::from_secs(12), manager.shutdown())
        .await
        .expect("shutdown hung");

    assert_gone_soon(&pid, "after shutdown").await;
}
