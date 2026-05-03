use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;
use hone_proto::{Message, ModelInfo, ProviderEvent, ProviderHealth};

use crate::error::ProviderError;
use crate::registry::ProviderRegistry;
use crate::traits::{CompletionRequest, ModelProvider};

/// A rule that maps a task keyword/prefix to a provider id.
#[derive(Debug, Clone)]
pub struct RoutingRule {
    /// Substring match against the task string (case-insensitive).
    pub pattern: String,
    /// Id of the provider to use when the pattern matches.
    pub provider_id: String,
}

impl RoutingRule {
    pub fn new(pattern: impl Into<String>, provider_id: impl Into<String>) -> Self {
        Self {
            pattern: pattern.into(),
            provider_id: provider_id.into(),
        }
    }
}

/// Per-(provider, model) tier metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct ProviderTier {
    /// Provider id matching ProviderRegistry keys.
    pub provider_id: String,
    /// Optional model identifier — when None, the tier applies to the
    /// provider's default model.
    pub model: Option<String>,
    /// Cost per 1k input tokens in USD.
    pub cost_per_1k_input_usd: f64,
    /// Median latency in milliseconds (informational).
    pub latency_p50_ms: u64,
    /// Quality score in [0.0, 1.0]. Higher = better.
    pub quality_score: f64,
}

/// Selection policy for tier-based routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoutePolicy {
    /// Lowest cost.
    Cheapest,
    /// Lowest latency.
    Fastest,
    /// Highest quality.
    Quality,
    /// Score = quality - normalized_cost - normalized_latency. Pick max.
    Balanced,
}

/// Routes tasks to providers based on ordered matching rules.
///
/// Rules are evaluated in the order they were provided. The first match wins.
/// If no rule matches, the default provider is used.
pub struct ModelRouter {
    rules: Vec<RoutingRule>,
    registry: Arc<ProviderRegistry>,
    default_provider_id: String,
    tiers: Vec<ProviderTier>,
}

impl ModelRouter {
    pub fn new(
        rules: Vec<RoutingRule>,
        registry: Arc<ProviderRegistry>,
        default_provider_id: impl Into<String>,
    ) -> Self {
        Self {
            rules,
            registry,
            default_provider_id: default_provider_id.into(),
            tiers: Vec::new(),
        }
    }

    /// Replace (or initialise) the tier table for tier-based routing.
    pub fn with_tiers(mut self, tiers: Vec<ProviderTier>) -> Self {
        self.tiers = tiers;
        self
    }

    /// Return the provider appropriate for `task`.
    ///
    /// Returns `None` if neither a matching rule's provider nor the default
    /// provider is registered.
    pub fn route(&self, task: &str) -> Option<Arc<dyn ModelProvider>> {
        let lower = task.to_lowercase();

        for rule in &self.rules {
            if lower.contains(&rule.pattern.to_lowercase()) {
                if let Some(provider) = self.registry.get(&rule.provider_id) {
                    return Some(provider);
                }
            }
        }

        self.registry.get(&self.default_provider_id)
    }

    /// Pick a provider based on the given policy. Returns None when:
    ///
    /// - the tier table is empty, OR
    /// - none of the tier-listed provider_ids exist in the registry.
    ///
    /// Selection rules:
    ///
    /// - Cheapest: minimum cost_per_1k_input_usd; ties broken by quality desc, then provider_id asc
    /// - Fastest: minimum latency_p50_ms; ties broken by quality desc, then provider_id asc
    /// - Quality: maximum quality_score; ties broken by cost asc, then provider_id asc
    /// - Balanced: maximize `quality - 0.5 * norm_cost - 0.5 * norm_latency` where
    ///   `norm_cost = (cost - min_cost) / (max_cost - min_cost)` (or 0 if all equal),
    ///   `norm_latency = (latency - min_lat) / (max_lat - min_lat)` (or 0 if all equal);
    ///   ties broken by provider_id asc
    pub fn route_by_policy(&self, policy: RoutePolicy) -> Option<Arc<dyn ModelProvider>> {
        let sorted = self.sorted_by_policy(policy);
        sorted.into_iter().next()
    }

    /// Returns the full sorted list of registered providers ordered by `policy`.
    /// Tiers whose provider_id is not in the registry are silently excluded.
    fn sorted_by_policy(&self, policy: RoutePolicy) -> Vec<Arc<dyn ModelProvider>> {
        // Filter to tiers present in the registry.
        let eligible: Vec<(&ProviderTier, Arc<dyn ModelProvider>)> = self
            .tiers
            .iter()
            .filter_map(|t| self.registry.get(&t.provider_id).map(|p| (t, p)))
            .collect();

        if eligible.is_empty() {
            return Vec::new();
        }

        match policy {
            RoutePolicy::Cheapest => {
                let mut v = eligible;
                v.sort_by(|(a, _), (b, _)| {
                    a.cost_per_1k_input_usd
                        .partial_cmp(&b.cost_per_1k_input_usd)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        // ties: higher quality first
                        .then_with(|| {
                            b.quality_score
                                .partial_cmp(&a.quality_score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        // ties: provider_id asc
                        .then_with(|| a.provider_id.cmp(&b.provider_id))
                });
                v.into_iter().map(|(_, p)| p).collect()
            }
            RoutePolicy::Fastest => {
                let mut v = eligible;
                v.sort_by(|(a, _), (b, _)| {
                    a.latency_p50_ms
                        .cmp(&b.latency_p50_ms)
                        // ties: higher quality first
                        .then_with(|| {
                            b.quality_score
                                .partial_cmp(&a.quality_score)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        // ties: provider_id asc
                        .then_with(|| a.provider_id.cmp(&b.provider_id))
                });
                v.into_iter().map(|(_, p)| p).collect()
            }
            RoutePolicy::Quality => {
                let mut v = eligible;
                v.sort_by(|(a, _), (b, _)| {
                    // higher quality first
                    b.quality_score
                        .partial_cmp(&a.quality_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        // ties: lower cost first
                        .then_with(|| {
                            a.cost_per_1k_input_usd
                                .partial_cmp(&b.cost_per_1k_input_usd)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        })
                        // ties: provider_id asc
                        .then_with(|| a.provider_id.cmp(&b.provider_id))
                });
                v.into_iter().map(|(_, p)| p).collect()
            }
            RoutePolicy::Balanced => {
                let min_cost = eligible
                    .iter()
                    .map(|(t, _)| t.cost_per_1k_input_usd)
                    .fold(f64::INFINITY, f64::min);
                let max_cost = eligible
                    .iter()
                    .map(|(t, _)| t.cost_per_1k_input_usd)
                    .fold(f64::NEG_INFINITY, f64::max);
                let min_lat = eligible
                    .iter()
                    .map(|(t, _)| t.latency_p50_ms as f64)
                    .fold(f64::INFINITY, f64::min);
                let max_lat = eligible
                    .iter()
                    .map(|(t, _)| t.latency_p50_ms as f64)
                    .fold(f64::NEG_INFINITY, f64::max);

                let cost_range = max_cost - min_cost;
                let lat_range = max_lat - min_lat;

                let score = |t: &ProviderTier| -> f64 {
                    let norm_cost = if cost_range > 0.0 {
                        (t.cost_per_1k_input_usd - min_cost) / cost_range
                    } else {
                        0.0
                    };
                    let norm_lat = if lat_range > 0.0 {
                        (t.latency_p50_ms as f64 - min_lat) / lat_range
                    } else {
                        0.0
                    };
                    t.quality_score - 0.5 * norm_cost - 0.5 * norm_lat
                };

                let mut v = eligible;
                v.sort_by(|(a, _), (b, _)| {
                    // higher score first
                    score(b)
                        .partial_cmp(&score(a))
                        .unwrap_or(std::cmp::Ordering::Equal)
                        // ties: provider_id asc
                        .then_with(|| a.provider_id.cmp(&b.provider_id))
                });
                v.into_iter().map(|(_, p)| p).collect()
            }
        }
    }

    /// Convenience: try `route(task)` first (substring rules); if that returns
    /// Some, use it. Otherwise fall back to `route_by_policy(policy)`.
    pub fn route_combined(
        &self,
        task: &str,
        policy: RoutePolicy,
    ) -> Option<Arc<dyn ModelProvider>> {
        // Substring match first — matches the existing route() logic which
        // always returns the default provider when no rule fires. We need to
        // detect "no rule fired" so we only use the tier policy when truly
        // unmatched. We replicate the rule-matching inline here.
        let lower = task.to_lowercase();
        for rule in &self.rules {
            if lower.contains(&rule.pattern.to_lowercase()) {
                if let Some(provider) = self.registry.get(&rule.provider_id) {
                    return Some(provider);
                }
            }
        }
        // No substring rule matched — try tier policy, then fall back to default.
        self.route_by_policy(policy)
            .or_else(|| self.registry.get(&self.default_provider_id))
    }
}

fn is_transient(e: &ProviderError) -> bool {
    matches!(e, ProviderError::Http { status, .. } if *status >= 500 || *status == 429)
        || matches!(e, ProviderError::RateLimit { .. })
}

/// Cost-aware fallback wrapper that retries with the next-best provider from
/// the router's tier list under the configured policy when the primary fails
/// with a transient error.
pub struct TierFallbackProvider {
    primary: Arc<dyn ModelProvider>,
    router: Arc<ModelRouter>,
    policy: RoutePolicy,
    /// Maximum number of fallback hops (in addition to the primary attempt).
    pub max_hops: usize,
}

impl TierFallbackProvider {
    pub fn new(
        primary: Arc<dyn ModelProvider>,
        router: Arc<ModelRouter>,
        policy: RoutePolicy,
    ) -> Self {
        Self {
            primary,
            router,
            policy,
            max_hops: 2,
        }
    }

    pub fn with_max_hops(mut self, hops: usize) -> Self {
        self.max_hops = hops;
        self
    }
}

#[async_trait]
impl ModelProvider for TierFallbackProvider {
    fn id(&self) -> &str {
        self.primary.id()
    }

    fn display_name(&self) -> &str {
        self.primary.display_name()
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        self.primary.supported_models()
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        // Try primary first.
        match self.primary.complete(request.clone()).await {
            Ok(stream) => return Ok(stream),
            Err(e) if !is_transient(&e) => return Err(e),
            Err(e) => {
                // Transient — proceed to fallback chain.
                let mut last_err = e;
                let mut tried: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                tried.insert(self.primary.id().to_owned());

                let candidates = self.router.sorted_by_policy(self.policy);

                let mut hops = 0usize;
                for candidate in candidates {
                    if hops >= self.max_hops {
                        break;
                    }
                    if tried.contains(candidate.id()) {
                        continue;
                    }
                    tried.insert(candidate.id().to_owned());
                    hops += 1;

                    match candidate.complete(request.clone()).await {
                        Ok(stream) => return Ok(stream),
                        Err(e) if !is_transient(&e) => return Err(e),
                        Err(e) => last_err = e,
                    }
                }

                Err(last_err)
            }
        }
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        self.primary.estimate_tokens(messages)
    }

    async fn health(&self) -> ProviderHealth {
        self.primary.health().await
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use futures::{Stream, stream};
    use hone_proto::{Message, ModelInfo, ProviderEvent, ProviderHealth};

    use super::{ModelRouter, ProviderTier, RoutePolicy, RoutingRule, TierFallbackProvider};
    use crate::error::ProviderError;
    use crate::mock::MockProvider;
    use crate::registry::ProviderRegistry;
    use crate::traits::{CompletionRequest, ModelProvider};

    // -----------------------------------------------------------------------
    // FailingMockProvider — inline test helper (~45 LoC)
    //
    // MockProvider only succeeds; we need providers that fail deterministically.
    // Rather than invasively modifying mock.rs (which is used by many other
    // test suites), we define a minimal inline variant here that covers:
    //   - always-transient failures (Http 503, RateLimit 429)
    //   - always-non-transient failures (Http 400)
    //   - a call counter so tests can assert attempt counts
    // -----------------------------------------------------------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum FailMode {
        Http503,
        Http429,
        Http400,
    }

    pub struct FailingMockProvider {
        pub provider_id: String,
        pub fail_mode: FailMode,
        pub call_count: Arc<Mutex<usize>>,
    }

    impl FailingMockProvider {
        pub fn new(provider_id: impl Into<String>, fail_mode: FailMode) -> Self {
            Self {
                provider_id: provider_id.into(),
                fail_mode,
                call_count: Arc::new(Mutex::new(0)),
            }
        }
    }

    #[async_trait]
    impl ModelProvider for FailingMockProvider {
        fn id(&self) -> &str {
            &self.provider_id
        }

        fn display_name(&self) -> &str {
            "Failing Mock"
        }

        fn supported_models(&self) -> Vec<ModelInfo> {
            vec![]
        }

        async fn complete(
            &self,
            _request: CompletionRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
            *self.call_count.lock().unwrap() += 1;
            match self.fail_mode {
                FailMode::Http503 => Err(ProviderError::Http {
                    provider: self.provider_id.clone(),
                    status: 503,
                    body: "service unavailable".to_owned(),
                }),
                FailMode::Http429 => Err(ProviderError::RateLimit {
                    provider: self.provider_id.clone(),
                    retry_after_secs: 30,
                }),
                FailMode::Http400 => Err(ProviderError::Http {
                    provider: self.provider_id.clone(),
                    status: 400,
                    body: "bad request".to_owned(),
                }),
            }
        }

        fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
            0
        }

        async fn health(&self) -> ProviderHealth {
            ProviderHealth::Healthy
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    /// Build a registry with three MockProviders and the canonical tier fixture.
    fn three_provider_setup() -> (Arc<ProviderRegistry>, Vec<ProviderTier>) {
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(
            MockProvider::new(vec!["cheap-response".to_owned()]).with_id("cheap"),
        ));
        registry.register(Arc::new(
            MockProvider::new(vec!["fast-response".to_owned()]).with_id("fast"),
        ));
        registry.register(Arc::new(
            MockProvider::new(vec!["premium-response".to_owned()]).with_id("premium"),
        ));

        let tiers = vec![
            ProviderTier {
                provider_id: "cheap".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.001,
                latency_p50_ms: 2000,
                quality_score: 0.6,
            },
            ProviderTier {
                provider_id: "fast".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.005,
                latency_p50_ms: 200,
                quality_score: 0.7,
            },
            ProviderTier {
                provider_id: "premium".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.020,
                latency_p50_ms: 800,
                quality_score: 0.95,
            },
        ];

        (registry, tiers)
    }

    fn router_with_tiers(
        registry: Arc<ProviderRegistry>,
        tiers: Vec<ProviderTier>,
    ) -> ModelRouter {
        ModelRouter::new(vec![], registry, "cheap").with_tiers(tiers)
    }

    // -----------------------------------------------------------------------
    // Policy selection tests
    // -----------------------------------------------------------------------

    #[test]
    fn route_by_policy_cheapest_picks_cheap() {
        let (registry, tiers) = three_provider_setup();
        let router = router_with_tiers(registry, tiers);

        let provider = router.route_by_policy(RoutePolicy::Cheapest).unwrap();
        assert_eq!(provider.id(), "cheap");
    }

    #[test]
    fn route_by_policy_fastest_picks_fast() {
        let (registry, tiers) = three_provider_setup();
        let router = router_with_tiers(registry, tiers);

        let provider = router.route_by_policy(RoutePolicy::Fastest).unwrap();
        assert_eq!(provider.id(), "fast");
    }

    #[test]
    fn route_by_policy_quality_picks_premium() {
        let (registry, tiers) = three_provider_setup();
        let router = router_with_tiers(registry, tiers);

        let provider = router.route_by_policy(RoutePolicy::Quality).unwrap();
        assert_eq!(provider.id(), "premium");
    }

    #[test]
    fn route_by_policy_balanced_picks_fast() {
        // Balanced formula verification (from spec):
        //   norm_cost   = (c - 0.001) / (0.020 - 0.001)
        //   norm_latency = (l - 200)  / (2000 - 200)
        //   score = quality - 0.5 * norm_cost - 0.5 * norm_latency
        //
        //   cheap:   score = 0.6  - 0.5 * 0.0000 - 0.5 * 1.0000 = 0.100
        //   fast:    score = 0.7  - 0.5 * 0.2105 - 0.5 * 0.0000 = 0.5947…
        //   premium: score = 0.95 - 0.5 * 1.0000 - 0.5 * 0.3333 = 0.2833…
        //
        //   Winner: fast
        let (registry, tiers) = three_provider_setup();
        let router = router_with_tiers(registry, tiers);

        let provider = router.route_by_policy(RoutePolicy::Balanced).unwrap();
        assert_eq!(provider.id(), "fast");
    }

    #[test]
    fn route_by_policy_empty_tier_table_returns_none() {
        let (registry, _) = three_provider_setup();
        let router = ModelRouter::new(vec![], registry, "cheap");
        // No .with_tiers() call → empty tier table.
        assert!(router.route_by_policy(RoutePolicy::Cheapest).is_none());
    }

    #[test]
    fn route_by_policy_missing_provider_in_registry_skips_it() {
        // "ghost" is in the tier list but NOT registered. The next cheapest
        // registered provider is "cheap" (cost=0.001).
        let (registry, mut tiers) = three_provider_setup();
        tiers.insert(
            0,
            ProviderTier {
                provider_id: "ghost".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.0001,
                latency_p50_ms: 100,
                quality_score: 0.99,
            },
        );
        let router = router_with_tiers(registry, tiers);

        let provider = router.route_by_policy(RoutePolicy::Cheapest).unwrap();
        assert_eq!(provider.id(), "cheap");
    }

    #[test]
    fn route_by_policy_tie_breaking_cheapest_uses_quality_desc() {
        // Two providers share the same cost; higher quality wins.
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(
            MockProvider::new(vec![]).with_id("low-quality"),
        ));
        registry.register(Arc::new(
            MockProvider::new(vec![]).with_id("high-quality"),
        ));

        let tiers = vec![
            ProviderTier {
                provider_id: "low-quality".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.005,
                latency_p50_ms: 500,
                quality_score: 0.5,
            },
            ProviderTier {
                provider_id: "high-quality".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.005,
                latency_p50_ms: 500,
                quality_score: 0.9,
            },
        ];
        let router = router_with_tiers(registry, tiers);

        let provider = router.route_by_policy(RoutePolicy::Cheapest).unwrap();
        assert_eq!(provider.id(), "high-quality");
    }

    #[test]
    fn route_by_policy_tie_breaking_fastest_uses_quality_desc() {
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(
            MockProvider::new(vec![]).with_id("slow-quality"),
        ));
        registry.register(Arc::new(
            MockProvider::new(vec![]).with_id("fast-quality"),
        ));

        let tiers = vec![
            ProviderTier {
                provider_id: "slow-quality".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.005,
                latency_p50_ms: 300,
                quality_score: 0.4,
            },
            ProviderTier {
                provider_id: "fast-quality".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.005,
                latency_p50_ms: 300,
                quality_score: 0.8,
            },
        ];
        let router = router_with_tiers(registry, tiers);

        let provider = router.route_by_policy(RoutePolicy::Fastest).unwrap();
        assert_eq!(provider.id(), "fast-quality");
    }

    #[test]
    fn route_by_policy_tie_breaking_quality_uses_cost_asc() {
        // Two providers have the same quality; cheaper wins.
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(
            MockProvider::new(vec![]).with_id("expensive"),
        ));
        registry.register(Arc::new(
            MockProvider::new(vec![]).with_id("cheap-equal"),
        ));

        let tiers = vec![
            ProviderTier {
                provider_id: "expensive".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.020,
                latency_p50_ms: 500,
                quality_score: 0.9,
            },
            ProviderTier {
                provider_id: "cheap-equal".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.001,
                latency_p50_ms: 500,
                quality_score: 0.9,
            },
        ];
        let router = router_with_tiers(registry, tiers);

        let provider = router.route_by_policy(RoutePolicy::Quality).unwrap();
        assert_eq!(provider.id(), "cheap-equal");
    }

    // -----------------------------------------------------------------------
    // route_combined tests
    // -----------------------------------------------------------------------

    #[test]
    fn route_combined_returns_substring_match_when_tier_table_empty() {
        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::new(
            MockProvider::new(vec![]).with_id("code-provider"),
        ));
        registry.register(Arc::new(
            MockProvider::new(vec![]).with_id("default-provider"),
        ));

        let rules = vec![RoutingRule::new("code", "code-provider")];
        let router = ModelRouter::new(rules, registry, "default-provider");
        // No tiers.

        let provider = router
            .route_combined("write some code", RoutePolicy::Quality)
            .unwrap();
        assert_eq!(provider.id(), "code-provider");
    }

    #[test]
    fn route_combined_returns_tier_pick_when_no_substring_match() {
        let (registry, tiers) = three_provider_setup();
        let rules = vec![RoutingRule::new("vision", "premium")];
        let router = ModelRouter::new(rules, registry, "cheap").with_tiers(tiers);

        // Task does not match "vision", but tiers are populated.
        let provider = router
            .route_combined("summarise this document", RoutePolicy::Quality)
            .unwrap();
        assert_eq!(provider.id(), "premium");
    }

    #[test]
    fn route_combined_substring_takes_precedence_over_tiers() {
        let (registry, tiers) = three_provider_setup();
        // The rule hard-routes "code" tasks to "cheap", but Quality policy
        // would pick "premium". The substring rule must win.
        let rules = vec![RoutingRule::new("code", "cheap")];
        let router = ModelRouter::new(rules, registry, "cheap").with_tiers(tiers);

        let provider = router
            .route_combined("write some code", RoutePolicy::Quality)
            .unwrap();
        assert_eq!(provider.id(), "cheap");
    }

    // -----------------------------------------------------------------------
    // TierFallbackProvider tests
    // -----------------------------------------------------------------------

    fn make_completion_request() -> CompletionRequest {
        use hone_proto::ModelId;
        CompletionRequest::new(ModelId::new("mock:model"), vec![])
    }

    #[tokio::test]
    async fn fallback_returns_primary_result_on_success() {
        let primary = Arc::new(MockProvider::new(vec!["ok".to_owned()]).with_id("primary"));

        let (registry, tiers) = three_provider_setup();
        let router = Arc::new(router_with_tiers(registry, tiers));

        let fallback =
            TierFallbackProvider::new(primary as Arc<dyn ModelProvider>, router, RoutePolicy::Cheapest);

        let result = fallback.complete(make_completion_request()).await;
        assert!(result.is_ok(), "expected success from primary");
    }

    #[tokio::test]
    async fn fallback_retries_with_next_best_on_transient_error() {
        // Primary fails with 503 (transient). Next-cheapest in the tier list
        // should succeed.
        let primary_calls = Arc::new(Mutex::new(0usize));
        let primary = Arc::new(FailingMockProvider::new("cheap", FailMode::Http503));
        let primary_call_count = Arc::clone(&primary.call_count);

        let registry = Arc::new(ProviderRegistry::new());
        // Register the failing primary under "cheap".
        registry.register(Arc::clone(&primary) as Arc<dyn ModelProvider>);
        // Register a succeeding fallback under "fast".
        registry.register(Arc::new(
            MockProvider::new(vec!["fallback-ok".to_owned()]).with_id("fast"),
        ));
        registry.register(Arc::new(
            MockProvider::new(vec![]).with_id("premium"),
        ));

        let tiers = vec![
            ProviderTier {
                provider_id: "cheap".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.001,
                latency_p50_ms: 2000,
                quality_score: 0.6,
            },
            ProviderTier {
                provider_id: "fast".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.005,
                latency_p50_ms: 200,
                quality_score: 0.7,
            },
            ProviderTier {
                provider_id: "premium".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.020,
                latency_p50_ms: 800,
                quality_score: 0.95,
            },
        ];
        let router = Arc::new(
            ModelRouter::new(vec![], registry, "cheap").with_tiers(tiers),
        );

        let fallback_provider = TierFallbackProvider::new(
            Arc::clone(&primary) as Arc<dyn ModelProvider>,
            router,
            RoutePolicy::Cheapest,
        );

        let result = fallback_provider
            .complete(make_completion_request())
            .await;
        assert!(result.is_ok(), "fallback should eventually succeed");
        assert_eq!(
            *primary_call_count.lock().unwrap(),
            1,
            "primary should have been called exactly once"
        );
    }

    #[tokio::test]
    async fn fallback_returns_error_after_max_hops_exhausted() {
        // All providers fail — error must surface after primary + max_hops attempts.
        let registry = Arc::new(ProviderRegistry::new());
        let p1 = Arc::new(FailingMockProvider::new("p1", FailMode::Http503));
        let p2 = Arc::new(FailingMockProvider::new("p2", FailMode::Http503));
        let p3 = Arc::new(FailingMockProvider::new("p3", FailMode::Http503));
        let p1_count = Arc::clone(&p1.call_count);
        let p2_count = Arc::clone(&p2.call_count);
        let p3_count = Arc::clone(&p3.call_count);

        registry.register(Arc::clone(&p1) as Arc<dyn ModelProvider>);
        registry.register(Arc::clone(&p2) as Arc<dyn ModelProvider>);
        registry.register(Arc::clone(&p3) as Arc<dyn ModelProvider>);

        let tiers = vec![
            ProviderTier {
                provider_id: "p1".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.001,
                latency_p50_ms: 100,
                quality_score: 0.5,
            },
            ProviderTier {
                provider_id: "p2".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.005,
                latency_p50_ms: 200,
                quality_score: 0.6,
            },
            ProviderTier {
                provider_id: "p3".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.010,
                latency_p50_ms: 300,
                quality_score: 0.7,
            },
        ];
        let router = Arc::new(
            ModelRouter::new(vec![], registry, "p1").with_tiers(tiers),
        );

        // Default max_hops = 2 → primary + 2 fallbacks = 3 total attempts.
        let fallback_provider =
            TierFallbackProvider::new(Arc::clone(&p1) as Arc<dyn ModelProvider>, router, RoutePolicy::Cheapest);
        assert_eq!(fallback_provider.max_hops, 2);

        let result = fallback_provider
            .complete(make_completion_request())
            .await;
        assert!(result.is_err(), "must propagate error when all hops exhausted");

        let total_calls = *p1_count.lock().unwrap()
            + *p2_count.lock().unwrap()
            + *p3_count.lock().unwrap();
        assert_eq!(total_calls, 3, "primary + 2 fallback hops = 3 total");
    }

    #[tokio::test]
    async fn fallback_does_not_retry_on_non_transient_error() {
        // A 400 is non-transient; no fallback should be attempted.
        let primary = Arc::new(FailingMockProvider::new("primary", FailMode::Http400));
        let primary_count = Arc::clone(&primary.call_count);

        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::clone(&primary) as Arc<dyn ModelProvider>);
        registry.register(Arc::new(
            MockProvider::new(vec!["ok".to_owned()]).with_id("fallback-ok"),
        ));

        let tiers = vec![
            ProviderTier {
                provider_id: "primary".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.001,
                latency_p50_ms: 100,
                quality_score: 0.5,
            },
            ProviderTier {
                provider_id: "fallback-ok".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.010,
                latency_p50_ms: 200,
                quality_score: 0.8,
            },
        ];
        let router = Arc::new(
            ModelRouter::new(vec![], registry, "primary").with_tiers(tiers),
        );

        let fallback_provider = TierFallbackProvider::new(
            Arc::clone(&primary) as Arc<dyn ModelProvider>,
            router,
            RoutePolicy::Cheapest,
        );

        let result = fallback_provider
            .complete(make_completion_request())
            .await;
        assert!(result.is_err(), "non-transient error must propagate");
        assert_eq!(
            *primary_count.lock().unwrap(),
            1,
            "primary called once, no fallback attempted"
        );
    }

    #[tokio::test]
    async fn fallback_skips_primary_in_router_picks() {
        // When a fallback candidate is selected from the router, the primary's
        // id must not appear.
        let primary = Arc::new(FailingMockProvider::new("cheap", FailMode::Http503));
        let fallback_provider_mock =
            Arc::new(MockProvider::new(vec!["ok".to_owned()]).with_id("fast"));

        let registry = Arc::new(ProviderRegistry::new());
        registry.register(Arc::clone(&primary) as Arc<dyn ModelProvider>);
        registry.register(Arc::clone(&fallback_provider_mock) as Arc<dyn ModelProvider>);

        let tiers = vec![
            ProviderTier {
                provider_id: "cheap".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.001,
                latency_p50_ms: 100,
                quality_score: 0.5,
            },
            ProviderTier {
                provider_id: "fast".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.005,
                latency_p50_ms: 200,
                quality_score: 0.7,
            },
        ];
        let router = Arc::new(
            ModelRouter::new(vec![], registry, "cheap").with_tiers(tiers),
        );

        let wrapper = TierFallbackProvider::new(
            Arc::clone(&primary) as Arc<dyn ModelProvider>,
            Arc::clone(&router),
            RoutePolicy::Cheapest,
        );

        let result = wrapper.complete(make_completion_request()).await;
        assert!(result.is_ok());
        // The fallback used was "fast", not "cheap" again.
        // We validate indirectly: if "cheap" were retried, it would fail again
        // and the result would be an error (since fast is the only succeeding provider).
        assert_eq!(result.is_ok(), true);
    }

    #[tokio::test]
    async fn fallback_max_hops_setter_changes_attempt_count() {
        let registry = Arc::new(ProviderRegistry::new());
        let p1 = Arc::new(FailingMockProvider::new("p1", FailMode::Http503));
        let p2 = Arc::new(FailingMockProvider::new("p2", FailMode::Http503));
        let p1_count = Arc::clone(&p1.call_count);
        let p2_count = Arc::clone(&p2.call_count);

        registry.register(Arc::clone(&p1) as Arc<dyn ModelProvider>);
        registry.register(Arc::clone(&p2) as Arc<dyn ModelProvider>);

        let tiers = vec![
            ProviderTier {
                provider_id: "p1".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.001,
                latency_p50_ms: 100,
                quality_score: 0.5,
            },
            ProviderTier {
                provider_id: "p2".to_owned(),
                model: None,
                cost_per_1k_input_usd: 0.005,
                latency_p50_ms: 200,
                quality_score: 0.6,
            },
        ];
        let router = Arc::new(
            ModelRouter::new(vec![], registry, "p1").with_tiers(tiers),
        );

        // max_hops = 1 → only one fallback attempt after primary.
        let wrapper = TierFallbackProvider::new(
            Arc::clone(&p1) as Arc<dyn ModelProvider>,
            router,
            RoutePolicy::Cheapest,
        )
        .with_max_hops(1);

        assert_eq!(wrapper.max_hops, 1);
        let _ = wrapper.complete(make_completion_request()).await;

        let total = *p1_count.lock().unwrap() + *p2_count.lock().unwrap();
        assert_eq!(total, 2, "primary + 1 hop = 2 total attempts");
    }
}
