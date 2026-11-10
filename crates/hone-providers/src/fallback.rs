use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelInfo, ProviderEvent, ProviderHealth};

use crate::error::ProviderError;
use crate::stream_probe::{Attempt, LastFailure, attempt, hands_over, with_carried};
use crate::traits::{CompletionRequest, ModelProvider};

/// A provider that tries each member in order, moving on when one fails with
/// a transient error, an `Unavailable` health status, or a stream that fails
/// before any output.
pub struct FallbackChain {
    providers: Vec<Arc<dyn ModelProvider>>,
}

impl FallbackChain {
    pub fn new(providers: Vec<Arc<dyn ModelProvider>>) -> Self {
        assert!(
            !providers.is_empty(),
            "FallbackChain requires at least one provider"
        );
        Self { providers }
    }
}

#[async_trait]
impl ModelProvider for FallbackChain {
    fn runs_own_tools(&self) -> bool {
        self.providers.iter().any(|p| p.runs_own_tools())
    }

    fn id(&self) -> &str {
        "fallback-chain"
    }

    fn display_name(&self) -> &str {
        "Fallback Chain"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        self.providers
            .iter()
            .flat_map(|p| p.supported_models())
            .collect()
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        // Usage reported by members that failed before any output, passed on
        // ahead of the events of the member that finally answers.
        let mut carried: Vec<ProviderEvent> = Vec::new();
        let mut last: Option<LastFailure> = None;

        for provider in &self.providers {
            // Skip unavailable providers without even trying them.
            if let ProviderHealth::Unavailable { reason } = provider.health().await {
                last = Some(LastFailure::Request(ProviderError::Http {
                    provider: provider.id().to_owned(),
                    status: 503,
                    body: reason,
                }));
                continue;
            }

            match attempt(provider.as_ref(), request.clone()).await {
                Attempt::Stream(stream) => return Ok(with_carried(carried, stream)),
                Attempt::Failed(failure) => {
                    carried.extend(failure.usage);
                    last = Some(LastFailure::Stream(failure.message, failure.kind));
                }
                Attempt::Request(e) if hands_over(&e) => {
                    last = Some(LastFailure::Request(e));
                }
                Attempt::Request(e) => return Err(e),
            }
        }

        // Safe: constructor asserts !providers.is_empty(), so the loop ran at least once.
        #[allow(clippy::expect_used)]
        last.expect("providers vec is non-empty").report(carried)
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        self.providers
            .first()
            .map(|p| p.estimate_tokens(messages))
            .unwrap_or(0)
    }

    async fn health(&self) -> ProviderHealth {
        // Healthy if any member is healthy.
        for provider in &self.providers {
            if provider.health().await == ProviderHealth::Healthy {
                return ProviderHealth::Healthy;
            }
        }
        ProviderHealth::Degraded {
            reason: "all members unhealthy".to_owned(),
        }
    }
}

/// A `MockProvider` variant that always returns a `RateLimit` error.
/// Used internally by tests; public so integration tests can reference it.
pub struct RateLimitedProvider {
    pub id: String,
}

#[async_trait]
impl ModelProvider for RateLimitedProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        "Rate Limited"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        Err(ProviderError::RateLimit {
            provider: self.id.clone(),
            retry_after_secs: 60,
        })
    }

    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        0
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Degraded {
            reason: "rate limited".to_owned(),
        }
    }
}

/// A `ModelProvider` that always reports `Unavailable` health and errors on complete.
pub struct UnavailableProvider {
    pub id: String,
}

#[async_trait]
impl ModelProvider for UnavailableProvider {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        "Unavailable"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        Err(ProviderError::Http {
            provider: self.id.clone(),
            status: 503,
            body: "unavailable".to_owned(),
        })
    }

    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        0
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Unavailable {
            reason: "offline".to_owned(),
        }
    }
}
