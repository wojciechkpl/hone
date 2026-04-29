use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, ToolCallRequest};
use tokio::sync::Mutex;

use crate::adapters::cli_common::{check_binary_available, estimate_tokens_heuristic};
use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

use super::transport::{AcpTransport, TransportEvent};
use super::types::{
    AcpToolSchema, ClientInfo, InitializeParams, PromptContent, PromptParams, SessionNewParams,
    SessionUpdate, ToolResultParams, UpdatePayload,
};

// ---------------------------------------------------------------------------
// AcpProvider
// ---------------------------------------------------------------------------

/// A `ModelProvider` that speaks the Agent Client Protocol (ACP) — JSON-RPC 2.0
/// over the stdin/stdout of a persistent subprocess.
pub struct AcpProvider {
    binary: String,
    args: Vec<String>,
    provider_id: String,
    display_name_str: String,
    models: Vec<ModelInfo>,
    /// Lazily-initialised transport + session state, shared across `complete()`
    /// calls because the trait takes `&self`.
    state: Mutex<Option<AcpState>>,
}

struct AcpState {
    transport: AcpTransport,
    session_id: String,
}

impl AcpProvider {
    /// Provider that delegates to the `claude --acp` CLI.
    pub fn claude() -> Self {
        Self::custom(
            "claude",
            &["--acp"],
            "claude-acp",
            "Claude (ACP)",
            vec![
                ModelInfo {
                    id: ModelId::new("claude-acp:claude-sonnet-4-5"),
                    display_name: "Claude Sonnet 4.5 (ACP)".to_owned(),
                    context_length: 200_000,
                    supports_tools: true,
                    supports_streaming: true,
                },
                ModelInfo {
                    id: ModelId::new("claude-acp:claude-opus-4-5"),
                    display_name: "Claude Opus 4.5 (ACP)".to_owned(),
                    context_length: 200_000,
                    supports_tools: true,
                    supports_streaming: true,
                },
            ],
        )
    }

    /// Provider that delegates to the `gemini --acp` CLI.
    pub fn gemini() -> Self {
        Self::custom(
            "gemini",
            &["--acp"],
            "gemini-acp",
            "Gemini (ACP)",
            vec![ModelInfo {
                id: ModelId::new("gemini-acp:gemini-2.5-pro"),
                display_name: "Gemini 2.5 Pro (ACP)".to_owned(),
                context_length: 1_000_000,
                supports_tools: true,
                supports_streaming: true,
            }],
        )
    }

    /// Create a provider with a fully custom binary, arguments, and metadata.
    pub fn custom(
        binary: &str,
        args: &[&str],
        id: &str,
        name: &str,
        models: Vec<ModelInfo>,
    ) -> Self {
        Self {
            binary: binary.to_owned(),
            args: args.iter().map(|s| (*s).to_owned()).collect(),
            provider_id: id.to_owned(),
            display_name_str: name.to_owned(),
            models,
            state: Mutex::new(None),
        }
    }

    /// Ensure the transport is initialised and return a reference to the session
    /// id string.  Spawns the binary and performs the `initialize` + `session/new`
    /// handshake on first call.
    async fn ensure_initialised(
        state: &mut Option<AcpState>,
        binary: &str,
        args: &[String],
    ) -> Result<(), ProviderError> {
        if state.is_some() {
            return Ok(());
        }

        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let mut transport = AcpTransport::spawn(binary, &args_ref).await?;

        // initialize
        let init_params = serde_json::to_value(InitializeParams {
            protocol_version: 1,
            client_info: ClientInfo {
                name: "hone".to_owned(),
                version: "0.1.0".to_owned(),
            },
        })?;
        transport.request("initialize", Some(init_params)).await?;

        // session/new
        let session_params = serde_json::to_value(SessionNewParams {
            cwd: std::env::current_dir()
                .map_err(|e| ProviderError::Http {
                    provider: "acp".to_owned(),
                    status: 0,
                    body: format!("failed to determine current directory: {e}"),
                })?
                .to_string_lossy()
                .into_owned(),
        })?;
        let session_result = transport
            .request("session/new", Some(session_params))
            .await?;

        let session_id = session_result
            .get("sessionId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ProviderError::Http {
                provider: "acp".to_owned(),
                status: 0,
                body: "ACP session/new response missing sessionId".to_owned(),
            })?
            .to_owned();

        *state = Some(AcpState {
            transport,
            session_id,
        });

        Ok(())
    }
}

#[async_trait]
impl ModelProvider for AcpProvider {
    fn id(&self) -> &str {
        &self.provider_id
    }

    fn display_name(&self) -> &str {
        &self.display_name_str
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        self.models.clone()
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        // Acquire the state mutex and drive the full ACP conversation for this
        // prompt.  We build all events into a Vec and stream them at the end —
        // this keeps the lifetime of the MutexGuard contained to this function.
        let mut guard = self.state.lock().await;

        Self::ensure_initialised(&mut guard, &self.binary, &self.args).await?;

        // Safe: ensure_initialised guarantees guard contains Some after returning Ok.
        #[allow(clippy::expect_used)]
        let state = guard.as_mut().expect("state initialised above");

        // Convert messages to ACP prompt content.
        let prompt: Vec<PromptContent> = request
            .messages
            .iter()
            .map(|m| PromptContent {
                content_type: "text".to_owned(),
                text: m.content.as_text().to_owned(),
            })
            .collect();

        let session_id = state.session_id.clone();

        let tools: Vec<AcpToolSchema> = request
            .tools
            .into_iter()
            .map(|t| AcpToolSchema {
                name: t.name,
                description: t.description,
                parameters: t.parameters,
            })
            .collect();

        let prompt_params = serde_json::to_value(PromptParams {
            session_id: session_id.clone(),
            prompt,
            tools,
        })?;

        // Collect events before sending them as a stream.
        let mut events: Vec<ProviderEvent> = Vec::new();

        // Send the prompt request *without* blocking for the response.  The
        // agent may interleave notifications (tool_call, agent_message_chunk)
        // and may even block waiting for a `session/toolResult` before it
        // sends the prompt response.  We therefore use the non-blocking
        // `send_request` + `next_event` primitives to drive the conversation.
        let prompt_id = state
            .transport
            .send_request("session/prompt", Some(prompt_params))
            .await?;

        // Bidirectional event loop: read responses and notifications until
        // either the `session/prompt` response (id == prompt_id) arrives or a
        // recoverable error is surfaced.  Done is always pushed last so the
        // provider stream invariant is preserved.
        'outer: loop {
            let event = match state.transport.next_event().await {
                Ok(e) => e,
                Err(e) => {
                    events.push(ProviderEvent::Error {
                        message: format!("ACP transport error: {e}"),
                    });
                    break 'outer;
                }
            };

            match event {
                TransportEvent::Response(resp) if resp.id == prompt_id => {
                    // The session/prompt call has finished — exit the loop.
                    break 'outer;
                }
                TransportEvent::Response(_) => {
                    // Response for a different in-flight request (e.g. a
                    // session/toolResult we fired); discard it.
                }
                TransportEvent::Notification(notif) => {
                    let Some(params) = notif.params else {
                        continue;
                    };
                    let update = match serde_json::from_value::<SessionUpdate>(params) {
                        Ok(u) => u,
                        Err(_) => continue,
                    };

                    match &update.update {
                        UpdatePayload::ToolCall {
                            tool_call_id,
                            title,
                            raw_input,
                            ..
                        } => {
                            // (a) Emit ToolCall event to the consumer.  An
                            // absent `rawInput` becomes an empty JSON object
                            // (legacy-agent compatibility — ToolCallRequest
                            // contracts arguments to be a JSON value, never
                            // null).
                            events.push(ProviderEvent::ToolCall(ToolCallRequest {
                                id: tool_call_id.clone(),
                                name: title.clone(),
                                arguments: raw_input.clone().unwrap_or_else(|| {
                                    serde_json::Value::Object(serde_json::Map::new())
                                }),
                            }));

                            // (b) Pull the ToolResult from the caller-supplied channel.
                            let tool_result = match &request.tool_results {
                                Some(rx) => match rx.recv().await {
                                    Some(r) => r,
                                    None => {
                                        events.push(ProviderEvent::Error {
                                            message: "tool result channel closed before result \
                                                      arrived"
                                                .to_owned(),
                                        });
                                        break 'outer;
                                    }
                                },
                                None => {
                                    events.push(ProviderEvent::Error {
                                        message: "received tool result request but no tool result \
                                                  channel was provided"
                                            .to_owned(),
                                    });
                                    break 'outer;
                                }
                            };

                            // (c) Forward the result to the agent.  Fire-and-forget:
                            // the agent's response will be consumed (and discarded)
                            // in the next iteration of this loop as a Response with
                            // a non-matching id.
                            let tool_result_params =
                                serde_json::to_value(ToolResultParams {
                                    session_id: session_id.clone(),
                                    tool_call_id: tool_result.tool_call_id,
                                    content: tool_result.content,
                                    is_error: tool_result.is_error,
                                })?;
                            state
                                .transport
                                .send_request("session/toolResult", Some(tool_result_params))
                                .await?;
                        }
                        _ => {
                            if let Some(provider_event) =
                                update_payload_to_provider_event(update.update)
                            {
                                events.push(provider_event);
                            }
                        }
                    }
                }
            }
        }

        events.push(ProviderEvent::Done);

        Ok(Box::pin(stream::iter(events)))
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        estimate_tokens_heuristic(messages)
    }

    async fn health(&self) -> ProviderHealth {
        check_binary_available(&self.binary).await
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn update_payload_to_provider_event(payload: UpdatePayload) -> Option<ProviderEvent> {
    match payload {
        UpdatePayload::MessageChunk { content } => {
            Some(ProviderEvent::Token { text: content.text })
        }
        UpdatePayload::ToolCall {
            tool_call_id,
            title,
            raw_input,
            kind: _,
            status: _,
        } => Some(ProviderEvent::ToolCall(ToolCallRequest {
            id: tool_call_id,
            name: title,
            arguments: raw_input
                .unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new())),
        })),
        UpdatePayload::ToolCallUpdate {
            tool_call_id,
            status,
        } => Some(ProviderEvent::ObservedToolEnd {
            id: tool_call_id,
            is_error: status_indicates_error(&status),
            result: status,
        }),
    }
}

/// Whether a `tool_call_update.status` string represents a terminal failure.
///
/// Matched case-insensitively against the strings `"failed"` and `"error"`,
/// which are the two terminal-failure markers used by the agents we target
/// (claude --acp, codex, goose-style implementations).  Anything else —
/// including `"completed"`, `"success"`, in-progress markers, or unknown
/// future variants — is treated as non-error so `ObservedToolEnd.is_error`
/// stays conservative; downstream callers can re-classify based on the
/// preserved `result` string.
fn status_indicates_error(status: &str) -> bool {
    let lower = status.to_ascii_lowercase();
    lower == "failed" || lower == "error"
}
