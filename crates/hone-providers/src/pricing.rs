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

        // Source: https://platform.claude.com/docs/en/about-claude/pricing (2026-11-17).
        // `cache_creation` is the 5-minute write price: Hone uses the default ephemeral cache.
        // The Claude CLI mirrors Anthropic pricing, so both prefixes come from one list.
        // (id, input, output, cache_read, cache_creation) per million tokens.
        const CLAUDE_PRICES: &[(&str, f64, f64, f64, f64)] = &[
            ("claude-fable-5-2", 8.0, 40.0, 0.20, 10.0),
            ("claude-mythos-5-2", 8.0, 40.0, 0.20, 10.0),
            ("claude-fable-5-1", 10.0, 50.0, 0.25, 12.50),
            ("claude-mythos-5-1", 10.0, 50.0, 0.25, 12.50),
            ("claude-fable-5", 10.0, 50.0, 1.0, 12.50),
            ("claude-mythos-5", 10.0, 50.0, 1.0, 12.50),
            ("claude-opus-5-5", 4.0, 20.0, 0.20, 5.0),
            ("claude-opus-5", 5.0, 25.0, 0.50, 6.25),
            ("claude-opus-4-8", 5.0, 25.0, 0.50, 6.25),
            ("claude-opus-4-7", 5.0, 25.0, 0.50, 6.25),
            ("claude-opus-4-6", 5.0, 25.0, 0.50, 6.25),
            ("claude-opus-4-5", 5.0, 25.0, 0.50, 6.25),
            ("claude-sonnet-5-5", 2.0, 10.0, 0.10, 2.50),
            ("claude-sonnet-5", 2.0, 10.0, 0.20, 2.50),
            ("claude-sonnet-4-6", 3.0, 15.0, 0.30, 3.75),
            ("claude-sonnet-4-5", 3.0, 15.0, 0.30, 3.75),
            // Haiku 5.5 is priced at the <=100K-token rate. Prompts over 100K tokens
            // cost 0.50 / 2.50 / 0.05 / 0.625; tier support is a follow-up.
            ("claude-haiku-5-5", 0.10, 0.50, 0.01, 0.125),
            // Keyed by alias; dated ids resolve via `strip_date_suffix`.
            ("claude-haiku-4-5", 1.0, 5.0, 0.10, 1.25),
        ];
        for prefix in ["anthropic", "claude-cli"] {
            for &(id, input, output, cache_read, cache_write) in CLAUDE_PRICES {
                table.insert(PricingEntry {
                    model: ModelId::new(format!("{prefix}:{id}")),
                    input_per_million: input,
                    output_per_million: output,
                    cache_read_per_million: Some(cache_read),
                    cache_creation_per_million: Some(cache_write),
                });
            }
        }
        table.insert(PricingEntry {
            model: ModelId::new("anthropic:claude-haiku-3-5"),
            input_per_million: 0.80,
            output_per_million: 4.0,
            cache_read_per_million: Some(0.08),
            cache_creation_per_million: Some(1.00),
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
