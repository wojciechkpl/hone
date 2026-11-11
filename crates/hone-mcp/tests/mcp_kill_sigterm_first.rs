#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
#![cfg(unix)]
//! P13 of `docs/plans/mcp-parallel-connect-spec.md`: `kill_running_commands()`
//! stops MCP groups with SIGTERM first (those still alive about 1 s later get
//! SIGKILL) and returns once every MCP group is gone; shell-command groups are
//! still SIGKILLed at once. The registry is process-wide, so the tests of this
//! binary are serialised.

use std::path::Path;
use std::time::{Duration, Instant};

use hone_config::mcp::McpConfig;
use hone_mcp::McpManager;

static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Handshakes, then on SIGTERM writes `<dir>/term-marker` and exits (when
/// argv[2] is `polite`) or ignores SIGTERM (`stubborn`). Ignores its input
/// closing either way. argv[1] = directory.
const SERVER: &str = r#"
import json, os, signal, sys, time
d, mode = sys.argv[1], sys.argv[2]
open(os.path.join(d, "server.pid"), "w").write(str(os.getpid()))
if mode == "polite":
    def bye(*a):
        open(os.path.join(d, "term-marker"), "w").write("x")
        sys.exit(0)
    signal.signal(signal.SIGTERM, bye)
else:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
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

async fn connected(dir: &Path, mode: &str) -> (McpManager, String) {
    let script = dir.join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    let config: McpConfig = serde_json::from_value(serde_json::json!({
        "mcpServers": { "s": {
            "command": "python3",
            "args": [script.to_str().unwrap(), dir.to_str().unwrap(), mode]
        } }
    }))
    .unwrap();
    let manager = tokio::time::timeout(Duration::from_secs(10), McpManager::from_config(&config))
        .await
        .expect("startup hung");
    assert_eq!(manager.server_count(), 1);
    let pid = std::fs::read_to_string(dir.join("server.pid"))
        .unwrap()
        .trim()
        .to_owned();
    (manager, pid)
}

fn force_kill(pid: &str) {
    let _ = std::process::Command::new("kill")
        .args(["-9", pid])
        .stderr(std::process::Stdio::null())
        .status();
}

#[tokio::test]
async fn test_p13_a_server_that_handles_sigterm_should_get_to_clean_up_before_it_is_gone() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (manager, pid) = connected(dir.path(), "polite").await;

    hone_tools::kill_running_commands();

    let marker = dir.path().join("term-marker").exists();
    let gone = !alive(&pid);
    force_kill(&pid);
    assert!(marker, "the server was SIGTERMed first and could clean up");
    assert!(gone, "the server is gone when the call returns");
    drop(manager);
}

#[tokio::test]
async fn test_p13_a_server_that_ignores_sigterm_should_be_gone_after_the_call_within_two_seconds() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let (manager, pid) = connected(dir.path(), "stubborn").await;

    let start = Instant::now();
    hone_tools::kill_running_commands();
    let took = start.elapsed();

    let gone = !alive(&pid);
    force_kill(&pid);
    assert!(
        gone,
        "SIGKILL follows for a server still alive after SIGTERM"
    );
    assert!(took < Duration::from_secs(2), "returned after {took:?}");
    drop(manager);
}

#[tokio::test]
async fn test_p13_a_shell_command_group_should_still_be_killed_at_once() {
    let _g = SERIAL.lock().await;
    let mut cmd = std::process::Command::new("sleep");
    cmd.arg("30");
    let running = tokio::spawn(hone_tools::run_command(cmd, None));
    tokio::time::sleep(Duration::from_millis(300)).await;

    let start = Instant::now();
    hone_tools::kill_running_commands();
    let took = start.elapsed();

    assert!(
        took < Duration::from_millis(500),
        "no MCP group lingers, so the call must not wait ({took:?})"
    );
    tokio::time::timeout(Duration::from_secs(2), running)
        .await
        .expect("the command group is killed")
        .unwrap()
        .ok();
}
