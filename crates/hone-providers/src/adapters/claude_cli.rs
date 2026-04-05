use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth};
use tokio::process::Command;

use crate::adapters::cli_common::{
    check_binary_available, estimate_tokens_heuristic, messages_to_prompt, spawn_cli_stream,
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
        cmd.arg("--output-format")
            .arg("stream-json")
            .arg("--model")
            .arg(&model_name)
            .arg("-p")
            .arg(&prompt);

        Ok(spawn_cli_stream(cmd))
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        estimate_tokens_heuristic(messages)
    }

    async fn health(&self) -> ProviderHealth {
        check_binary_available(&self.binary).await
    }
}
