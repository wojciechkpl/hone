use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth};

use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

pub struct AnthropicProvider {
    #[allow(dead_code)]
    api_key: String,
}

impl AnthropicProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self { api_key: api_key.into() }
    }
}

#[async_trait]
impl ModelProvider for AnthropicProvider {
    fn id(&self) -> &str {
        "anthropic"
    }

    fn display_name(&self) -> &str {
        "Anthropic"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: ModelId::new("anthropic:claude-opus-4-5"),
                display_name: "Claude Opus 4.5".to_owned(),
                context_length: 200_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("anthropic:claude-sonnet-4-5"),
                display_name: "Claude Sonnet 4.5".to_owned(),
                context_length: 200_000,
                supports_tools: true,
                supports_streaming: true,
            },
        ]
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        todo!("Anthropic HTTP adapter not yet implemented")
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        let _ = messages;
        todo!("Anthropic token estimation not yet implemented")
    }

    async fn health(&self) -> ProviderHealth {
        todo!("Anthropic health check not yet implemented")
    }
}
