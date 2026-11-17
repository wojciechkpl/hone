#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Spec G6/G7: `FallbackChain` reports its members after the first as the
//! models that may serve a request instead; `CachingProvider` forwards its
//! inner provider's `fallback_models`.
//!
//! Assumption (G6): a chain member's model is `ModelId::new(member.id())`
//! (the dispatch's pinned members report their full model id as `id()`).

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth};
use hone_providers::{
    CachingProvider, CompletionRequest, FallbackChain, ModelProvider, ProviderError,
};

struct Fake {
    id: String,
    fallbacks: Vec<ModelId>,
}

fn fake(id: &str, fallbacks: &[&str]) -> Arc<dyn ModelProvider> {
    Arc::new(Fake {
        id: id.to_owned(),
        fallbacks: fallbacks.iter().map(|m| ModelId::new(*m)).collect(),
    })
}

#[async_trait]
impl ModelProvider for Fake {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.id
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }
    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        Err(ProviderError::Http {
            provider: "fake".to_owned(),
            status: 500,
            body: String::new(),
        })
    }
    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        0
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
    fn fallback_models(&self, _model: &ModelId) -> Vec<ModelId> {
        self.fallbacks.clone()
    }
}

fn ids(v: Vec<ModelId>) -> Vec<String> {
    v.into_iter().map(|m| m.as_str().to_owned()).collect()
}

#[test]
fn test_g6_fallback_chain_when_several_members_should_report_those_after_the_first() {
    let chain = FallbackChain::new(vec![
        fake("anthropic:claude-opus-4-7", &[]),
        fake("openai:gpt-4o", &[]),
        fake("ollama:llama3.2", &[]),
    ]);
    let got = chain.fallback_models(&ModelId::new("anthropic:claude-opus-4-7"));
    assert_eq!(ids(got), ["openai:gpt-4o", "ollama:llama3.2"]);
}

#[test]
fn test_g6_fallback_chain_when_single_member_should_report_none() {
    let chain = FallbackChain::new(vec![fake("anthropic:claude-opus-4-7", &[])]);
    assert!(
        chain
            .fallback_models(&ModelId::new("anthropic:claude-opus-4-7"))
            .is_empty()
    );
}

#[test]
fn test_g7_caching_provider_when_inner_reports_fallbacks_should_forward_them() {
    let inner = fake(
        "anthropic",
        &["openai:gpt-4o", "anthropic:claude-haiku-4-5"],
    );
    let cache = CachingProvider::new(inner, 8);
    let got = cache.fallback_models(&ModelId::new("anthropic:claude-opus-4-7"));
    assert_eq!(ids(got), ["openai:gpt-4o", "anthropic:claude-haiku-4-5"]);
}
