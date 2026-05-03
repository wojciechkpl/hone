use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::jsonrpc::{JsonRpcError, JsonRpcId, JsonRpcRequest, JsonRpcResponse};
use crate::server::{McpServer, McpServerError};

/// Drive an `McpServer` over an arbitrary AsyncRead + AsyncWrite pair.
///
/// Reads newline-delimited JSON requests, dispatches to the server, writes
/// newline-delimited JSON responses. Returns `Ok(())` on clean EOF. Malformed
/// JSON for a single line produces a parse-error response (with id = null) and
/// processing continues with the next line.
pub async fn serve<R, W>(server: Arc<McpServer>, read: R, mut write: W) -> Result<(), McpServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // EOF
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<JsonRpcRequest>(trimmed) {
            Ok(req) => server.handle_request(req).await,
            Err(e) => Some(JsonRpcResponse::err(
                JsonRpcId::Null,
                JsonRpcError::parse_error(e.to_string()),
            )),
        };

        if let Some(resp) = response {
            let mut s = serde_json::to_string(&resp)?;
            s.push('\n');
            write.write_all(s.as_bytes()).await?;
            write.flush().await?;
        }
    }
    Ok(())
}

/// Convenience: wire to actual stdin/stdout.
pub async fn serve_stdio(server: Arc<McpServer>) -> Result<(), McpServerError> {
    serve(server, tokio::io::stdin(), tokio::io::stdout()).await
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use hone_proto::ToolLayer;
    use hone_sandbox::NullSandbox;
    use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::Arc;

    // -----------------------------------------------------------------------
    // Minimal mock tool for integration tests
    // -----------------------------------------------------------------------

    struct MockTool;

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str { "mock" }
        fn description(&self) -> &str { "A mock tool for testing." }
        fn schema(&self) -> Value {
            json!({ "type": "object", "properties": {} })
        }
        fn layer(&self) -> ToolLayer { ToolLayer::Native }
        async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::new(json!({ "pong": true })))
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_server() -> Arc<McpServer> {
        let registry = ToolRegistry::builder()
            .register(Box::new(MockTool))
            .build();
        let ctx = ToolContext {
            sandbox: Arc::new(NullSandbox),
            working_dir: PathBuf::from("/tmp"),
        };
        Arc::new(McpServer::new(registry, ctx))
    }

    fn encode(v: &Value) -> Vec<u8> {
        let mut b = serde_json::to_vec(v).unwrap();
        b.push(b'\n');
        b
    }

    // -----------------------------------------------------------------------
    // Tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn stdio_serve_handles_initialize_then_tools_list() {
        let server = make_server();

        // Build the input the "client" would send.
        let mut input = Vec::<u8>::new();
        input.extend(encode(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }
        })));
        input.extend(encode(&json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        })));
        input.extend(encode(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        })));

        let mut output = Vec::<u8>::new();
        serve(server, input.as_slice(), &mut output).await.unwrap();

        // Parse all newline-delimited responses.
        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        // initialize response + tools/list response = 2 (notification has no response)
        assert_eq!(responses.len(), 2, "expected exactly 2 responses");

        let init_resp = &responses[0];
        assert_eq!(init_resp["id"], 1);
        assert!(init_resp["result"]["serverInfo"]["name"].as_str().is_some());

        let list_resp = &responses[1];
        assert_eq!(list_resp["id"], 2);
        let tools = list_resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], "mock");
    }

    #[tokio::test]
    async fn stdio_serve_returns_parse_error_for_malformed_json_then_continues() {
        let server = make_server();

        let mut input = Vec::<u8>::new();
        input.extend(b"not json\n");
        input.extend(encode(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }
        })));

        let mut output = Vec::<u8>::new();
        serve(server, input.as_slice(), &mut output).await.unwrap();

        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();

        assert_eq!(responses.len(), 2, "expected parse-error response + initialize response");

        let parse_err = &responses[0];
        assert!(parse_err.get("error").is_some(), "first response must be an error");
        assert_eq!(
            parse_err["error"]["code"],
            JsonRpcError::PARSE_ERROR,
            "must use parse error code"
        );
        // Per JSON-RPC spec, when request id is unknown/unparseable, id must be null.
        assert!(
            parse_err["id"].is_null(),
            "parse error response id must be null"
        );

        let init_resp = &responses[1];
        assert!(init_resp.get("result").is_some(), "second response must be a result");
    }

    #[tokio::test]
    async fn stdio_serve_returns_clean_on_eof() {
        let server = make_server();

        let mut input = Vec::<u8>::new();
        input.extend(encode(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": { "name": "test", "version": "0" }
            }
        })));
        // No explicit EOF marker — the slice ends, which is EOF.

        let mut output = Vec::<u8>::new();
        let result = serve(server, input.as_slice(), &mut output).await;

        assert!(result.is_ok(), "serve must return Ok(()) on clean EOF, got: {:?}", result);

        let responses: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(responses.len(), 1, "should have one response for the initialize request");
    }
}
