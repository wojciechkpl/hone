#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W51 of `docs/plans/worker-tools-spec.md`: the stdio MCP client never waits
//! forever on a reply it skipped. An `id: null` error is the answer to the
//! outstanding request; a request from the server is answered (`ping` with `{}`,
//! anything else with -32601) and skipped; ids compare as JSON values, so a
//! string `"1"` is not our numeric `1`. Every test is bounded by an outer
//! timeout so a hang fails instead of stalling the suite.

use std::time::Duration;

use hone_mcp::{McpClient, StdioMcpClient};
use serde_json::{Value, json};

/// `argv[1]` = mode, `argv[2]` = file where the server stores the reply it got
/// to its own request. Non-`tools/call` requests are answered plainly.
const SERVER: &str = r#"
import json, sys
mode, reply_path = sys.argv[1], sys.argv[2]

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()

def ask(request):
    send(request)
    reply = sys.stdin.readline()
    open(reply_path, "w").write(reply)

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    req = json.loads(line)
    if "id" not in req:
        continue
    ours = req["id"]
    if req.get("method") != "tools/call":
        send({"jsonrpc": "2.0", "id": ours, "result": {}})
        continue
    ok = {"content": [{"type": "text", "text": "RIGHT"}]}
    if mode == "null-id-error":
        send({"jsonrpc": "2.0", "id": None,
              "error": {"code": -32600, "message": "Invalid Request"}})
        continue
    if mode == "roots":
        ask({"jsonrpc": "2.0", "id": 9001, "method": "roots/list"})
    elif mode == "ping":
        ask({"jsonrpc": "2.0", "id": 9001, "method": "ping"})
    elif mode == "string-id-request":
        ask({"jsonrpc": "2.0", "id": "srv-1", "method": "roots/list"})
    elif mode == "string-echo":
        # A response whose id is our id AS A STRING: not ours.
        send({"jsonrpc": "2.0", "id": str(ours),
              "result": {"content": [{"type": "text", "text": "WRONG"}]}})
    send({"jsonrpc": "2.0", "id": ours, "result": ok})
"#;

struct Outcome {
    result: Result<Value, hone_mcp::McpError>,
    reply: Option<Value>,
    _dir: tempfile::TempDir,
}

async fn run(mode: &str) -> Outcome {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("server.py");
    std::fs::write(&script, SERVER).unwrap();
    let reply_path = dir.path().join("reply.json");
    let client = tokio::time::timeout(
        Duration::from_secs(5),
        StdioMcpClient::spawn(
            "python3",
            &[script.to_str().unwrap(), mode, reply_path.to_str().unwrap()],
        ),
    )
    .await
    .expect("spawn")
    .unwrap();

    let result = tokio::time::timeout(Duration::from_secs(3), client.call_tool("t", json!({})))
        .await
        .unwrap_or_else(|_| panic!("mode {mode}: the call hung (no reply was ever matched)"));

    let reply = std::fs::read_to_string(&reply_path)
        .ok()
        .map(|s| serde_json::from_str(s.trim()).unwrap());
    Outcome {
        result,
        reply,
        _dir: dir,
    }
}

#[tokio::test]
async fn test_w51_null_id_error_should_be_returned_as_the_calls_error() {
    let out = run("null-id-error").await;

    let err = out.result.expect_err("the server answered with an error");
    assert!(err.to_string().contains("Invalid Request"), "{err}");
}

#[tokio::test]
async fn test_w51_server_request_should_get_method_not_found_and_our_call_should_complete() {
    let out = run("roots").await;

    let value = out.result.unwrap();
    assert!(value.to_string().contains("RIGHT"), "{value}");
    let reply = out.reply.expect("the server got a reply to its request");
    assert_eq!(reply["id"], 9001);
    assert_eq!(reply["error"]["code"], -32601, "{reply}");
}

#[tokio::test]
async fn test_w51_server_ping_should_be_answered_with_an_empty_result() {
    let out = run("ping").await;

    let value = out.result.unwrap();
    assert!(value.to_string().contains("RIGHT"), "{value}");
    let reply = out.reply.expect("the server got a reply to its ping");
    assert_eq!(reply["id"], 9001);
    assert_eq!(reply["result"], json!({}), "{reply}");
}

#[tokio::test]
async fn test_w51_server_request_with_a_string_id_should_be_answered_with_that_id() {
    let out = run("string-id-request").await;

    let value = out.result.unwrap();
    assert!(value.to_string().contains("RIGHT"), "{value}");
    let reply = out.reply.expect("the server got a reply");
    assert_eq!(
        reply["id"], "srv-1",
        "the id is echoed as the same JSON value"
    );
    assert_eq!(reply["error"]["code"], -32601);
}

#[tokio::test]
async fn test_w51_response_with_our_id_as_a_string_should_be_skipped() {
    let out = run("string-echo").await;

    let value = out.result.unwrap();
    assert!(value.to_string().contains("RIGHT"), "{value}");
    assert!(!value.to_string().contains("WRONG"), "{value}");
}
