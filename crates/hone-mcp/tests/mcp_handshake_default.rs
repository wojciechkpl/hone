#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! T7 of `docs/plans/mcp-timeout-spec.md`, the parts that need no new API:
//! `connect()` (initialize + first tools/list) is bounded by a handshake
//! timeout that defaults to the larger of the request timeout and 60 s, so a
//! cold `npx` start is not killed at the request timeout; later requests keep
//! the request timeout. Every test is bounded by an outer timeout.

use std::time::Duration;

use hone_mcp::{McpClient, McpError, StdioMcpClient};
use serde_json::json;

/// `initialize` answers after 600 ms; `tools/call` answers after 600 ms;
/// `tools/list` answers at once.
const SERVER: &str = r#"
import json, sys, time
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    method = req.get("method")
    if method == "tools/list":
        result = {"tools": []}
    elif method == "tools/call":
        time.sleep(0.6)
        result = {"content": [{"type": "text", "text": "called"}]}
    else:
        time.sleep(0.6)
        result = {"serverInfo": {"name": "slow-start", "version": "1"}}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

async fn spawn() -> (StdioMcpClient, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    let client = StdioMcpClient::spawn("python3", &[script.to_str().unwrap()])
        .await
        .unwrap();
    (client, dir)
}

#[tokio::test]
async fn test_t7_a_slow_initialize_should_not_be_cut_at_the_request_timeout() {
    let (client, _dir) = spawn().await;
    let mut client = client.with_request_timeout(Duration::from_millis(300));

    let info = tokio::time::timeout(Duration::from_secs(5), client.connect())
        .await
        .expect("hung")
        .expect("the handshake gets more time than a request");

    assert_eq!(info.name, "slow-start");
}

#[tokio::test]
async fn test_t7_requests_after_the_handshake_should_keep_the_request_timeout() {
    let (client, _dir) = spawn().await;
    let mut client = client.with_request_timeout(Duration::from_millis(300));
    tokio::time::timeout(Duration::from_secs(5), client.connect())
        .await
        .expect("hung")
        .expect("connects");

    let err = tokio::time::timeout(Duration::from_secs(5), client.call_tool("t", json!({})))
        .await
        .expect("hung")
        .expect_err("a 600 ms call exceeds the 300 ms request timeout");

    assert!(
        matches!(
            err,
            McpError::Timeout {
                timeout_ms: 300,
                ..
            }
        ),
        "{err:?}"
    );
}

#[tokio::test]
async fn test_t7_zero_request_timeout_should_mean_no_handshake_timeout_either() {
    let (client, _dir) = spawn().await;
    let mut client = client.with_request_timeout(Duration::ZERO);

    let info = tokio::time::timeout(Duration::from_secs(5), client.connect())
        .await
        .expect("hung")
        .unwrap();

    assert_eq!(info.name, "slow-start");
}
