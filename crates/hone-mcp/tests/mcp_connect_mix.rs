#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! P2 of `docs/plans/mcp-parallel-connect-spec.md`: nothing else changes. A mix
//! of healthy, failing (command not found, handshake error), timed-out and
//! disabled servers ends with the same live servers and tools; a server that
//! fails or times out is killed. A guard: passes before and after the change.
//! (The log lines are not captured here.)

use std::path::Path;
use std::time::Duration;

use hone_config::mcp::{McpConfig, McpServerConfig};
use hone_mcp::McpManager;

const HEALTHY: &str = r#"
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    if req.get("method") == "tools/list":
        result = {"tools": [{"name": "ok_tool", "description": "d", "inputSchema": {"type": "object"}}]}
    else:
        result = {"serverInfo": {"name": "ok", "version": "1"}}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

/// Answers `initialize` with a JSON-RPC error.
const BAD_HANDSHAKE: &str = r#"
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"],
        "error": {"code": -32000, "message": "nope"}}) + "\n")
    sys.stdout.flush()
"#;

/// Writes its pid, never reads or answers.
const HUNG: &str = r#"
import os, sys, time
open(sys.argv[1], "w").write(str(os.getpid()))
while True:
    time.sleep(1)
"#;

fn server(script: &Path, extra: &[&str], extra_json: serde_json::Value) -> McpServerConfig {
    let mut args = vec![script.to_str().unwrap().to_owned()];
    args.extend(extra.iter().map(|s| (*s).to_owned()));
    let mut value = serde_json::json!({ "command": "python3", "args": args });
    for (k, v) in extra_json.as_object().unwrap() {
        value[k] = v.clone();
    }
    serde_json::from_value(value).unwrap()
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
async fn test_p2_a_mix_of_servers_should_leave_only_the_healthy_one_live_and_kill_the_hung_one() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, src: &str| {
        let p = dir.path().join(name);
        std::fs::write(&p, src).unwrap();
        p
    };
    let healthy = write("healthy.py", HEALTHY);
    let bad = write("bad.py", BAD_HANDSHAKE);
    let hung = write("hung.py", HUNG);
    let pid_file = dir.path().join("hung.pid");

    let mut config = McpConfig::default();
    config
        .mcp_servers
        .insert("ok".into(), server(&healthy, &[], serde_json::json!({})));
    config.mcp_servers.insert(
        "missing".into(),
        serde_json::from_value(serde_json::json!({ "command": "/no/such/command-xyz" })).unwrap(),
    );
    config
        .mcp_servers
        .insert("bad".into(), server(&bad, &[], serde_json::json!({})));
    config.mcp_servers.insert(
        "hung".into(),
        server(
            &hung,
            &[pid_file.to_str().unwrap()],
            serde_json::json!({ "timeout_ms": 300, "handshake_timeout_ms": 300 }),
        ),
    );
    config.mcp_servers.insert(
        "off".into(),
        server(&healthy, &[], serde_json::json!({ "disabled": true })),
    );

    let manager = tokio::time::timeout(Duration::from_secs(8), McpManager::from_config(&config))
        .await
        .expect("startup hung");

    assert_eq!(manager.server_count(), 1, "only `ok` is live");
    let names: Vec<String> = manager
        .tools_as_native()
        .iter()
        .map(|t| t.name().to_owned())
        .collect();
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].contains("ok_tool"), "{names:?}");

    let pid = std::fs::read_to_string(&pid_file).expect("the hung server started");
    let mut gone = false;
    for _ in 0..30 {
        if !alive(pid.trim()) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if !gone {
        let _ = std::process::Command::new("kill")
            .args(["-9", pid.trim()])
            .status();
    }
    assert!(gone, "the timed-out server must be killed");
    manager.shutdown().await;
}
