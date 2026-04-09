use std::pin::Pin;
use std::process::Stdio;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, TokenUsage};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;

use crate::adapters::cli_common::{
    check_binary_available, estimate_tokens_heuristic, messages_to_prompt,
};
use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

/// A `ModelProvider` that delegates to the `claude` CLI tool.
///
/// Requires the `claude` binary to be installed and available on `PATH`.
pub struct ClaudeCliProvider {
    binary: String,
}

impl ClaudeCliProvider {
    /// Create a provider using the `claude` binary found on `PATH`.
    pub fn new() -> Self {
        Self {
            binary: "claude".to_owned(),
        }
    }

    /// Create a provider using a specific binary path.
    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }
}

impl Default for ClaudeCliProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for ClaudeCliProvider {
    fn id(&self) -> &str {
        "claude-cli"
    }

    fn display_name(&self) -> &str {
        "Claude CLI"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: ModelId::new("claude-cli:claude-sonnet-4-5"),
                display_name: "Claude Sonnet 4.5 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("claude-cli:claude-opus-4-5"),
                display_name: "Claude Opus 4.5 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("claude-cli:claude-haiku-3-5"),
                display_name: "Claude Haiku 3.5 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
        ]
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let model_name = request.model.model_name().to_owned();
        let prompt = messages_to_prompt(&request.messages);

        let mut cmd = Command::new(&self.binary);
        cmd.arg("-p")
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose")
            .arg("--model")
            .arg(&model_name);

        Ok(spawn_claude_stream_with_stdin(cmd, prompt))
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        estimate_tokens_heuristic(messages)
    }

    async fn health(&self) -> ProviderHealth {
        check_binary_available(&self.binary).await
    }
}

/// Spawn a Claude CLI process and parse its stream-json output into ProviderEvents.
///
/// The prompt is piped via stdin instead of passed as a CLI argument to avoid
/// OS arg-length limits and exposure in `ps` output.
///
/// The Claude CLI stream-json format emits newline-delimited JSON objects with a `type` field:
/// - `{"type":"assistant","message":{"content":[{"type":"text","text":"..."}],...}}` -> Token
/// - `{"type":"result","result":"...","total_cost_usd":...,"usage":{...}}` -> Usage + Done
///
/// Other types (system, rate_limit_event) are skipped.
fn spawn_claude_stream_with_stdin(mut cmd: Command, prompt: String) -> Pin<Box<dyn Stream<Item = ProviderEvent> + Send>> {
    let (tx, rx) = mpsc::channel::<ProviderEvent>(64);

    tokio::spawn(async move {
        let child_result = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();

        let mut child = match child_result {
            Ok(c) => c,
            Err(e) => {
                let _ = tx.send(ProviderEvent::Error {
                    message: format!("failed to spawn claude: {e}"),
                }).await;
                let _ = tx.send(ProviderEvent::Done).await;
                return;
            }
        };

        // Pipe the prompt via stdin and close to signal EOF.
        if let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            if let Err(e) = stdin.write_all(prompt.as_bytes()).await {
                let _ = tx.send(ProviderEvent::Error {
                    message: format!("failed to write prompt to stdin: {e}"),
                }).await;
                let _ = tx.send(ProviderEvent::Done).await;
                return;
            }
            drop(stdin); // Close stdin so the CLI knows input is complete.
        }

        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = tx.send(ProviderEvent::Error {
                    message: "failed to capture claude stdout".to_owned(),
                }).await;
                let _ = tx.send(ProviderEvent::Done).await;
                return;
            }
        };

        let mut lines = BufReader::new(stdout).lines();
        let mut last_text = String::new();

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    let Ok(json) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                        continue;
                    };

                    match json.get("type").and_then(|t| t.as_str()) {
                        Some("assistant") => {
                            // Extract text from content array
                            if let Some(content) = json
                                .get("message")
                                .and_then(|m| m.get("content"))
                                .and_then(|c| c.as_array())
                            {
                                for block in content {
                                    if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                            // Claude sends cumulative text — emit only the new part
                                            if text.len() > last_text.len() && text.starts_with(&last_text) {
                                                let new_part = &text[last_text.len()..];
                                                if !new_part.is_empty() {
                                                    let _ = tx.send(ProviderEvent::Token {
                                                        text: new_part.to_owned(),
                                                    }).await;
                                                }
                                            } else if text != last_text {
                                                let _ = tx.send(ProviderEvent::Token {
                                                    text: text.to_owned(),
                                                }).await;
                                            }
                                            last_text = text.to_owned();
                                        }
                                    }
                                }
                            }
                        }
                        Some("result") => {
                            // Extract usage if available
                            if let Some(usage) = json.get("usage") {
                                let input = usage.get("input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32;
                                let output = usage.get("output_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32;
                                let cache_read = usage.get("cache_read_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0) as u32;
                                let _ = tx.send(ProviderEvent::Usage(TokenUsage {
                                    input,
                                    output,
                                    cache_read,
                                })).await;
                            }
                            break;
                        }
                        _ => {
                            // Skip system, rate_limit_event, etc.
                        }
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    let _ = tx.send(ProviderEvent::Error {
                        message: format!("error reading claude output: {e}"),
                    }).await;
                    break;
                }
            }
        }

        // Wait for process to finish
        match child.wait().await {
            Ok(status) if !status.success() => {
                let code = status.code().unwrap_or(-1);
                let _ = tx.send(ProviderEvent::Error {
                    message: format!("claude exited with status {code}"),
                }).await;
            }
            _ => {}
        }

        let _ = tx.send(ProviderEvent::Done).await;
    });

    Box::pin(stream::unfold(rx, |mut rx| async move {
        let event = rx.recv().await?;
        Some((event, rx))
    }))
}
