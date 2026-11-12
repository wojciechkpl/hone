#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! P1 of `docs/plans/mcp-parallel-connect-spec.md`: servers connect at the
//! same time. Deterministic barrier: each of three servers writes a marker file
//! when it receives `initialize`, and answers only once all three markers
//! exist. Sequential connect never reaches the barrier (the first server waits
//! for servers that have not been started), so the outer timeout fails it.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hone_config::mcp::{McpConfig, McpServerConfig};
use hone_mcp::McpManager;

/// argv: marker_dir, name, total, pid_file.
const BARRIER_SERVER: &str = r#"
import json, os, sys, time
marker_dir, name, total, pid_file = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
open(pid_file, "w").write(str(os.getpid()))

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    method = req.get("method")
    if method == "initialize":
        open(os.path.join(marker_dir, name + ".marker"), "w").write("x")
        deadline = time.time() + 20
        while time.time() < deadline:
            if len([f for f in os.listdir(marker_dir) if f.endswith(".marker")]) >= total:
                break
            time.sleep(0.02)
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"serverInfo": {"name": name, "version": "1"}}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"tools": [
            {"name": name + "_tool", "description": "d", "inputSchema": {"type": "object"}}]}})
    else:
        send({"jsonrpc": "2.0", "id": req["id"], "result": {}})
"#;

/// Kills every pid recorded in `*.pid` files of the directory when dropped, so
/// a failing test leaves no server behind.
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

fn server(script: &Path, args: &[&str], timeout_ms: u64) -> McpServerConfig {
    let mut all = vec![script.to_str().unwrap().to_owned()];
    all.extend(args.iter().map(|s| (*s).to_owned()));
    serde_json::from_value(serde_json::json!({
        "command": "python3", "args": all, "timeout_ms": timeout_ms,
    }))
    .unwrap()
}

#[tokio::test]
async fn test_p1_servers_should_complete_their_handshakes_at_the_same_time() {
    let dir = tempfile::tempdir().unwrap();
    let _cleanup = Cleanup(dir.path().to_path_buf());
    let script = dir.path().join("barrier.py");
    std::fs::write(&script, BARRIER_SERVER).unwrap();
    let mut config = McpConfig::default();
    for name in ["a", "b", "c"] {
        let pid_file = dir.path().join(format!("{name}.pid"));
        config.mcp_servers.insert(
            name.to_owned(),
            server(
                &script,
                &[
                    dir.path().to_str().unwrap(),
                    name,
                    "3",
                    pid_file.to_str().unwrap(),
                ],
                60_000,
            ),
        );
    }

    let manager = tokio::time::timeout(Duration::from_secs(6), McpManager::from_config(&config))
        .await
        .expect(
            "startup did not reach the barrier: the handshakes are not in flight at the same time",
        );

    assert_eq!(manager.server_count(), 3);
    let mut names: Vec<String> = manager
        .tools_as_native()
        .iter()
        .map(|t| t.name().to_owned())
        .collect();
    names.sort();
    assert_eq!(names.len(), 3, "{names:?}");
    for n in ["a", "b", "c"] {
        assert!(
            names.iter().any(|t| t.contains(&format!("{n}_tool"))),
            "tool of server {n} missing: {names:?}"
        );
    }
    manager.shutdown().await;
}
