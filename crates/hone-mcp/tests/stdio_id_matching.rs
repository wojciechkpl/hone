#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W50 of `docs/plans/worker-tools-spec.md`: the stdio MCP client matches each
//! response to its request by JSON-RPC `id` and skips any other line, so a
//! call dropped mid-flight (Esc, worker timeout) cannot shift later results.
//! The servers are small self-contained Python scripts in a temp dir.

use std::time::Duration;

use hone_mcp::{McpClient, StdioMcpClient};

/// Echoes `result-for-<tag>`; a `slow` call sleeps 0.5 s first. Responds to
/// requests only (lines with an `id`).
const ECHO_SERVER: &str = r#"
import json, sys, time
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    if req.get("method") == "tools/call":
        args = req["params"]["arguments"]
        if args.get("slow"):
            time.sleep(0.5)
        result = {"content": [{"type": "text", "text": "result-for-" + args["tag"]}]}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    sys.stdout.flush()
"#;

/// Before each real response it writes a notification (no `id`), a response
/// with a foreign id, and a blank line.
const NOISY_SERVER: &str = r#"
import json, sys
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    out = sys.stdout
    out.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/message",
                          "params": {"level": "info", "data": "hello"}}) + "\n")
    out.write(json.dumps({"jsonrpc": "2.0", "id": 987654,
                          "result": {"content": [{"type": "text", "text": "FOREIGN"}]}}) + "\n")
    out.write("\n")
    if req.get("method") == "tools/call":
        args = req["params"]["arguments"]
        result = {"content": [{"type": "text", "text": "result-for-" + args["tag"]}]}
    else:
        result = {}
    out.write(json.dumps({"jsonrpc": "2.0", "id": req["id"], "result": result}) + "\n")
    out.flush()
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

fn text_of(v: &serde_json::Value) -> String {
    v.to_string()
}

#[tokio::test]
async fn test_w50_call_after_a_dropped_call_should_get_its_own_result() {
    let (client, _dir) = client_for(ECHO_SERVER).await;

    // Call A is slow; drop it after 100 ms (request sent, response not read).
    let a = client.call_tool("t", serde_json::json!({ "tag": "A", "slow": true }));
    let dropped = tokio::time::timeout(Duration::from_millis(100), a)
        .await
        .is_err();
    assert!(dropped, "call A must still be in flight when dropped");

    let b = client
        .call_tool("t", serde_json::json!({ "tag": "B" }))
        .await
        .unwrap();
    let c = client
        .call_tool("t", serde_json::json!({ "tag": "C" }))
        .await
        .unwrap();

    assert!(text_of(&b).contains("result-for-B"), "B got {b}");
    assert!(text_of(&c).contains("result-for-C"), "C got {c}");
}

#[tokio::test]
async fn test_w50_several_dropped_calls_in_a_row_should_not_shift_later_results() {
    let (client, _dir) = client_for(ECHO_SERVER).await;
    for tag in ["A1", "A2"] {
        let call = client.call_tool("t", serde_json::json!({ "tag": tag, "slow": true }));
        let _ = tokio::time::timeout(Duration::from_millis(50), call).await;
    }

    let b = client
        .call_tool("t", serde_json::json!({ "tag": "B" }))
        .await
        .unwrap();

    assert!(text_of(&b).contains("result-for-B"), "B got {b}");
}

#[tokio::test]
async fn test_w50_notifications_and_foreign_ids_before_the_response_should_be_skipped() {
    let (client, _dir) = client_for(NOISY_SERVER).await;

    let first = client
        .call_tool("t", serde_json::json!({ "tag": "ONE" }))
        .await
        .unwrap();
    let second = client
        .call_tool("t", serde_json::json!({ "tag": "TWO" }))
        .await
        .unwrap();

    assert!(text_of(&first).contains("result-for-ONE"), "got {first}");
    assert!(text_of(&second).contains("result-for-TWO"), "got {second}");
    assert!(!text_of(&first).contains("FOREIGN"));
}

#[tokio::test]
async fn test_w50_normal_sequential_calls_should_still_work() {
    let (client, _dir) = client_for(ECHO_SERVER).await;
    for tag in ["X", "Y", "Z"] {
        let r = client
            .call_tool("t", serde_json::json!({ "tag": tag }))
            .await
            .unwrap();
        assert!(text_of(&r).contains(&format!("result-for-{tag}")), "{r}");
    }
}
