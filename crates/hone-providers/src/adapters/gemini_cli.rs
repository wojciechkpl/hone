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

/// A `ModelProvider` that delegates to the `gemini` CLI tool.
///
/// Requires the `gemini` binary to be installed and available on `PATH`.
pub struct GeminiCliProvider {
    binary: String,
}

impl GeminiCliProvider {
    /// Create a provider using the `gemini` binary found on `PATH`.
    pub fn new() -> Self {
        Self {
            binary: "gemini".to_owned(),
        }
    }

    /// Create a provider using a specific binary path.
    pub fn with_binary(binary: impl Into<String>) -> Self {
        Self {
            binary: binary.into(),
        }
    }
}

impl Default for GeminiCliProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ModelProvider for GeminiCliProvider {
    fn id(&self) -> &str {
        "gemini-cli"
    }

    fn display_name(&self) -> &str {
        "Gemini CLI"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: ModelId::new("gemini-cli:gemini-2.5-pro"),
                display_name: "Gemini 2.5 Pro (CLI)".to_owned(),
                context_length: 1_000_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("gemini-cli:gemini-2.5-flash"),
                display_name: "Gemini 2.5 Flash (CLI)".to_owned(),
                context_length: 1_000_000,
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
        cmd.arg("-m").arg(&model_name).arg(&prompt);

        Ok(spawn_cli_stream(cmd))
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        estimate_tokens_heuristic(messages)
    }

    async fn health(&self) -> ProviderHealth {
        check_binary_available(&self.binary).await
    }
}
