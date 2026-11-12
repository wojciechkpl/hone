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

// ---------------------------------------------------------------------------
// P14 / P15 and pinning tests
// ---------------------------------------------------------------------------

/// Handshakes; ignores end of input. mode `polite`: on SIGTERM "cleans up" for
/// argv[3] seconds, writes `<dir>/term-marker` and exits; `stubborn` ignores
/// SIGTERM. argv: dir, mode, delay.
const DELAY_SERVER: &str = r#"
import json, os, signal, sys, time
d, mode, delay = sys.argv[1], sys.argv[2], float(sys.argv[3])
open(os.path.join(d, "server.pid"), "w").write(str(os.getpid()))
if mode == "polite":
    def bye(*a):
        time.sleep(delay)
        open(os.path.join(d, "term-marker"), "w").write("x")
        os._exit(0)
    signal.signal(signal.SIGTERM, bye)
elif mode == "stubborn":
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

async fn start_with(dir: &Path, command: &str, args: Vec<String>) -> (McpManager, String) {
    let config: McpConfig = serde_json::from_value(serde_json::json!({
        "mcpServers": { "s": { "command": command, "args": args } }
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

fn direct_args(dir: &Path, script: &Path, mode: &str, delay: &str) -> Vec<String> {
    vec![
        script.display().to_string(),
        dir.display().to_string(),
        mode.to_owned(),
        delay.to_owned(),
    ]
}

/// P14 / L1: through a shell launcher that does not exec the server, the server
/// is a grandchild. The grace must last while any group member is alive, so a
/// server that needs 300 ms for its SIGTERM cleanup still gets it after the
/// shell itself has died. The trailing `; true` makes sure no shell (dash, bash
/// 3.2) execs the last command by itself. Runs on macOS and on Linux (L1 of
/// `docs/plans/linux-launcher-grace-spec.md`).
#[tokio::test]
async fn test_p14_sigterm_grace_should_last_through_a_shell_launcher_that_does_not_exec() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, DELAY_SERVER).unwrap();
    let launcher = format!(
        "cd {} && python3 {} {} polite 0.3; true",
        dir.path().display(),
        script.display(),
        dir.path().display()
    );
    let (manager, pid) = start_with(dir.path(), "sh", vec!["-c".into(), launcher]).await;

    hone_tools::kill_running_commands();

    let marker = dir.path().join("term-marker").exists();
    let gone = !alive(&pid);
    force_kill(&pid);
    assert!(
        marker,
        "the server must finish its 300 ms SIGTERM cleanup although its shell launcher died first"
    );
    assert!(gone, "the server is gone when the call returns");
    drop(manager);
}

/// P15: a shell command started while `kill_running_commands` waits on a
/// stubborn MCP server (up to ~1 s) must be killed too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_p15_a_command_started_during_the_mcp_wait_should_not_outlive_the_call() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, DELAY_SERVER).unwrap();
    let (manager, server_pid) = start_with(
        dir.path(),
        "python3",
        direct_args(dir.path(), &script, "stubborn", "0"),
    )
    .await;

    let killer = std::thread::spawn(|| {
        hone_tools::kill_running_commands();
    });
    // ~200 ms into the call (it waits about 1 s for the stubborn server).
    tokio::time::sleep(Duration::from_millis(200)).await;
    let cmd_pid_file = dir.path().join("cmd.pid");
    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c").arg(format!(
        "echo $$ > {}; exec sleep 30",
        cmd_pid_file.display()
    ));
    let running = tokio::spawn(hone_tools::run_command(cmd, None));
    tokio::time::sleep(Duration::from_millis(200)).await;

    killer.join().unwrap();

    let cmd_pid = std::fs::read_to_string(&cmd_pid_file)
        .expect("the command started during the wait")
        .trim()
        .to_owned();
    let mut gone = false;
    for _ in 0..10 {
        if !alive(&cmd_pid) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    force_kill(&cmd_pid);
    force_kill(&server_pid);
    running.abort();
    assert!(
        gone,
        "the command started during the MCP wait (pid {cmd_pid}) outlived kill_running_commands()"
    );
    drop(manager);
}

/// L2 / pins the zombie rule (Linux) and the EPERM rule (macOS): a direct server
/// that exits on SIGTERM makes the call return at once, not after the grace.
#[tokio::test]
async fn test_p14_a_direct_server_that_exits_on_sigterm_should_end_the_call_quickly() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, DELAY_SERVER).unwrap();
    let (manager, pid) = start_with(
        dir.path(),
        "python3",
        direct_args(dir.path(), &script, "polite", "0"),
    )
    .await;

    let start = Instant::now();
    hone_tools::kill_running_commands();
    let took = start.elapsed();

    let marker = dir.path().join("term-marker").exists();
    force_kill(&pid);
    assert!(marker, "it cleaned up on SIGTERM");
    assert!(
        took < Duration::from_millis(500),
        "an exited server must end the wait at once ({took:?})"
    );
    drop(manager);
}

/// Pins a grace well above 100 ms: a direct server whose SIGTERM cleanup takes
/// ~300 ms finishes it.
#[tokio::test]
async fn test_p13_a_direct_server_whose_sigterm_cleanup_takes_300ms_should_finish_it() {
    let _g = SERIAL.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, DELAY_SERVER).unwrap();
    let (manager, pid) = start_with(
        dir.path(),
        "python3",
        direct_args(dir.path(), &script, "polite", "0.3"),
    )
    .await;

    hone_tools::kill_running_commands();

    let marker = dir.path().join("term-marker").exists();
    let gone = !alive(&pid);
    force_kill(&pid);
    assert!(marker, "the 300 ms cleanup completed before SIGKILL");
    assert!(gone);
    drop(manager);
}
