#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use hone_providers::{MockProvider, ModelProvider, ModelRouter, ProviderRegistry, RoutingRule};

fn registry_with(ids: &[&str]) -> Arc<ProviderRegistry> {
    let registry = Arc::new(ProviderRegistry::new());
    for &id in ids {
        let provider: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec![]).with_id(id));
        registry.register(provider);
    }
    registry
}

#[test]
fn test_router_selects_by_task() {
    let registry = registry_with(&["fast", "deep"]);
    let rules = vec![RoutingRule::new("planning", "deep")];
    let router = ModelRouter::new(rules, registry, "fast");

    let provider = router.route("I need help planning a project").unwrap();
    assert_eq!(provider.id(), "deep");
}

#[test]
fn test_router_default_fallback() {
    let registry = registry_with(&["fast", "deep"]);
    let rules = vec![RoutingRule::new("planning", "deep")];
    let router = ModelRouter::new(rules, registry, "fast");

    // Task that doesn't match any rule.
    let provider = router.route("write me a haiku").unwrap();
    assert_eq!(provider.id(), "fast");
}

#[test]
fn test_router_first_matching_rule_wins() {
    let registry = registry_with(&["fast", "deep", "code"]);
    let rules = vec![
        RoutingRule::new("code", "code"),
        RoutingRule::new("refactor", "deep"),
    ];
    let router = ModelRouter::new(rules, registry, "fast");

    // "refactor code" matches "code" rule first.
    let provider = router.route("refactor code").unwrap();
    assert_eq!(provider.id(), "code");
}

#[test]
fn test_router_returns_none_when_no_provider_registered() {
    let registry = Arc::new(ProviderRegistry::new());
    let router = ModelRouter::new(vec![], registry, "missing");

    assert!(router.route("anything").is_none());
}
