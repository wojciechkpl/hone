#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! P3 of `docs/plans/mcp-parallel-connect-spec.md`: cancelling startup leaves
//! nothing behind. Dropping the `from_config` future mid-handshake kills every
//! server it started; none keeps running or connecting in the background. A
//! guard against a naive parallel version that spawns detached tasks: it
//! passes on today's sequential code (kill-on-drop) and must keep passing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use hone_config::mcp::{McpConfig, McpServerConfig};
use hone_mcp::McpManager;

/// Writes its pid, then answers `initialize` only after 20 s.
const SLOW_START: &str = r#"
import json, os, sys, time
open(sys.argv[1], "w").write(str(os.getpid()))
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    time.sleep(20)
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": {}}) + "\n")
    sys.stdout.flush()
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
                            .status();
                    }
                }
            }
        }
    }
}

fn server(script: &Path, pid_file: &Path) -> McpServerConfig {
    serde_json::from_value(serde_json::json!({
        "command": "python3",
        "args": [script.to_str().unwrap(), pid_file.to_str().unwrap()],
        "timeout_ms": 60_000,
    }))
    .unwrap()
}

fn alive(pid: &str) -> bool {
    let out = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", pid])
        .output()
        .unwrap();
    let stat = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    !stat.is_empty() && !stat.starts_with('Z')
}

#[tokio::test]
async fn test_p3_dropping_startup_mid_handshake_should_kill_every_started_server() {
    let dir = tempfile::tempdir().unwrap();
    let _cleanup = Cleanup(dir.path().to_path_buf());
    let script = dir.path().join("slow.py");
    std::fs::write(&script, SLOW_START).unwrap();
    let mut config = McpConfig::default();
    for name in ["a", "b", "c"] {
        let pid_file = dir.path().join(format!("{name}.pid"));
        config
            .mcp_servers
            .insert(name.to_owned(), server(&script, &pid_file));
    }

    // Cancel startup 1.5 s in: every server's handshake is still waiting.
    let result = tokio::time::timeout(
        Duration::from_millis(1500),
        McpManager::from_config(&config),
    )
    .await;
    assert!(result.is_err(), "the servers answer only after 20 s");

    let pids: Vec<String> = ["a", "b", "c"]
        .iter()
        .filter_map(|n| std::fs::read_to_string(dir.path().join(format!("{n}.pid"))).ok())
        .map(|s| s.trim().to_owned())
        .collect();
    assert!(!pids.is_empty(), "at least one server was started");
    for _ in 0..20 {
        if pids.iter().all(|p| !alive(p)) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let still: Vec<&String> = pids.iter().filter(|p| alive(p)).collect();
    panic!("servers still running after startup was cancelled: {still:?}");
}
