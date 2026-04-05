use std::sync::Arc;

use hone_proto::ModelId;
use hone_providers::{MockProvider, ModelProvider, ProviderRegistry};

fn mock(id: &str) -> Arc<dyn ModelProvider> {
    Arc::new(MockProvider::new(vec![]).with_id(id))
}

#[test]
fn test_register_and_get() {
    let registry = ProviderRegistry::new();
    registry.register(mock("alpha"));

    let found = registry.get("alpha");
    assert!(found.is_some());
    assert_eq!(found.unwrap().id(), "alpha");
}

#[test]
fn test_get_by_model_id() {
    let registry = ProviderRegistry::new();
    registry.register(mock("anthropic"));

    let model_id = ModelId::new("anthropic:claude-opus-4-5");
    let found = registry.get_by_model(&model_id);
    assert!(found.is_some());
    assert_eq!(found.unwrap().id(), "anthropic");
}

#[test]
fn test_get_missing_returns_none() {
    let registry = ProviderRegistry::new();
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn test_swap_provider() {
    let registry = ProviderRegistry::new();
    registry.register(mock("old-provider"));

    let replacement = Arc::new(MockProvider::new(vec!["swapped".into()]).with_id("new-provider"));
    registry.swap("old-provider", replacement);

    assert!(registry.get("old-provider").is_none(), "old provider should be gone");
    let found = registry.get("new-provider");
    assert!(found.is_some());
    assert_eq!(found.unwrap().id(), "new-provider");
}

#[test]
fn test_all_returns_all_providers() {
    let registry = ProviderRegistry::new();
    registry.register(mock("p1"));
    registry.register(mock("p2"));
    registry.register(mock("p3"));

    let mut all: Vec<_> = registry.all().iter().map(|p| p.id().to_owned()).collect();
    all.sort();

    assert_eq!(all, vec!["p1", "p2", "p3"]);
}
