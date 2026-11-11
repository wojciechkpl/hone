#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! P12 of `docs/plans/mcp-parallel-connect-spec.md`: a TUI exit shuts MCP
//! servers down gracefully. The TUI's exit guard kills only shell-command
//! groups, through a new `hone_tools::kill_shell_commands()`; MCP servers are
//! left to `McpManager::shutdown()`.
//!
//! New API (the expected red is the compile error until it exists):
//! `hone_tools::kill_shell_commands()`, synchronous.
//!
//! Own file: the registry is process-wide.

use std::time::{Duration, Instant};

use hone_config::mcp::McpConfig;
use hone_mcp::McpManager;

/// A polite server with a child "browser" (R16). On end of input it asks the
/// child to close (SIGTERM), waits for it, writes `server-clean` and exits; the
/// child writes `browser-clean` when closed gracefully. argv[1] = directory.
const POLITE: &str = r#"
import json, os, subprocess, sys
d = sys.argv[1]
open(os.path.join(d, "server.pid"), "w").write(str(os.getpid()))
browser = subprocess.Popen([sys.executable, "-c", (
    "import signal, sys, time\n"
    "def bye(*a):\n"
    "    open(sys.argv[1] + '/browser-clean', 'w').write('x'); sys.exit(0)\n"
    "signal.signal(signal.SIGTERM, bye)\n"
    "while True: time.sleep(1)\n"), d])
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
browser.terminate(); browser.wait()
open(os.path.join(d, "server-clean"), "w").write("x")
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
async fn test_p12_kill_shell_commands_should_kill_a_command_group_and_leave_the_mcp_server_to_shut_down_gracefully()
 {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("polite.py");
    std::fs::write(&script, POLITE).unwrap();
    let config: McpConfig = serde_json::from_value(serde_json::json!({
        "mcpServers": { "polite": {
            "command": "python3",
            "args": [script.to_str().unwrap(), dir.path().to_str().unwrap()]
        } }
    }))
    .unwrap();
    let manager = tokio::time::timeout(Duration::from_secs(10), McpManager::from_config(&config))
        .await
        .expect("startup hung");
    assert_eq!(manager.server_count(), 1);
    let server_pid = std::fs::read_to_string(dir.path().join("server.pid"))
        .unwrap()
        .trim()
        .to_owned();
    tokio::time::sleep(Duration::from_millis(300)).await; // the child "browser" is up

    // A running shell command (its own process group, in the registry).
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("30");
    let running = tokio::spawn(hone_tools::run_command(cmd, None));
    tokio::time::sleep(Duration::from_millis(300)).await;

    // What the end of `run_tui` does.
    hone_tools::kill_shell_commands();

    let started = Instant::now();
    let ended = tokio::time::timeout(Duration::from_secs(3), running).await;
    assert!(
        ended.is_ok(),
        "the shell command group must be killed (waited {:?})",
        started.elapsed()
    );
    assert!(
        alive(&server_pid),
        "the MCP server must be left alive for its graceful shutdown"
    );

    // main's graceful shutdown.
    tokio::time::timeout(Duration::from_secs(10), manager.shutdown())
        .await
        .expect("shutdown hung");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        dir.path().join("server-clean").exists(),
        "the server cleaned up when its input ended"
    );
    assert!(
        dir.path().join("browser-clean").exists(),
        "the server's child was closed gracefully"
    );
    let _ = std::process::Command::new("kill")
        .args(["-9", &server_pid])
        .stderr(std::process::Stdio::null())
        .status();
}
