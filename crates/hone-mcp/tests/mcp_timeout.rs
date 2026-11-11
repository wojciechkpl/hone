#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! T1-T3 of `docs/plans/mcp-timeout-spec.md`: the stdio MCP client's request
//! timeout. New API assumed (this file fails to compile until it exists):
//! `StdioMcpClient::with_request_timeout(self, Duration) -> Self`;
//! `Duration::ZERO` means no timeout. Every test is also bounded by an outer
//! timeout so a hang fails instead of stalling the suite.

use std::time::{Duration, Instant};

use hone_mcp::{McpClient, McpError, StdioMcpClient};
use serde_json::json;

/// Modes (argv[1]): `silent` never answers `tools/call`, `tools/list` or
/// `initialize`; `late` answers the first `tools/call` 450 ms late and later
/// calls at once; `slow` answers every request after 500 ms.
const SERVER: &str = r#"
import json, sys, time
mode = sys.argv[1]
first_call = True

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
    if mode == "silent":
        continue
    if mode == "slow":
        time.sleep(0.5)
    if method == "tools/call":
        tag = req["params"]["arguments"].get("tag", "?")
        if mode == "late" and first_call:
            first_call = False
            time.sleep(0.45)
        send({"jsonrpc": "2.0", "id": req["id"],
              "result": {"content": [{"type": "text", "text": "result-for-" + tag}]}})
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"tools": []}})
    else:
        send({"jsonrpc": "2.0", "id": req["id"], "result": {"serverInfo": {"name": "s", "version": "1"}}})
"#;

async fn spawn(mode: &str) -> (StdioMcpClient, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    let client = StdioMcpClient::spawn("python3", &[script.to_str().unwrap(), mode])
        .await
        .unwrap();
    (client, dir)
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

async fn bounded<T>(f: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(4), f)
        .await
        .expect("hung: the request timeout did not fire")
}

fn assert_timeout(err: McpError) {
    match err {
        McpError::Timeout { timeout_ms, .. } => assert_eq!(timeout_ms, 300),
        other => panic!("expected McpError::Timeout, got {other:?}"),
    }
}

#[tokio::test]
async fn test_t1_call_tool_to_a_silent_server_should_time_out() {
    let (client, _dir) = spawn("silent").await;
    let client = client.with_request_timeout(ms(300));

    let start = Instant::now();
    let err = bounded(client.call_tool("t", json!({ "tag": "A" })))
        .await
        .expect_err("no answer");

    assert_timeout(err);
    assert!(start.elapsed() < Duration::from_secs(2), "promptly");
}

#[tokio::test]
async fn test_t1_list_tools_to_a_silent_server_should_time_out() {
    let (client, _dir) = spawn("silent").await;
    let client = client.with_request_timeout(ms(300));

    let err = bounded(client.list_tools()).await.expect_err("no answer");

    assert_timeout(err);
}

#[tokio::test]
async fn test_t1_connect_to_a_silent_server_should_time_out() {
    let (client, _dir) = spawn("silent").await;
    let mut client = client.with_request_timeout(ms(300));

    let err = bounded(client.connect()).await.expect_err("no handshake");

    assert_timeout(err);
}

#[tokio::test]
async fn test_t2_a_slow_answer_within_the_timeout_should_still_be_returned() {
    let (client, _dir) = spawn("slow").await;
    let client = client.with_request_timeout(Duration::from_secs(3));

    let out = bounded(client.call_tool("t", json!({ "tag": "S" })))
        .await
        .unwrap();

    assert!(out.to_string().contains("result-for-S"), "{out}");
}

#[tokio::test]
async fn test_t2_zero_timeout_should_mean_no_timeout() {
    let (client, _dir) = spawn("slow").await;
    let client = client.with_request_timeout(Duration::ZERO);

    let out = bounded(client.call_tool("t", json!({ "tag": "Z" })))
        .await
        .unwrap();

    assert!(out.to_string().contains("result-for-Z"), "{out}");
}

#[tokio::test]
async fn test_t3_call_after_a_timed_out_call_should_get_its_own_result() {
    let (client, _dir) = spawn("late").await;
    let client = client.with_request_timeout(ms(300));

    // A times out at 300 ms; the server answers it late, at 450 ms, and then
    // answers B at once. B (sent at ~300 ms, deadline ~600 ms) therefore has
    // ~150 ms of margin: the late answer to A arrives while B waits.
    let a = bounded(client.call_tool("t", json!({ "tag": "A" }))).await;
    assert_timeout(a.expect_err("A times out"));

    // B must get B's result, not A's late one.
    let b = bounded(client.call_tool("t", json!({ "tag": "B" })))
        .await
        .unwrap();

    assert!(b.to_string().contains("result-for-B"), "B got {b}");
}
