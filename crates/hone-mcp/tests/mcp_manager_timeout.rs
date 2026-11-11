#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! T5 of `docs/plans/mcp-timeout-spec.md`: a server whose `initialize` never
//! answers (within its `timeout_ms`) is skipped at startup and its process is
//! killed; the healthy server still connects. Existing APIs only.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use hone_config::mcp::{McpConfig, McpServerConfig};
use hone_mcp::McpManager;

/// Writes its pid to argv[1], then never reads stdin or answers (so closing
/// its stdin does not end it: only a kill does).
const HUNG_SERVER: &str = r#"
import os, sys, time
open(sys.argv[1], "w").write(str(os.getpid()))
while True:
    time.sleep(1)
"#;

const HEALTHY_SERVER: &str = r#"
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    if req.get("method") == "tools/list":
        result = {"tools": []}
    else:
        result = {"serverInfo": {"name": "healthy", "version": "1"}}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

fn server(script: &std::path::Path, extra: &[&str], timeout_ms: u64) -> McpServerConfig {
    let mut args = vec![script.to_str().unwrap().to_owned()];
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    McpServerConfig {
        command: "python3".to_owned(),
        args,
        env: HashMap::new(),
        disabled: false,
        timeout_ms,
    }
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
async fn test_t5_hung_initialize_should_be_skipped_and_its_process_killed() {
    let dir = tempfile::tempdir().unwrap();
    let hung = dir.path().join("hung.py");
    let healthy = dir.path().join("healthy.py");
    std::fs::write(&hung, HUNG_SERVER).unwrap();
    std::fs::write(&healthy, HEALTHY_SERVER).unwrap();
    let pid_file = dir.path().join("hung.pid");

    let mut config = McpConfig::default();
    config.mcp_servers.insert(
        "hung".to_owned(),
        server(&hung, &[pid_file.to_str().unwrap()], 300),
    );
    config
        .mcp_servers
        .insert("healthy".to_owned(), server(&healthy, &[], 5000));

    let start = Instant::now();
    let manager = tokio::time::timeout(Duration::from_secs(4), McpManager::from_config(&config))
        .await
        .expect("startup must not hang on a server that never answers initialize");

    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(manager.server_count(), 1, "only the healthy server is live");

    let pid = std::fs::read_to_string(&pid_file).expect("the hung server started");
    let mut gone = false;
    for _ in 0..30 {
        if !alive(pid.trim()) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Clean up in any case so a failure does not leak the process.
    if !gone {
        let _ = std::process::Command::new("kill")
            .args(["-9", pid.trim()])
            .status();
    }
    assert!(
        gone,
        "the hung server's process must be killed, not left running"
    );
    manager.shutdown().await;
}
