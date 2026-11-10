//! One provider that serves several: each request goes to the provider for its
//! model's prefix (`anthropic:`, `openai:`, `ollama:`), optionally with a list of
//! fallback models that take over when a request fails the way
//! [`FallbackChain`] hands over.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth};

use crate::error::ProviderError;
use crate::fallback::FallbackChain;
use crate::traits::{CompletionRequest, ModelProvider};

/// Routes each request to the provider for `request.model`'s prefix.
pub struct ModelDispatch {
    providers: HashMap<String, Arc<dyn ModelProvider>>,
    default_model: ModelId,
    fallback: Vec<ModelId>,
}

impl ModelDispatch {
    pub fn new(default_model: ModelId) -> Self {
        Self {
            providers: HashMap::new(),
            default_model,
            fallback: Vec::new(),
        }
    }

    /// Serve models whose id starts with `prefix:` (e.g. `"openai"`).
    pub fn with_provider(mut self, prefix: &str, provider: Arc<dyn ModelProvider>) -> Self {
        self.providers.insert(prefix.to_owned(), provider);
        self
    }

    /// Models tried, in order, after the request's own model fails.
    pub fn with_fallback(mut self, models: Vec<ModelId>) -> Self {
        self.fallback = models;
        self
    }

    /// True if a provider is registered for this model's prefix.
    pub fn has_provider_for(&self, model: &ModelId) -> bool {
        self.provider_for(model).is_some()
    }

    fn provider_for(&self, model: &ModelId) -> Option<&Arc<dyn ModelProvider>> {
        model.provider().and_then(|p| self.providers.get(p))
    }

    fn default_provider(&self) -> Option<&Arc<dyn ModelProvider>> {
        self.provider_for(&self.default_model)
    }
}

/// A provider that always asks for one particular model.
struct Pinned {
    inner: Arc<dyn ModelProvider>,
    model: ModelId,
}

#[async_trait]
impl ModelProvider for Pinned {
    fn runs_own_tools(&self) -> bool {
        self.inner.runs_own_tools()
    }
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn display_name(&self) -> &str {
        self.inner.display_name()
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        self.inner.supported_models()
    }
    async fn complete(
        &self,
        mut request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        request.model = self.model.clone();
        self.inner.complete(request).await
    }
    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        self.inner.estimate_tokens(messages)
    }
    /// Always healthy: [`FallbackChain`] asks every member for its health before
    /// trying it, and for the HTTP providers that is a real (billed) request.
    /// A member that is down fails its own request and the chain moves on.
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

#[async_trait]
impl ModelProvider for ModelDispatch {
    fn runs_own_tools(&self) -> bool {
        self.providers.values().any(|p| p.runs_own_tools())
    }

    fn id(&self) -> &str {
        self.default_provider().map_or("model-dispatch", |p| p.id())
    }

    fn display_name(&self) -> &str {
        self.default_provider()
            .map_or("Model dispatch", |p| p.display_name())
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        self.default_provider()
            .map(|p| p.supported_models())
            .unwrap_or_default()
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let Some(primary) = self.provider_for(&request.model) else {
            return Err(ProviderError::ModelNotFound {
                model: request.model.as_str().to_owned(),
                provider: request.model.provider().unwrap_or_default().to_owned(),
            });
        };

        // The request's own model first, then each fallback model that has a provider.
        let mut members: Vec<Arc<dyn ModelProvider>> = Vec::new();
        for model in &self.fallback {
            if *model == request.model {
                continue;
            }
            if let Some(provider) = self.provider_for(model) {
                members.push(Arc::new(Pinned {
                    inner: Arc::clone(provider),
                    model: model.clone(),
                }));
            }
        }
        if members.is_empty() {
            return primary.complete(request).await;
        }
        members.insert(
            0,
            Arc::new(Pinned {
                inner: Arc::clone(primary),
                model: request.model.clone(),
            }),
        );
        FallbackChain::new(members).complete(request).await
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        self.default_provider()
            .map_or(0, |p| p.estimate_tokens(messages))
    }

    async fn health(&self) -> ProviderHealth {
        match self.default_provider() {
            Some(p) => p.health().await,
            None => ProviderHealth::Unavailable {
                reason: "no provider for the default model".to_owned(),
            },
        }
    }
}
