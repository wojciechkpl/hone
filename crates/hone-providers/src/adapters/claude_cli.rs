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
                id: ModelId::new("claude-cli:claude-sonnet-4-6"),
                display_name: "Claude Sonnet 4.6 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("claude-cli:claude-opus-4-7"),
                display_name: "Claude Opus 4.7 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
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
/// The Claude CLI stream-json format emits newline-delimited JSON objects:
/// - `{"type":"assistant","message":{"content":[{"type":"text","text":"..."},...]}}`
///   Text blocks → Token.  `tool_use` blocks → ObservedToolStart.
/// - `{"type":"user","message":{"content":[{"type":"tool_result",...}]}}`
///   → ObservedToolEnd (the CLI handled execution; hone just observes).
/// - `{"type":"result",...}` → Usage + Done.
/// - stderr lines forwarded as Error events so nothing is silently swallowed.
fn spawn_claude_stream_with_stdin(mut cmd: Command, prompt: String) -> Pin<Box<dyn Stream<Item = ProviderEvent> + Send>> {
    let (tx, rx) = mpsc::channel::<ProviderEvent>(64);

    tokio::spawn(async move {
        let child_result = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())   // capture instead of /dev/null
            .kill_on_drop(true)       // kill subprocess when the task is aborted/dropped
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
            drop(stdin);
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

        // Drain stderr in a separate task so it doesn't block stdout reading.
        let stderr_tx = tx.clone();
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let trimmed = line.trim().to_owned();
                    if !trimmed.is_empty() {
                        tracing::debug!(source = "claude-cli-stderr", "{trimmed}");
                        // Only forward lines that look like errors to avoid
                        // forwarding normal startup/status noise.
                        let lower = trimmed.to_ascii_lowercase();
                        // Forward stderr lines that look like fatal errors (start with a recognised
                        // severity keyword). Avoid forwarding lines that merely mention "error" as
                        // part of a status message (e.g. "0 failures").
                        let is_error_line = lower.starts_with("error")
                            || lower.starts_with("fatal")
                            || lower.starts_with("uncaught")
                            || lower.starts_with("panic")
                            || (lower.contains("authentication") && lower.contains("fail"))
                            || (lower.contains("unauthorized") || lower.contains("403"));
                        if is_error_line {
                            let _ = stderr_tx.send(ProviderEvent::Error {
                                message: format!("claude: {trimmed}"),
                            }).await;
                        }
                    }
                }
            });
        }

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
                            last_text = String::new();  // reset for each assistant block
                            let content = json
                                .get("message")
                                .and_then(|m| m.get("content"))
                                .and_then(|c| c.as_array());

                            if let Some(blocks) = content {
                                for block in blocks {
                                    match block.get("type").and_then(|t| t.as_str()) {
                                        Some("text") => {
                                            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                                // Claude sends cumulative text — emit only the new part.
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
                                        Some("tool_use") => {
                                            // The claude CLI is invoking one of its own tools.
                                            // Emit an ObservedToolStart so the TUI can show it.
                                            let id = block.get("id")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("unknown")
                                                .to_owned();
                                            let name = block.get("name")
                                                .and_then(|v| v.as_str())
                                                .unwrap_or("tool")
                                                .to_owned();
                                            let input = block.get("input")
                                                .cloned()
                                                .unwrap_or(serde_json::Value::Null);
                                            let _ = tx.send(ProviderEvent::ObservedToolStart { id, name, input }).await;
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }

                        Some("user") => {
                            // Tool results sent back to the model by the claude CLI.
                            let content = json
                                .get("message")
                                .and_then(|m| m.get("content"))
                                .and_then(|c| c.as_array());

                            if let Some(blocks) = content {
                                for block in blocks {
                                    if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                                        let id = block.get("tool_use_id")
                                            .and_then(|v| v.as_str())
                                            .unwrap_or("unknown")
                                            .to_owned();
                                        let is_error = block.get("is_error")
                                            .and_then(|v| v.as_bool())
                                            .unwrap_or(false);
                                        let result = match block.get("content") {
                                            Some(serde_json::Value::String(s)) => s.clone(),
                                            Some(serde_json::Value::Array(arr)) => {
                                                arr.iter()
                                                    .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                                    .collect::<Vec<_>>()
                                                    .join("\n")
                                            }
                                            _ => String::new(),
                                        };
                                        let _ = tx.send(ProviderEvent::ObservedToolEnd { id, result, is_error }).await;
                                    }
                                }
                            }
                        }

                        Some("result") => {
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
                                    cache_creation: 0,
                                })).await;
                            }
                            // Check for error result subtype.
                            if json.get("subtype").and_then(|v| v.as_str()) == Some("error") {
                                let msg = json.get("error")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown error");
                                let _ = tx.send(ProviderEvent::Error {
                                    message: msg.to_owned(),
                                }).await;
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
