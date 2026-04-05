use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth};

use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

pub struct OllamaProvider {
    #[allow(dead_code)]
    base_url: String,
}

impl OllamaProvider {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self { base_url: base_url.into() }
    }
}

#[async_trait]
impl ModelProvider for OllamaProvider {
    fn id(&self) -> &str {
        "ollama"
    }

    fn display_name(&self) -> &str {
        "Ollama"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("ollama:llama3.2"),
            display_name: "Llama 3.2".to_owned(),
            context_length: 128_000,
            supports_tools: true,
            supports_streaming: true,
        }]
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        todo!("Ollama HTTP adapter not yet implemented")
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        let _ = messages;
        todo!("Ollama token estimation not yet implemented")
    }

    async fn health(&self) -> ProviderHealth {
        todo!("Ollama health check not yet implemented")
    }
}
