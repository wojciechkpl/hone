use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, TokenUsage, ToolCallRequest};

use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

/// Deterministic in-process provider for tests.
///
/// Emits a configurable sequence of `ProviderEvent`s followed by `Done`.
/// Never touches the network.
///
/// The last `CompletionRequest` received by `complete()` is stored in
/// `last_request` so tests can assert on what was sent.
pub struct MockProvider {
    id: String,
    tokens: Vec<String>,
    tool_calls: Vec<ToolCallRequest>,
    usage: Option<TokenUsage>,
    /// Captures the most recent `CompletionRequest` passed to `complete()`.
    pub last_request: Arc<Mutex<Option<CompletionRequest>>>,
}

impl MockProvider {
    /// Create a provider that streams the given tokens then emits `Done`.
    pub fn new(tokens: Vec<String>) -> Self {
        Self {
            id: "mock".to_owned(),
            tokens,
            tool_calls: Vec::new(),
            usage: None,
            last_request: Arc::new(Mutex::new(None)),
        }
    }

    /// Create a provider that emits a single `ToolCall` event then `Done`.
    pub fn with_tool_call(
        id: impl Into<String>,
        name: impl Into<String>,
        args: serde_json::Value,
    ) -> Self {
        let call = ToolCallRequest {
            id: id.into(),
            name: name.into(),
            arguments: args,
        };
        Self {
            id: "mock".to_owned(),
            tokens: Vec::new(),
            tool_calls: vec![call],
            usage: None,
            last_request: Arc::new(Mutex::new(None)),
        }
    }

    /// Builder: attach a `Usage` event to the stream.
    pub fn with_usage(mut self, usage: TokenUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Builder: override the provider id string.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }

    fn build_events(&self) -> Vec<ProviderEvent> {
        let mut events: Vec<ProviderEvent> = self
            .tokens
            .iter()
            .map(|t| ProviderEvent::Token { text: t.clone() })
            .collect();

        for call in &self.tool_calls {
            events.push(ProviderEvent::ToolCall(call.clone()));
        }

        if let Some(usage) = self.usage {
            events.push(ProviderEvent::Usage(usage));
        }

        events.push(ProviderEvent::Done);
        events
    }
}

#[async_trait]
impl ModelProvider for MockProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        "Mock Provider"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:mock-model"),
            display_name: "Mock Model".to_owned(),
            context_length: 4096,
            supports_tools: true,
            supports_streaming: true,
        }]
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        *self.last_request.lock().unwrap() = Some(request);
        let events = self.build_events();
        Ok(Box::pin(stream::iter(events)))
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        // Rough heuristic: 4 chars per token, minimum 1.
        let total_chars: usize = messages
            .iter()
            .map(|m| m.content.as_text().len())
            .sum();
        ((total_chars / 4) as u32).max(1)
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}
