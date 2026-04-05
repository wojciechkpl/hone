use std::sync::Arc;

use crate::registry::ProviderRegistry;
use crate::traits::ModelProvider;

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

/// Routes tasks to providers based on ordered matching rules.
///
/// Rules are evaluated in the order they were provided. The first match wins.
/// If no rule matches, the default provider is used.
pub struct ModelRouter {
    rules: Vec<RoutingRule>,
    registry: Arc<ProviderRegistry>,
    default_provider_id: String,
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
        }
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
}
