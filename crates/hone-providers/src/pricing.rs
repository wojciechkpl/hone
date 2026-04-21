use std::collections::HashMap;

use hone_proto::{ModelId, PricingEntry};

/// A registry of per-model pricing data.
pub struct PricingTable {
    entries: HashMap<String, PricingEntry>,
}

impl PricingTable {
    /// Build a table pre-populated with known model prices.
    pub fn default_table() -> Self {
        let mut table = Self {
            entries: HashMap::new(),
        };

        // Anthropic models (Claude 4.x)
        table.insert(PricingEntry {
            model: ModelId::new("anthropic:claude-opus-4-7"),
            input_per_million: 15.0,
            output_per_million: 75.0,
            cache_read_per_million: Some(1.50),
            cache_creation_per_million: Some(18.75),
        });
        table.insert(PricingEntry {
            model: ModelId::new("anthropic:claude-sonnet-4-6"),
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: Some(0.30),
            cache_creation_per_million: Some(3.75),
        });
        table.insert(PricingEntry {
            model: ModelId::new("anthropic:claude-haiku-4-5-20251001"),
            input_per_million: 0.80,
            output_per_million: 4.0,
            cache_read_per_million: Some(0.08),
            cache_creation_per_million: Some(1.00),
        });
        // Claude 4.5 generation (still in service)
        table.insert(PricingEntry {
            model: ModelId::new("anthropic:claude-opus-4-5"),
            input_per_million: 15.0,
            output_per_million: 75.0,
            cache_read_per_million: Some(1.50),
            cache_creation_per_million: Some(18.75),
        });
        table.insert(PricingEntry {
            model: ModelId::new("anthropic:claude-sonnet-4-5"),
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: Some(0.30),
            cache_creation_per_million: Some(3.75),
        });
        table.insert(PricingEntry {
            model: ModelId::new("anthropic:claude-haiku-3-5"),
            input_per_million: 0.80,
            output_per_million: 4.0,
            cache_read_per_million: Some(0.08),
            cache_creation_per_million: Some(1.00),
        });

        // Claude CLI models (mirrors Anthropic pricing)
        table.insert(PricingEntry {
            model: ModelId::new("claude-cli:claude-opus-4-7"),
            input_per_million: 15.0,
            output_per_million: 75.0,
            cache_read_per_million: Some(1.50),
            cache_creation_per_million: Some(18.75),
        });
        table.insert(PricingEntry {
            model: ModelId::new("claude-cli:claude-sonnet-4-6"),
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: Some(0.30),
            cache_creation_per_million: Some(3.75),
        });
        table.insert(PricingEntry {
            model: ModelId::new("claude-cli:claude-opus-4-5"),
            input_per_million: 15.0,
            output_per_million: 75.0,
            cache_read_per_million: Some(1.50),
            cache_creation_per_million: Some(18.75),
        });
        table.insert(PricingEntry {
            model: ModelId::new("claude-cli:claude-sonnet-4-5"),
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: Some(0.30),
            cache_creation_per_million: Some(3.75),
        });

        // Gemini CLI models
        table.insert(PricingEntry {
            model: ModelId::new("gemini-cli:gemini-2.5-pro"),
            input_per_million: 1.25,
            output_per_million: 10.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        });
        table.insert(PricingEntry {
            model: ModelId::new("gemini-cli:gemini-2.5-flash"),
            input_per_million: 0.15,
            output_per_million: 0.60,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        });

        // OpenAI models
        table.insert(PricingEntry {
            model: ModelId::new("openai:gpt-4o"),
            input_per_million: 2.50,
            output_per_million: 10.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        });
        table.insert(PricingEntry {
            model: ModelId::new("openai:gpt-4o-mini"),
            input_per_million: 0.15,
            output_per_million: 0.60,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        });

        table
    }

    /// Look up pricing for a model by its `ModelId`.
    pub fn get(&self, model: &ModelId) -> Option<&PricingEntry> {
        self.entries.get(model.as_str())
    }

    /// Add or replace a pricing entry (keyed by the entry's model ID).
    pub fn insert(&mut self, entry: PricingEntry) {
        self.entries.insert(entry.model.as_str().to_owned(), entry);
    }
}
