use std::collections::HashMap;

use hone_proto::ModelId;

/// Context window sizes (in tokens) for known models.
///
/// These are consulted by provider adapters to emit
/// `AgentEvent::ModelInfo { context_window }` on the first successful
/// completion.  Values are upper bounds as documented by the vendor; actual
/// usable context may be slightly smaller due to reserved tokens for the
/// system prompt and tool definitions.
///
/// For Ollama the value can be fetched dynamically via `/api/show`; this table
/// is used as a fallback when the API call fails or the field is absent.
pub struct ContextWindowTable {
    entries: HashMap<String, u32>,
    /// Default returned when no explicit entry is found.
    default: u32,
}

impl ContextWindowTable {
    /// Return a table pre-populated with known context window sizes.
    pub fn default_table() -> Self {
        let mut t = Self {
            entries: HashMap::new(),
            // Conservative fallback — matches StatusInfo::default()
            default: 128_000,
        };

        // ---- Anthropic -------------------------------------------------------
        // Claude 4.x series: all 200 k
        t.insert("anthropic:claude-opus-4-5", 200_000);
        t.insert("anthropic:claude-sonnet-4-5", 200_000);
        t.insert("anthropic:claude-haiku-4-5", 200_000);
        // Claude 3.x series: 200 k (sonnet/opus) or 200 k (haiku)
        t.insert("anthropic:claude-3-5-sonnet-20241022", 200_000);
        t.insert("anthropic:claude-3-5-haiku-20241022", 200_000);
        t.insert("anthropic:claude-3-opus-20240229", 200_000);
        t.insert("anthropic:claude-3-sonnet-20240229", 200_000);
        t.insert("anthropic:claude-3-haiku-20240307", 200_000);
        // Claude CLI mirrors Anthropic
        t.insert("claude-cli:claude-sonnet-4-5", 200_000);
        t.insert("claude-cli:claude-opus-4-5", 200_000);
        t.insert("claude-cli:claude-3-5-sonnet-20241022", 200_000);

        // ---- OpenAI ----------------------------------------------------------
        t.insert("openai:gpt-4o", 128_000);
        t.insert("openai:gpt-4o-mini", 128_000);
        t.insert("openai:gpt-4-turbo", 128_000);
        t.insert("openai:gpt-4-turbo-preview", 128_000);
        t.insert("openai:gpt-4", 8_192);
        t.insert("openai:gpt-3.5-turbo", 16_385);
        t.insert("openai:o1-preview", 200_000);
        t.insert("openai:o1-mini", 128_000);
        t.insert("openai:o3", 200_000);
        t.insert("openai:o3-mini", 200_000);

        // ---- Gemini CLI ------------------------------------------------------
        t.insert("gemini-cli:gemini-2.5-pro", 1_000_000);
        t.insert("gemini-cli:gemini-2.5-flash", 1_000_000);
        t.insert("gemini-cli:gemini-2.0-flash", 1_000_000);
        t.insert("gemini-cli:gemini-1.5-pro", 2_000_000);
        t.insert("gemini-cli:gemini-1.5-flash", 1_000_000);

        // ---- Ollama defaults (can be overridden at runtime via /api/show) ----
        t.insert("ollama:llama3.2", 128_000);
        t.insert("ollama:llama3.1", 128_000);
        t.insert("ollama:qwen2.5-coder", 32_768);
        t.insert("ollama:mistral", 32_768);
        t.insert("ollama:codellama", 16_384);

        t
    }

    /// Look up the context window for a model by its [`ModelId`].
    ///
    /// Falls back to the table default (128 000) when the model is unknown.
    pub fn get(&self, model: &ModelId) -> u32 {
        self.entries
            .get(model.as_str())
            .copied()
            .unwrap_or(self.default)
    }

    fn insert(&mut self, key: &str, size: u32) {
        self.entries.insert(key.to_owned(), size);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_anthropic_model_returns_200k() {
        let table = ContextWindowTable::default_table();
        let model = ModelId::new("anthropic:claude-sonnet-4-5");
        assert_eq!(table.get(&model), 200_000);
    }

    #[test]
    fn known_openai_model_returns_128k() {
        let table = ContextWindowTable::default_table();
        let model = ModelId::new("openai:gpt-4o");
        assert_eq!(table.get(&model), 128_000);
    }

    #[test]
    fn unknown_model_returns_default() {
        let table = ContextWindowTable::default_table();
        let model = ModelId::new("openai:gpt-99-ultra");
        assert_eq!(table.get(&model), 128_000);
    }

    #[test]
    fn gemini_returns_1m() {
        let table = ContextWindowTable::default_table();
        let model = ModelId::new("gemini-cli:gemini-2.5-pro");
        assert_eq!(table.get(&model), 1_000_000);
    }
}
