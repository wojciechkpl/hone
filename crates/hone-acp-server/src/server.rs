use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};

use crate::handler::AcpHandler;
use crate::types::{JsonRpcError, JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

// The imports above are referenced by DispatchOutcome (mpsc::Receiver,
// oneshot::Receiver) and by the tests.  They are NOT used by the stub
// dispatch() body, so allow dead_code in the prod path.
#[allow(dead_code)]
const _IMPORTS_USED_BY_TESTS_AND_OUTCOME: () = ();

// ---------------------------------------------------------------------------
// AgentInfoOverride
// ---------------------------------------------------------------------------

/// Override the default agent identification surfaced via `initialize`.
pub struct AgentInfoOverride {
    pub name: String,
    pub version: String,
}

impl Default for AgentInfoOverride {
    fn default() -> Self {
        Self {
            name: "hone".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

// ---------------------------------------------------------------------------
// DispatchOutcome
// ---------------------------------------------------------------------------

/// Outcome of dispatching one inbound JSON-RPC envelope.
pub enum DispatchOutcome {
    /// A single response to send back synchronously.
    Reply(JsonRpcResponse),
    /// For `session/prompt`: the server streams notifications then sends a
    /// final response.  The receiver yields zero or more
    /// [`JsonRpcNotification`]s followed (on channel close) by a single
    /// [`JsonRpcResponse`] from `completion_rx`.
    Streamed {
        updates_rx: mpsc::Receiver<JsonRpcNotification>,
        completion_rx: oneshot::Receiver<JsonRpcResponse>,
    },
    /// Incoming notification — no reply needed.
    Silent,
}

// ---------------------------------------------------------------------------
// AcpServer
// ---------------------------------------------------------------------------

pub struct AcpServer {
    handler: Arc<dyn AcpHandler>,
    info: AgentInfoOverride,
    initialized: AtomicBool,
    /// Active sessions: session_id → cwd.
    sessions: Mutex<HashMap<String, String>>,
}

impl AcpServer {
    pub fn new(handler: Arc<dyn AcpHandler>) -> Self {
        Self {
            handler,
            info: AgentInfoOverride::default(),
            initialized: AtomicBool::new(false),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn with_agent_info(mut self, info: AgentInfoOverride) -> Self {
        self.info = info;
        self
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }

    /// Process one inbound JSON-RPC envelope.
    pub async fn dispatch(&self, request: JsonRpcRequest) -> DispatchOutcome {
        // Notifications (id == null) get no reply.
        if matches!(request.id, Value::Null) {
            return DispatchOutcome::Silent;
        }

        // Validate JSON-RPC version.
        if request.jsonrpc != "2.0" {
            return DispatchOutcome::Reply(error_response(
                request.id,
                -32600,
                "invalid request: jsonrpc must be \"2.0\"",
            ));
        }

        match request.method.as_str() {
            "initialize" => {
                self.initialized.store(true, Ordering::Relaxed);
                let result = serde_json::json!({
                    "protocolVersion": 1,
                    "agentInfo": {
                        "name": self.info.name,
                        "version": self.info.version,
                    }
                });
                DispatchOutcome::Reply(ok_response(request.id, result))
            }

            "session/new" => {
                let cwd: String = match parse_params::<serde_json::Value>(request.params) {
                    Ok(v) => v
                        .get("cwd")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_owned(),
                    Err(e) => {
                        return DispatchOutcome::Reply(error_response(request.id, -32602, &e));
                    }
                };
                let session_id = uuid::Uuid::now_v7().to_string();
                self.sessions
                    .lock()
                    .await
                    .insert(session_id.clone(), cwd);
                let result = serde_json::json!({ "sessionId": session_id });
                DispatchOutcome::Reply(ok_response(request.id, result))
            }

            "session/prompt" => {
                let params: crate::types::PromptParams =
                    match parse_params(request.params) {
                        Ok(p) => p,
                        Err(e) => {
                            return DispatchOutcome::Reply(error_response(
                                request.id,
                                -32602,
                                &e,
                            ));
                        }
                    };

                // Verify session exists.
                {
                    let guard = self.sessions.lock().await;
                    if !guard.contains_key(&params.session_id) {
                        return DispatchOutcome::Reply(error_response(
                            request.id,
                            -32602,
                            "unknown session id",
                        ));
                    }
                }

                let (notif_tx, notif_rx) = mpsc::channel::<JsonRpcNotification>(64);
                let (completion_tx, completion_rx) = oneshot::channel::<JsonRpcResponse>();

                let handler = Arc::clone(&self.handler);
                let session_id = params.session_id.clone();
                let prompt = params.prompt;
                let tools = params.tools;
                let req_id = request.id;

                tokio::spawn(async move {
                    // Channel for raw UpdatePayload from the handler.
                    let (update_tx, mut update_rx) = mpsc::channel::<crate::types::UpdatePayload>(64);

                    // Spawn the handler.
                    let handler_fut = tokio::spawn({
                        let handler = Arc::clone(&handler);
                        let session_id = session_id.clone();
                        async move {
                            handler
                                .handle_prompt(session_id, prompt, tools, update_tx)
                                .await
                        }
                    });

                    // Drain updates from handler, forwarding as notifications.
                    while let Some(payload) = update_rx.recv().await {
                        let notif = JsonRpcNotification {
                            jsonrpc: "2.0".into(),
                            method: "session/update".into(),
                            params: Some(serde_json::json!({
                                "sessionId": session_id,
                                "update": payload,
                            })),
                        };
                        if notif_tx.send(notif).await.is_err() {
                            break;
                        }
                    }
                    // notif_tx dropped here → updates_rx will return None.

                    // Send final completion.
                    let resp = match handler_fut.await {
                        Ok(Ok(stop_reason)) => JsonRpcResponse {
                            jsonrpc: "2.0".into(),
                            id: req_id,
                            result: Some(serde_json::json!({ "stopReason": stop_reason })),
                            error: None,
                        },
                        Ok(Err(e)) => error_response(req_id, -32603, &e.to_string()),
                        Err(e) => error_response(req_id, -32603, &e.to_string()),
                    };
                    let _ = completion_tx.send(resp);
                });

                DispatchOutcome::Streamed {
                    updates_rx: notif_rx,
                    completion_rx,
                }
            }

            _ => DispatchOutcome::Reply(error_response(
                request.id,
                -32601,
                "method not found",
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AcpServerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn ok_response(id: Value, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_owned(),
        id,
        result: Some(result),
        error: None,
    }
}

fn error_response(id: Value, code: i32, message: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_owned(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_owned(),
        }),
    }
}

fn parse_params<T: serde::de::DeserializeOwned>(params: Option<Value>) -> Result<T, String> {
    let v = params.unwrap_or(Value::Null);
    serde_json::from_value(v).map_err(|e| format!("invalid params: {e}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::MockHandler;
    use crate::types::{MessageContent, UpdatePayload};

    fn make_server() -> Arc<AcpServer> {
        Arc::new(AcpServer::new(Arc::new(MockHandler::new(vec![], "end_turn"))))
    }

    fn make_request(method: &str, params: Option<Value>) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id: Value::Number(1.into()),
            method: method.to_owned(),
            params,
        }
    }

    fn init_request() -> JsonRpcRequest {
        make_request(
            "initialize",
            Some(serde_json::json!({
                "protocolVersion": 1,
                "clientInfo": { "name": "test", "version": "0.0.1" }
            })),
        )
    }

    async fn session_new_id(server: &AcpServer) -> String {
        let req = make_request(
            "session/new",
            Some(serde_json::json!({ "cwd": "/tmp" })),
        );
        let outcome = server.dispatch(req).await;
        match outcome {
            DispatchOutcome::Reply(resp) => resp
                .result
                .unwrap()
                .get("sessionId")
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned(),
            _ => panic!("expected Reply"),
        }
    }

    // -----------------------------------------------------------------------
    // initialize
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatch_initialize_returns_agent_info() {
        let server = make_server();
        let outcome = server.dispatch(init_request()).await;

        let resp = match outcome {
            DispatchOutcome::Reply(r) => r,
            _ => panic!("expected Reply for initialize"),
        };

        assert!(resp.error.is_none(), "expected no error");
        let result = resp.result.expect("result must be present");
        assert_eq!(result["agentInfo"]["name"], "hone");
        assert_eq!(result["protocolVersion"], 1);
    }

    #[tokio::test]
    async fn dispatch_initialize_marks_server_initialized() {
        let server = make_server();
        assert!(!server.is_initialized());
        server.dispatch(init_request()).await;
        assert!(server.is_initialized());
    }

    // -----------------------------------------------------------------------
    // session/new
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatch_session_new_creates_unique_session_id() {
        let server = make_server();
        let id1 = session_new_id(&server).await;
        let id2 = session_new_id(&server).await;
        assert_ne!(id1, id2, "session ids must be unique");
    }

    #[tokio::test]
    async fn dispatch_session_new_session_id_is_uuid_v7() {
        let server = make_server();
        let id = session_new_id(&server).await;
        let parsed = uuid::Uuid::parse_str(&id).expect("session id must parse as UUID");
        assert_eq!(
            parsed.get_version(),
            Some(uuid::Version::SortRand),
            "session id must be UUIDv7"
        );
    }

    // -----------------------------------------------------------------------
    // session/prompt
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatch_session_prompt_returns_streamed_outcome() {
        let server = make_server();
        let sid = session_new_id(&server).await;

        let req = make_request(
            "session/prompt",
            Some(serde_json::json!({
                "sessionId": sid,
                "prompt": [{ "type": "text", "text": "hello" }],
                "tools": []
            })),
        );

        let outcome = server.dispatch(req).await;
        assert!(
            matches!(outcome, DispatchOutcome::Streamed { .. }),
            "session/prompt must return Streamed"
        );
    }

    #[tokio::test]
    async fn dispatch_session_prompt_unknown_session_id_returns_error_reply() {
        let server = make_server();
        let req = make_request(
            "session/prompt",
            Some(serde_json::json!({
                "sessionId": "does-not-exist",
                "prompt": [],
                "tools": []
            })),
        );

        let outcome = server.dispatch(req).await;
        let resp = match outcome {
            DispatchOutcome::Reply(r) => r,
            _ => panic!("expected Reply (error)"),
        };

        let err = resp.error.expect("error must be set");
        assert_eq!(err.code, -32602);
    }

    // -----------------------------------------------------------------------
    // error cases
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn dispatch_unknown_method_returns_method_not_found() {
        let server = make_server();
        let req = make_request("bogus/method", None);
        let outcome = server.dispatch(req).await;

        let resp = match outcome {
            DispatchOutcome::Reply(r) => r,
            _ => panic!("expected Reply"),
        };

        let err = resp.error.expect("error must be set");
        assert_eq!(err.code, -32601, "must return method-not-found");
    }

    #[tokio::test]
    async fn dispatch_request_with_wrong_jsonrpc_version_returns_invalid_request() {
        let server = make_server();
        let req = JsonRpcRequest {
            jsonrpc: "1.0".to_owned(),
            id: Value::Number(99.into()),
            method: "initialize".to_owned(),
            params: None,
        };
        let outcome = server.dispatch(req).await;

        let resp = match outcome {
            DispatchOutcome::Reply(r) => r,
            _ => panic!("expected Reply"),
        };

        let err = resp.error.expect("error must be set");
        assert_eq!(err.code, -32600, "must return invalid-request");
    }

    // -----------------------------------------------------------------------
    // agent info override
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn with_agent_info_overrides_default() {
        let server = AcpServer::new(Arc::new(MockHandler::new(vec![], "end_turn")))
            .with_agent_info(AgentInfoOverride {
                name: "custom-agent".to_owned(),
                version: "9.9.9".to_owned(),
            });

        let outcome = server.dispatch(init_request()).await;
        let resp = match outcome {
            DispatchOutcome::Reply(r) => r,
            _ => panic!("expected Reply"),
        };

        let result = resp.result.unwrap();
        assert_eq!(result["agentInfo"]["name"], "custom-agent");
        assert_eq!(result["agentInfo"]["version"], "9.9.9");
    }

    // -----------------------------------------------------------------------
    // Streamed outcome end-to-end
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn streamed_outcome_yields_handler_updates_then_completion() {
        let canned = vec![
            UpdatePayload::MessageChunk {
                content: MessageContent {
                    content_type: "text".to_owned(),
                    text: "chunk-1".to_owned(),
                },
            },
            UpdatePayload::MessageChunk {
                content: MessageContent {
                    content_type: "text".to_owned(),
                    text: "chunk-2".to_owned(),
                },
            },
        ];

        let server = Arc::new(
            AcpServer::new(Arc::new(MockHandler::new(canned, "end_turn"))),
        );

        let sid = session_new_id(&server).await;

        let req = make_request(
            "session/prompt",
            Some(serde_json::json!({
                "sessionId": sid,
                "prompt": [{ "type": "text", "text": "go" }],
                "tools": []
            })),
        );

        let outcome = server.dispatch(req).await;
        let (mut updates_rx, completion_rx) = match outcome {
            DispatchOutcome::Streamed {
                updates_rx,
                completion_rx,
            } => (updates_rx, completion_rx),
            _ => panic!("expected Streamed"),
        };

        // Drain exactly 2 notifications.
        let n1 = updates_rx.recv().await.expect("first notification");
        let n2 = updates_rx.recv().await.expect("second notification");

        assert_eq!(n1.method, "session/update");
        assert_eq!(n2.method, "session/update");

        // Verify the notifications carry the correct text.
        let p1 = n1.params.unwrap();
        let p2 = n2.params.unwrap();
        assert_eq!(p1["update"]["content"]["text"], "chunk-1");
        assert_eq!(p2["update"]["content"]["text"], "chunk-2");

        // No further notifications.
        assert!(updates_rx.recv().await.is_none());

        // Completion response must carry stopReason.
        let final_resp = completion_rx.await.expect("completion response");
        assert!(final_resp.error.is_none());
        assert_eq!(final_resp.result.unwrap()["stopReason"], "end_turn");
    }
}
