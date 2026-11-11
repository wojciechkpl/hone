#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! T8 of `docs/plans/mcp-timeout-spec.md`: lines are read as bytes up to `\n` and
//! decoded afterwards; a line that is not valid UTF-8 (including one cut inside
//! a multi-byte character by a timeout) is skipped like any other non-JSON
//! line. Existing APIs only: real failures.

use std::time::Duration;

use hone_mcp::{McpClient, McpError, StdioMcpClient};
use serde_json::json;

/// Call 1: writes a reply line cut inside the 2-byte "é", finishes it after
/// 600 ms (past the client's 300 ms timeout). Later calls answer at once.
const SPLIT: &str = r#"
import json, sys, time
out = sys.stdout.buffer
n = 0
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    n += 1
    if n == 1:
        body = json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": {"t": "café"}},
                          ensure_ascii=False).encode("utf-8") + b"\n"
        cut = body.index("é".encode("utf-8")) + 1
        out.write(body[:cut]); out.flush()
        time.sleep(0.6)
        out.write(body[cut:]); out.flush()
    else:
        out.write((json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": {"n": n}}) + "\n").encode())
        out.flush()
"#;

/// Before each real reply: a line of invalid UTF-8.
const BAD_BYTES: &str = r#"
import json, sys
out = sys.stdout.buffer
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    out.write(b"log \xff\xfe not utf-8\n")
    out.write((json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": {"ok": True}}) + "\n").encode())
    out.flush()
"#;

async fn spawn(source: &str) -> (StdioMcpClient, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, source).unwrap();
    let client = StdioMcpClient::spawn("python3", &[script.to_str().unwrap()])
        .await
        .unwrap();
    (client, dir)
}

#[tokio::test]
async fn test_t8_timeout_inside_a_multibyte_char_should_not_fail_the_next_call() {
    let (client, _dir) = spawn(SPLIT).await;
    let client = client.with_request_timeout(Duration::from_millis(300));

    let first = tokio::time::timeout(Duration::from_secs(4), client.call_tool("t", json!({})))
        .await
        .expect("hung");
    assert!(matches!(first, Err(McpError::Timeout { .. })), "{first:?}");
    // The rest of line 1 has arrived by now.
    tokio::time::sleep(Duration::from_millis(800)).await;

    let second = tokio::time::timeout(Duration::from_secs(4), client.call_tool("t", json!({})))
        .await
        .expect("hung")
        .unwrap_or_else(|e| panic!("the cut line failed the next call: {e}"));

    assert_eq!(second["n"], 2, "{second}");
}

#[tokio::test]
async fn test_t8_a_line_of_invalid_utf8_before_a_reply_should_be_skipped() {
    let (client, _dir) = spawn(BAD_BYTES).await;

    for _ in 0..2 {
        let out = tokio::time::timeout(Duration::from_secs(4), client.call_tool("t", json!({})))
            .await
            .expect("hung")
            .unwrap_or_else(|e| panic!("invalid UTF-8 failed the call: {e}"));
        assert_eq!(out["ok"], true, "{out}");
    }
}
