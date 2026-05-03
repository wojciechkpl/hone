use std::sync::Arc;

use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

use crate::server::{AcpServer, AcpServerError, DispatchOutcome};
use crate::types::{JsonRpcError, JsonRpcResponse};

/// Drive an [`AcpServer`] over arbitrary `AsyncRead + AsyncWrite` streams (test
/// pipes or real stdin/stdout).  Reads newline-delimited JSON requests,
/// dispatches each one, and writes replies and intermediate notifications.
/// Returns `Ok(())` on EOF.
pub async fn serve<R, W>(
    server: Arc<AcpServer>,
    read: R,
    write: W,
) -> Result<(), AcpServerError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(read);
    let writer = Arc::new(Mutex::new(write));
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

        match serde_json::from_str::<crate::types::JsonRpcRequest>(trimmed) {
            Ok(req) => {
                let outcome = server.dispatch(req).await;
                match outcome {
                    DispatchOutcome::Reply(resp) => {
                        write_json_line(&writer, &resp).await?;
                    }
                    DispatchOutcome::Streamed {
                        mut updates_rx,
                        completion_rx,
                    } => {
                        let writer2 = Arc::clone(&writer);
                        tokio::spawn(async move {
                            while let Some(notif) = updates_rx.recv().await {
                                if let Err(e) = write_json_line(&writer2, &notif).await {
                                    tracing::warn!("write notification failed: {e}");
                                    return;
                                }
                            }
                            match completion_rx.await {
                                Ok(resp) => {
                                    if let Err(e) = write_json_line(&writer2, &resp).await {
                                        tracing::warn!("write completion failed: {e}");
                                    }
                                }
                                Err(_) => tracing::warn!("completion channel closed"),
                            }
                        });
                    }
                    DispatchOutcome::Silent => {}
                }
            }
            Err(e) => {
                let resp = JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id: serde_json::Value::Null,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32700,
                        message: format!("parse error: {e}"),
                    }),
                };
                write_json_line(&writer, &resp).await?;
            }
        }
    }

    Ok(())
}

async fn write_json_line<W, T>(
    writer: &Arc<Mutex<W>>,
    value: &T,
) -> Result<(), AcpServerError>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let mut s = serde_json::to_string(value)?;
    s.push('\n');
    let mut guard = writer.lock().await;
    guard.write_all(s.as_bytes()).await?;
    guard.flush().await?;
    Ok(())
}

/// Convenience wrapper that binds `serve` to real stdin/stdout.
pub async fn serve_stdio(server: Arc<AcpServer>) -> Result<(), AcpServerError> {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    serve(server, stdin, stdout).await
}


// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::MockHandler;
    use crate::server::AcpServer;
    use crate::types::{MessageContent, UpdatePayload};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    fn make_server_with_updates(updates: Vec<UpdatePayload>) -> Arc<AcpServer> {
        Arc::new(AcpServer::new(Arc::new(MockHandler::new(updates, "end_turn"))))
    }

    /// Write a JSON value as a newline-delimited line to `writer`.
    async fn client_write(writer: &mut (impl AsyncWriteExt + Unpin), value: serde_json::Value) {
        let mut s = serde_json::to_string(&value).unwrap();
        s.push('\n');
        writer.write_all(s.as_bytes()).await.unwrap();
    }

    /// Read the next non-empty line from `reader` and parse it as JSON.
    async fn client_read(reader: &mut (impl AsyncBufReadExt + Unpin)) -> serde_json::Value {
        let mut line = String::new();
        loop {
            line.clear();
            let n = reader.read_line(&mut line).await.unwrap();
            assert!(n > 0, "unexpected EOF from server");
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                return serde_json::from_str(trimmed).expect("server sent valid JSON");
            }
        }
    }

    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn stdio_serve_handles_initialize_session_new_then_prompt() {
        let chunk = UpdatePayload::MessageChunk {
            content: MessageContent {
                content_type: "text".to_owned(),
                text: "hi".to_owned(),
            },
        };
        let server = make_server_with_updates(vec![chunk]);

        // duplex creates an in-memory pipe pair.
        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server_stream);
        let (client_rx, mut client_write_half) = tokio::io::split(client_stream);
        let mut client_reader = BufReader::new(client_rx);

        // Spawn the server task.
        tokio::spawn(serve(server, server_read, server_write));

        // 1. initialize
        client_write(
            &mut client_write_half,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": 1, "clientInfo": { "name": "test", "version": "0" } }
            }),
        )
        .await;
        let init_resp = client_read(&mut client_reader).await;
        assert_eq!(init_resp["result"]["agentInfo"]["name"], "hone");

        // 2. session/new
        client_write(
            &mut client_write_half,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "session/new",
                "params": { "cwd": "/tmp" }
            }),
        )
        .await;
        let new_resp = client_read(&mut client_reader).await;
        let session_id = new_resp["result"]["sessionId"]
            .as_str()
            .expect("sessionId must be string")
            .to_owned();

        // 3. session/prompt
        client_write(
            &mut client_write_half,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "session/prompt",
                "params": {
                    "sessionId": session_id,
                    "prompt": [{ "type": "text", "text": "go" }],
                    "tools": []
                }
            }),
        )
        .await;

        // Expect: at least one session/update notification, then the final prompt reply.
        let msg1 = client_read(&mut client_reader).await;
        // Could be a notification OR the final reply depending on ordering.
        // Collect until we see the prompt response (id == 3).
        let mut notifications = vec![];
        let mut final_resp_opt: Option<serde_json::Value> = None;

        let mut current = msg1;
        loop {
            if current.get("method").map(|m| m == "session/update").unwrap_or(false) {
                notifications.push(current);
                current = client_read(&mut client_reader).await;
            } else {
                final_resp_opt = Some(current);
                break;
            }
        }

        // Assertions: ordering — notifications before reply.
        assert!(
            !notifications.is_empty(),
            "must receive at least one session/update notification"
        );
        let final_resp = final_resp_opt.expect("final prompt reply must arrive");
        assert_eq!(final_resp["id"], 3);
        assert_eq!(final_resp["result"]["stopReason"], "end_turn");
    }

    #[tokio::test]
    async fn stdio_serve_returns_clean_on_eof() {
        let server = make_server_with_updates(vec![]);

        // Use a pair of duplex streams: one for the server, one for the client.
        // We keep them unsplit so that dropping the client DuplexStream sends
        // EOF to the server's read half (the two halves of a split() share an
        // Arc, so dropping only the write half does NOT produce EOF).
        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server_stream);

        // Wrap client_stream in a BufWriter/BufReader pair without splitting —
        // we can't do that directly, so use a fresh duplex pair as the test pipe.
        // Pipe layout:
        //   test_writer -> pipe -> client_read
        //   client_stream -> server (which also writes replies back)
        //
        // Simpler: write via client_stream write half then drop the WHOLE
        // client_stream after reading the reply to trigger EOF.
        let (client_rx, mut client_tx) = tokio::io::split(client_stream);
        let mut client_reader = BufReader::new(client_rx);

        let serve_handle = tokio::spawn(serve(server, server_read, server_write));

        // Send initialize, read the response.
        client_write(
            &mut client_tx,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": 1, "clientInfo": { "name": "t", "version": "0" } }
            }),
        )
        .await;
        let _ = client_read(&mut client_reader).await;

        // Shutdown the write side explicitly — this sends a FIN to the server,
        // which will surface as EOF on the server's read half.
        client_tx.shutdown().await.unwrap();

        let result = serve_handle.await.expect("serve task did not panic");
        assert!(result.is_ok(), "serve must return Ok on EOF");
    }

    #[tokio::test]
    async fn stdio_serve_returns_parse_error_for_malformed_json() {
        let server = make_server_with_updates(vec![]);

        let (client_stream, server_stream) = tokio::io::duplex(4096);
        let (server_read, server_write) = tokio::io::split(server_stream);
        let (client_rx, mut client_write_half) = tokio::io::split(client_stream);
        let mut client_reader = BufReader::new(client_rx);

        tokio::spawn(serve(server, server_read, server_write));

        // 1. Send garbage.
        client_write_half
            .write_all(b"garbage json!!!\n")
            .await
            .unwrap();

        // Expect a parse-error response with null id.
        let err_resp = client_read(&mut client_reader).await;
        assert_eq!(err_resp["error"]["code"], -32700, "must be parse error");
        assert!(err_resp["id"].is_null(), "id must be null for parse errors");

        // 2. Send a valid initialize — must still work.
        client_write(
            &mut client_write_half,
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": 1, "clientInfo": { "name": "t", "version": "0" } }
            }),
        )
        .await;

        let ok_resp = client_read(&mut client_reader).await;
        assert!(ok_resp["error"].is_null(), "second request must succeed");
        assert_eq!(ok_resp["result"]["agentInfo"]["name"], "hone");
    }
}
