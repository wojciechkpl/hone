use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, ToolSchema};

use crate::error::ProviderError;

/// A request to complete a conversation turn with a model.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: ModelId,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSchema>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    pub stop: Vec<String>,
}

impl CompletionRequest {
    pub fn new(model: impl Into<ModelId>, messages: Vec<Message>) -> Self {
        Self {
            model: model.into(),
            messages,
            tools: Vec::new(),
            temperature: None,
            max_tokens: None,
            stop: Vec::new(),
        }
    }
}

/// A streaming model provider.
#[async_trait]
pub trait ModelProvider: Send + Sync + 'static {
    /// Stable identifier for this provider (e.g. `"anthropic"`, `"openai"`).
    fn id(&self) -> &str;

    /// Human-readable name for this provider.
    fn display_name(&self) -> &str;

    /// All models this provider exposes.
    fn supported_models(&self) -> Vec<ModelInfo>;

    /// Begin a streaming completion. Returns a pinned stream of `ProviderEvent`s.
    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError>;

    /// Rough token count heuristic used for cost estimation / context trimming.
    fn estimate_tokens(&self, messages: &[Message]) -> u32;

    /// Current health of this provider.
    async fn health(&self) -> ProviderHealth;
}
