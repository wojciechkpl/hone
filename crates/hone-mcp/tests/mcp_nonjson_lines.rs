#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! T4 of `docs/plans/mcp-timeout-spec.md` (plus a T2 guard): a line on the
//! server's stdout that is not JSON is skipped instead of failing the call.
//! Uses only existing APIs, so the reds are real failures.

use std::time::Duration;

use hone_mcp::{McpClient, StdioMcpClient};
use serde_json::json;

/// Before each real response: a plain log line, then the tail of a JSON object
/// (what is left of a line whose read was dropped).
const SERVER: &str = r#"
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    out = sys.stdout
    out.write("INFO starting to handle a request\n")
    out.write('"result":{"x":1}}\n')
    tag = req.get("params", {}).get("arguments", {}).get("tag", "?")
    out.write(json.dumps({"jsonrpc": "2.0", "id": req["id"],
        "result": {"content": [{"type": "text", "text": "result-for-" + tag}]}}) + "\n")
    out.flush()
"#;

/// Answers after 500 ms; the raw constructor must not time out on its own.
const SLOW_SERVER: &str = r#"
import json, sys, time
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    time.sleep(0.5)
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"],
        "result": {"content": [{"type": "text", "text": "late-but-fine"}]}}) + "\n")
    sys.stdout.flush()
"#;

async fn client_for(source: &str) -> (StdioMcpClient, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, source).unwrap();
    let client = StdioMcpClient::spawn("python3", &[script.to_str().unwrap()])
        .await
        .unwrap();
    (client, dir)
}

#[tokio::test]
async fn test_t4_non_json_lines_before_a_response_should_be_skipped() {
    let (client, _dir) = client_for(SERVER).await;

    for tag in ["ONE", "TWO"] {
        let out = tokio::time::timeout(
            Duration::from_secs(4),
            client.call_tool("t", json!({ "tag": tag })),
        )
        .await
        .expect("hung")
        .unwrap_or_else(|e| panic!("a log line failed the call: {e}"));
        assert!(
            out.to_string().contains(&format!("result-for-{tag}")),
            "{out}"
        );
    }
}

#[tokio::test]
async fn test_t2_a_client_built_directly_should_have_no_timeout_of_its_own() {
    let (client, _dir) = client_for(SLOW_SERVER).await;

    let out = tokio::time::timeout(Duration::from_secs(4), client.call_tool("t", json!({})))
        .await
        .expect("hung")
        .unwrap();

    assert!(out.to_string().contains("late-but-fine"), "{out}");
}
