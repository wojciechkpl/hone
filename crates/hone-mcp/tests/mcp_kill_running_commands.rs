#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! P10/P11 of `docs/plans/mcp-parallel-connect-spec.md`: every MCP server runs
//! in its own process group registered with the running-process registry that
//! shell commands use, so `hone_tools::kill_running_commands()` (called by the
//! signal handlers and before each `process::exit`) kills connected MCP
//! servers too. Own file: the registry is process-wide and this test kills
//! everything registered in its binary.

use std::time::{Duration, Instant};

use hone_config::mcp::McpConfig;
use hone_mcp::McpManager;

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

fn alive(pid: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

#[tokio::test]
async fn test_p10_kill_running_commands_should_kill_connected_mcp_servers_and_what_they_started() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    let pid_file = dir.path().join("server.pid");
    std::fs::write(&script, STUBBORN).unwrap();
    // Through a launcher, so python is a grandchild of `sh` (P11 too).
    let launcher = format!("python3 {} {}; true", script.display(), pid_file.display());
    let config: McpConfig = serde_json::from_value(serde_json::json!({
        "mcpServers": { "launched": {
            "command": "sh", "args": ["-c", launcher], "timeout_ms": 10_000
        } }
    }))
    .unwrap();
    let manager = tokio::time::timeout(Duration::from_secs(10), McpManager::from_config(&config))
        .await
        .expect("startup hung");
    assert_eq!(manager.server_count(), 1);
    let start = Instant::now();
    let pid = loop {
        if let Ok(p) = std::fs::read_to_string(&pid_file) {
            if !p.trim().is_empty() {
                break p.trim().to_owned();
            }
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no pid file");
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert!(alive(&pid));

    hone_tools::kill_running_commands();

    let mut gone = false;
    for _ in 0..20 {
        if !alive(&pid) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !gone {
        let _ = std::process::Command::new("kill")
            .args(["-9", &pid])
            .stderr(std::process::Stdio::null())
            .status();
    }
    assert!(
        gone,
        "kill_running_commands() must reach the MCP server's process group"
    );
    drop(manager);
}
