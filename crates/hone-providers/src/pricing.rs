use std::collections::HashMap;

use hone_proto::{ModelId, PricingEntry};

/// A registry of per-model pricing data.
pub struct PricingTable {
    entries: HashMap<String, PricingEntry>,
}

/// `id` without a trailing `-YYYY-MM-DD` or `-YYYYMMDD`, when it has one.
fn strip_date_suffix(id: &str) -> Option<&str> {
    let bytes = id.as_bytes();
    let n = bytes.len();
    let digits = |r: &[u8]| r.iter().all(u8::is_ascii_digit);
    if n >= 11 {
        let t = &bytes[n - 11..];
        if t[0] == b'-'
            && t[5] == b'-'
            && t[8] == b'-'
            && digits(&t[1..5])
            && digits(&t[6..8])
            && digits(&t[9..11])
        {
            return Some(&id[..n - 11]);
        }
    }
    if n >= 9 {
        let t = &bytes[n - 9..];
        // Exactly eight digits after the dash (not nine).
        if t[0] == b'-' && digits(&t[1..]) {
            return Some(&id[..n - 9]);
        }
    }
    None
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
    ///
    /// An id ending in `-YYYY-MM-DD` that has no entry of its own is priced with
    /// its base id's entry (`openai:gpt-4o-2024-08-06` as `openai:gpt-4o`).
    pub fn get(&self, model: &ModelId) -> Option<&PricingEntry> {
        let id = model.as_str();
        self.entries
            .get(id)
            .or_else(|| strip_date_suffix(id).and_then(|base| self.entries.get(base)))
    }

    /// Add or replace a pricing entry (keyed by the entry's model ID).
    pub fn insert(&mut self, entry: PricingEntry) {
        self.entries.insert(entry.model.as_str().to_owned(), entry);
    }
}
