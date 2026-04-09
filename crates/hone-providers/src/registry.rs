use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use hone_proto::ModelId;

use crate::traits::ModelProvider;

/// Thread-safe registry of model providers, supporting hot-swap.
///
/// Uses `std::sync::RwLock` with poison recovery: if a thread panics while
/// holding the lock, subsequent accesses recover the inner data instead of
/// cascading the panic across every caller.
pub struct ProviderRegistry {
    providers: RwLock<HashMap<String, Arc<dyn ModelProvider>>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
        }
    }

    /// Register a provider. Replaces any existing provider with the same id.
    pub fn register(&self, provider: Arc<dyn ModelProvider>) {
        let id = provider.id().to_owned();
        self.providers
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, provider);
    }

    /// Look up a provider by its stable id.
    pub fn get(&self, id: &str) -> Option<Arc<dyn ModelProvider>> {
        self.providers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    /// Find the provider responsible for the given `ModelId`.
    ///
    /// Matches on the provider portion of the model id (the segment before `:`).
    /// Falls back to a full-string match when no `:` is present.
    pub fn get_by_model(&self, model_id: &ModelId) -> Option<Arc<dyn ModelProvider>> {
        let provider_key = model_id.provider().unwrap_or(model_id.as_str());
        self.get(provider_key)
    }

    /// Atomically replace the provider registered under `old_id` with `new`.
    ///
    /// Does nothing if `old_id` is not currently registered.
    pub fn swap(&self, old_id: &str, new: Arc<dyn ModelProvider>) {
        let mut map = self.providers.write().unwrap_or_else(|e| e.into_inner());
        if map.contains_key(old_id) {
            map.remove(old_id);
            let new_id = new.id().to_owned();
            map.insert(new_id, new);
        }
    }

    /// Return all registered providers in an unspecified order.
    pub fn all(&self) -> Vec<Arc<dyn ModelProvider>> {
        self.providers
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
