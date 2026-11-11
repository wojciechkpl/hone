#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! T9 of `docs/plans/mcp-timeout-spec.md`: shutdown closes the server's stdin
//! before waiting, so a server that exits when its input ends is gone at once;
//! the 5 s wait and kill remain the fallback. Existing APIs only.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use hone_config::mcp::{McpConfig, McpServerConfig};
use hone_mcp::McpManager;

/// Writes its pid, answers everything, exits when stdin closes.
const POLITE: &str = r#"
import json, os, sys
open(sys.argv[1], "w").write(str(os.getpid()))
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    result = {"tools": []} if req.get("method") == "tools/list" else {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

/// Answers the handshake, then ignores end of input: only a kill ends it.
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

async fn manager_for(source: &str) -> (McpManager, String, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, source).unwrap();
    let pid_file = dir.path().join("pid");
    let mut servers = HashMap::new();
    // Built from JSON so the test does not depend on the struct's field list.
    let server: McpServerConfig = serde_json::from_value(serde_json::json!({
        "command": "python3",
        "args": [script.to_str().unwrap(), pid_file.to_str().unwrap()],
        "timeout_ms": 10_000,
    }))
    .unwrap();
    servers.insert("s".to_owned(), server);
    let manager = McpManager::from_config(&McpConfig {
        mcp_servers: servers,
    })
    .await;
    assert_eq!(manager.server_count(), 1);
    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .to_owned();
    (manager, pid, dir)
}

/// Alive = the pid exists and is not a zombie.
fn alive(pid: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

#[tokio::test]
async fn test_t9_shutdown_of_a_server_that_exits_on_end_of_input_should_be_quick() {
    let (manager, pid, _dir) = manager_for(POLITE).await;

    let start = Instant::now();
    tokio::time::timeout(Duration::from_secs(8), manager.shutdown())
        .await
        .expect("shutdown hung");

    assert!(
        start.elapsed() < Duration::from_millis(1500),
        "shutdown took {:?}: it waited for the 5 s fallback",
        start.elapsed()
    );
    assert!(!alive(&pid), "the server process is gone");
}

/// The fallback: a server that ignores the end of input is still killed. This
/// waits out the 5 s grace period, so it takes about 5 s.
#[tokio::test]
async fn test_t9_shutdown_should_still_kill_a_server_that_ignores_end_of_input() {
    let (manager, pid, _dir) = manager_for(STUBBORN).await;

    tokio::time::timeout(Duration::from_secs(10), manager.shutdown())
        .await
        .expect("shutdown hung");

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
            .status();
    }
    assert!(gone, "the stubborn server must be killed by the fallback");
}
