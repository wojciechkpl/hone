#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! P4 and P5 of `docs/plans/mcp-parallel-connect-spec.md`: shutdown of several
//! servers is concurrent, and a server that stopped reading cannot stall exit.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use hone_config::mcp::{McpConfig, McpServerConfig};
use hone_mcp::{McpClient, McpManager, StdioMcpClient};

/// Answers the handshake, then ignores the end of its input (only a kill ends
/// it). argv[1] = pid file.
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

/// Answers `initialize` and `tools/list`, then stops reading stdin for good.
const STOPS_READING: &str = r#"
import json, os, sys, time
open(sys.argv[1], "w").write(str(os.getpid()))
done = False
while not done:
    req = json.loads(sys.stdin.readline())
    if "id" not in req:
        continue
    result = {"tools": []} if req.get("method") == "tools/list" else {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
    done = req.get("method") == "tools/list"
while True:
    time.sleep(60)
"#;

struct Cleanup(PathBuf);

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Ok(entries) = std::fs::read_dir(&self.0) {
            for e in entries.flatten() {
                if e.path().extension().is_some_and(|x| x == "pid") {
                    if let Ok(pid) = std::fs::read_to_string(e.path()) {
                        let _ = std::process::Command::new("kill")
                            .args(["-9", pid.trim()])
                            .stderr(std::process::Stdio::null())
                            .status();
                    }
                }
            }
        }
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

async fn assert_gone(pid_file: &Path) {
    let pid = std::fs::read_to_string(pid_file).unwrap().trim().to_owned();
    for _ in 0..20 {
        if !alive(&pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("process {pid} is still running");
}

fn server(script: &Path, pid_file: &Path) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({
        "command": "python3",
        "args": [script.to_str().unwrap(), pid_file.to_str().unwrap()],
        "timeout_ms": 10_000,
    }))
    .unwrap()
}

#[tokio::test]
async fn test_p4_shutdown_of_two_servers_ignoring_input_close_should_take_one_budget_not_two() {
    let dir = tempfile::tempdir().unwrap();
    let _cleanup = Cleanup(dir.path().to_path_buf());
    let script = dir.path().join("stubborn.py");
    std::fs::write(&script, STUBBORN).unwrap();
    let mut config = McpConfig::default();
    let mut pid_files = vec![];
    for name in ["a", "b"] {
        let pid_file = dir.path().join(format!("{name}.pid"));
        config
            .mcp_servers
            .insert(name.to_owned(), server(&script, &pid_file));
        pid_files.push(pid_file);
    }
    let manager = tokio::time::timeout(Duration::from_secs(10), McpManager::from_config(&config))
        .await
        .expect("startup hung");
    assert_eq!(manager.server_count(), 2);

    let start = Instant::now();
    tokio::time::timeout(Duration::from_secs(15), manager.shutdown())
        .await
        .expect("shutdown hung");

    assert!(
        start.elapsed() < Duration::from_secs(8),
        "shutdown took {:?}: the servers were shut down one after another",
        start.elapsed()
    );
    for pid_file in &pid_files {
        assert_gone(pid_file).await;
    }
}

#[tokio::test]
async fn test_p5_shutdown_of_a_server_that_stopped_reading_should_not_stall() {
    let dir = tempfile::tempdir().unwrap();
    let _cleanup = Cleanup(dir.path().to_path_buf());
    let script = dir.path().join("stops.py");
    std::fs::write(&script, STOPS_READING).unwrap();
    let pid_file = dir.path().join("s.pid");
    let mut client = StdioMcpClient::spawn(
        "python3",
        &[script.to_str().unwrap(), pid_file.to_str().unwrap()],
    )
    .await
    .unwrap()
    .with_request_timeout(Duration::from_millis(500));
    tokio::time::timeout(Duration::from_secs(5), client.connect())
        .await
        .expect("handshake hung")
        .expect("the server answers the handshake");

    // A ~1 MB argument fills the server's input pipe; the write times out midway.
    let big = "x".repeat(1_000_000);
    let call = tokio::time::timeout(
        Duration::from_secs(5),
        client.call_tool("t", serde_json::json!({ "data": big })),
    )
    .await
    .expect("the call itself must time out, not hang");
    assert!(call.is_err(), "the server never answers");

    let start = Instant::now();
    let shutdown = tokio::time::timeout(Duration::from_secs(8), client.shutdown()).await;
    assert!(
        shutdown.is_ok(),
        "shutdown blocked on a server that stopped reading (no time limit on the notification write)"
    );
    assert!(start.elapsed() < Duration::from_secs(8));
    assert_gone(&pid_file).await;
}
