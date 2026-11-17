#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! Spec F3: `ModelProvider::fallback_models(&self, &ModelId) -> Vec<ModelId>`
//! on `ModelDispatch` returns the configured fallbacks it has a provider for,
//! in order, excluding the request's own model (the members its `complete`
//! would chain); none without `with_fallback`.

use std::sync::Arc;

use hone_proto::ModelId;
use hone_providers::{MockProvider, ModelDispatch, ModelProvider};

fn ids(v: Vec<ModelId>) -> Vec<String> {
    v.into_iter().map(|m| m.as_str().to_owned()).collect()
}

fn dispatch() -> ModelDispatch {
    ModelDispatch::new(ModelId::new("anthropic:claude-opus-4-7"))
        .with_provider("anthropic", Arc::new(MockProvider::new(vec![])))
        .with_provider("openai", Arc::new(MockProvider::new(vec![])))
}

#[test]
fn test_fallback_models_when_some_have_no_provider_should_return_only_served_ones() {
    let d = dispatch().with_fallback(vec![
        ModelId::new("openai:gpt-4o"),
        ModelId::new("ollama:llama3.2"), // no provider registered
        ModelId::new("anthropic:claude-haiku-4-5"),
    ]);
    let got = d.fallback_models(&ModelId::new("anthropic:claude-opus-4-7"));
    assert_eq!(
        ids(got),
        ["openai:gpt-4o", "anthropic:claude-haiku-4-5"],
        "order kept, unserved model dropped"
    );
}

#[test]
fn test_fallback_models_when_request_model_is_in_the_list_should_exclude_it() {
    let d = dispatch().with_fallback(vec![
        ModelId::new("openai:gpt-4o"),
        ModelId::new("anthropic:claude-haiku-4-5"),
    ]);
    let got = d.fallback_models(&ModelId::new("openai:gpt-4o"));
    assert_eq!(ids(got), ["anthropic:claude-haiku-4-5"]);
}

#[test]
fn test_fallback_models_when_no_fallback_configured_should_be_empty() {
    let d = dispatch();
    assert!(
        d.fallback_models(&ModelId::new("anthropic:claude-opus-4-7"))
            .is_empty()
    );
}
