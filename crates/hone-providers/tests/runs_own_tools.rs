//! P2.3 spec section 5: `ModelProvider::runs_own_tools()`.
//!
//! Default is `false`; adapters whose backend executes tools itself (and so
//! bypasses the permission gate) return `true`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use hone_providers::{
    AnthropicProvider, CachingProvider, ClaudeCliProvider, FallbackChain, GeminiCliProvider,
    MockProvider, ModelProvider, ModelRouter, ProviderRegistry, ProviderTier, RoutePolicy,
    TierFallbackProvider,
};

#[test]
fn test_runs_own_tools_when_mock_provider_should_default_false() {
    let p = MockProvider::new(vec!["hi".to_owned()]);
    assert!(!p.runs_own_tools());
}

#[test]
fn test_runs_own_tools_when_anthropic_api_provider_should_be_false() {
    let p = AnthropicProvider::new("test-key".to_owned());
    assert!(!p.runs_own_tools());
}

#[test]
fn test_runs_own_tools_when_claude_cli_adapter_should_be_true() {
    let p = ClaudeCliProvider::new();
    assert!(p.runs_own_tools());
}

#[test]
fn test_runs_own_tools_when_claude_cli_custom_binary_should_be_true() {
    let p = ClaudeCliProvider::with_binary("/nonexistent/claude");
    assert!(p.runs_own_tools());
}

#[test]
fn test_runs_own_tools_should_be_callable_through_dyn_trait() {
    let p: Box<dyn ModelProvider> = Box::new(ClaudeCliProvider::new());
    assert!(p.runs_own_tools());
}

#[test]
fn test_runs_own_tools_when_gemini_cli_adapter_should_be_true() {
    // Gemini CLI has built-in tools we cannot verify it never runs: fail closed.
    let p = GeminiCliProvider::new();
    assert!(p.runs_own_tools());
}

fn tier(id: &str) -> ProviderTier {
    ProviderTier {
        provider_id: id.to_owned(),
        model: None,
        cost_per_1k_input_usd: 0.001,
        latency_p50_ms: 100,
        quality_score: 0.5,
    }
}

fn tier_fallback(
    primary: Arc<dyn ModelProvider>,
    others: Vec<Arc<dyn ModelProvider>>,
) -> TierFallbackProvider {
    let registry = Arc::new(ProviderRegistry::new());
    let mut tiers = vec![tier(primary.id())];
    registry.register(Arc::clone(&primary));
    for o in others {
        tiers.push(tier(o.id()));
        registry.register(o);
    }
    let router = Arc::new(ModelRouter::new(vec![], registry, primary.id()).with_tiers(tiers));
    TierFallbackProvider::new(primary, router, RoutePolicy::Cheapest)
}

#[test]
fn test_runs_own_tools_when_tier_fallback_secondary_tier_runs_tools_should_be_true() {
    let primary: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new(vec!["x".to_owned()]).with_id("primary"));
    let secondary: Arc<dyn ModelProvider> = Arc::new(ClaudeCliProvider::new());
    assert!(!primary.runs_own_tools());
    let p = tier_fallback(primary, vec![secondary]);
    assert!(
        p.runs_own_tools(),
        "a fallback hop could land on a provider that runs its own tools"
    );
}

#[test]
fn test_runs_own_tools_when_tier_fallback_has_no_tool_running_tier_should_be_false() {
    let primary: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new(vec!["x".to_owned()]).with_id("primary"));
    let other: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new(vec!["y".to_owned()]).with_id("other"));
    let p = tier_fallback(primary, vec![other]);
    assert!(!p.runs_own_tools());
}

#[test]
fn test_runs_own_tools_when_tier_fallback_primary_runs_tools_should_be_true() {
    let primary: Arc<dyn ModelProvider> = Arc::new(ClaudeCliProvider::new());
    let other: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new(vec!["y".to_owned()]).with_id("other"));
    assert!(tier_fallback(primary, vec![other]).runs_own_tools());
}

#[test]
fn test_runs_own_tools_when_fallback_chain_any_member_runs_tools_should_be_true() {
    let a: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec!["x".to_owned()]));
    let b: Arc<dyn ModelProvider> = Arc::new(ClaudeCliProvider::new());
    assert!(FallbackChain::new(vec![a, b]).runs_own_tools());
}

#[test]
fn test_runs_own_tools_when_caching_provider_wraps_tool_runner_should_forward() {
    let inner: Arc<dyn ModelProvider> = Arc::new(ClaudeCliProvider::new());
    assert!(CachingProvider::new(inner, 8).runs_own_tools());
    let plain: Arc<dyn ModelProvider> = Arc::new(MockProvider::new(vec!["x".to_owned()]));
    assert!(!CachingProvider::new(plain, 8).runs_own_tools());
}
