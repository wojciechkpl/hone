use std::pin::Pin;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, ToolCallRequest};
use tokio::sync::Mutex;

use crate::adapters::cli_common::{check_binary_available, estimate_tokens_heuristic};
use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

use super::transport::AcpTransport;
use super::types::{
    ClientInfo, InitializeParams, PromptContent, PromptParams, SessionNewParams, SessionUpdate,
    UpdatePayload,
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
            vec![
                ModelInfo {
                    id: ModelId::new("gemini-acp:gemini-2.5-pro"),
                    display_name: "Gemini 2.5 Pro (ACP)".to_owned(),
                    context_length: 1_000_000,
                    supports_tools: true,
                    supports_streaming: true,
                },
            ],
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

        let prompt_params = serde_json::to_value(PromptParams {
            session_id: session_id.clone(),
            prompt,
        })?;

        // We need to send `session/prompt` and then consume notifications until
        // we see the response.  Because `AcpTransport::request` buffers
        // notifications internally while waiting for the response, we:
        //   1. Call `request("session/prompt", ...)`.
        //   2. Drain `next_notification()` for all buffered updates.
        //
        // However, we need to process notifications that arrive *before* the
        // response.  The transport's `request()` already buffers those.  After
        // the call returns we drain the buffer.

        // Collect events before sending them as a stream.
        let mut events: Vec<ProviderEvent> = Vec::new();

        // Send the prompt request — this blocks until the response arrives and
        // buffers any notifications received in transit.
        let _prompt_result = state
            .transport
            .request("session/prompt", Some(prompt_params))
            .await?;

        // Drain all buffered notifications and convert to ProviderEvents.
        loop {
            // `next_notification()` would block on a live read when the buffer
            // is empty; we only want buffered entries here.
            // We use a workaround: call an async-cancellable variant with a
            // zero timeout.  Since `AcpTransport` doesn't expose a non-blocking
            // peek, we rely on the fact that after `session/prompt` returns the
            // agent has sent all updates.  We give it a short grace period.
            let notif_result = tokio::time::timeout(
                std::time::Duration::from_millis(100),
                state.transport.next_notification(),
            )
            .await;

            match notif_result {
                Ok(Ok(notif)) => {
                    if let Some(params) = notif.params {
                        if let Ok(update) =
                            serde_json::from_value::<SessionUpdate>(params)
                        {
                            if let Some(event) =
                                update_payload_to_provider_event(update.update)
                            {
                                events.push(event);
                            }
                        }
                    }
                }
                // Timeout — no more buffered notifications.
                Err(_) => break,
                // Transport error — stop reading.
                Ok(Err(_)) => break,
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
        UpdatePayload::MessageChunk { content } => Some(ProviderEvent::Token { text: content.text }),
        UpdatePayload::ToolCall {
            tool_call_id,
            title,
            kind: _,
            status: _,
        } => Some(ProviderEvent::ToolCall(ToolCallRequest {
            id: tool_call_id,
            name: title,
            arguments: serde_json::Value::Object(serde_json::Map::new()),
        })),
        UpdatePayload::ToolCallUpdate { .. } => {
            // tool_call_update signals completion of a tool call; there is no
            // direct ProviderEvent mapping — callers observe Done at the end.
            None
        }
    }
}
