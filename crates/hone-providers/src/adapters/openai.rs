use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth};

use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

pub struct OpenAiProvider {
    #[allow(dead_code)]
    api_key: String,
}

impl OpenAiProvider {
    pub fn new(api_key: impl Into<String>) -> Self {
        Self { api_key: api_key.into() }
    }
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn id(&self) -> &str {
        "openai"
    }

    fn display_name(&self) -> &str {
        "OpenAI"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: ModelId::new("openai:gpt-4o"),
                display_name: "GPT-4o".to_owned(),
                context_length: 128_000,
                supports_tools: true,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("openai:o3"),
                display_name: "o3".to_owned(),
                context_length: 200_000,
                supports_tools: true,
                supports_streaming: false,
            },
        ]
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        todo!("OpenAI HTTP adapter not yet implemented")
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        let _ = messages;
        todo!("OpenAI token estimation not yet implemented")
    }

    async fn health(&self) -> ProviderHealth {
        todo!("OpenAI health check not yet implemented")
    }
}
