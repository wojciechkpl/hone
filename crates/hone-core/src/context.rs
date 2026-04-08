use std::sync::OnceLock;

use hone_proto::Message;
use tiktoken_rs::CoreBPE;

/// Manages conversation history within a token budget.
pub struct ContextManager {
    max_context_tokens: u32,
    messages: Vec<Message>,
}

impl ContextManager {
    pub fn new(max_context_tokens: u32) -> Self {
        Self {
            max_context_tokens,
            messages: Vec::new(),
        }
    }

    /// Append a message to the conversation history, trimming old messages if
    /// the estimated token count exceeds the budget.
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
        self.trim_to_budget();
    }

    /// Drop the oldest non-system messages until the estimated total fits
    /// within `max_context_tokens`, then insert a marker so the model knows
    /// context was trimmed.
    fn trim_to_budget(&mut self) {
        let estimates: Vec<u32> = self
            .messages
            .iter()
            .map(estimate_message_tokens)
            .collect();

        let total_tokens: u32 = estimates.iter().sum();

        if total_tokens <= self.max_context_tokens {
            return;
        }

        // Keep the first message (system/context marker) and drop from index 1
        // forward until we're under budget.
        let mut drop_until = 1;
        let mut freed = 0u32;
        while total_tokens.saturating_sub(freed) > self.max_context_tokens
            && drop_until < self.messages.len().saturating_sub(1)
        {
            freed += estimates[drop_until];
            drop_until += 1;
        }

        if drop_until > 1 {
            let dropped_count = drop_until - 1;
            self.messages.drain(1..drop_until);
            self.messages.insert(
                1,
                Message::system(format!(
                    "[{dropped_count} earlier messages trimmed to fit context window]"
                )),
            );
        }
    }

    /// Build the full message list for a completion request.
    ///
    /// Prepends a system message derived from `system_prompt`, then appends
    /// all stored messages.  Budget enforcement is performed incrementally in
    /// `add_message` / `trim_to_budget`; no secondary window is applied here
    /// so that trim markers inserted by `trim_to_budget` are always visible
    /// to the model.
    pub fn build_messages(&self, system_prompt: &str) -> Vec<Message> {
        let system = Message::system(system_prompt);
        let mut result = Vec::with_capacity(self.messages.len() + 1);
        result.push(system);
        result.extend_from_slice(&self.messages);
        result
    }

    /// Remove all stored messages (system prompt is re-supplied at build time).
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Number of non-system messages stored.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Estimated total token count of all stored messages (excludes the system
    /// prompt, which is supplied at build time and not stored here).
    pub fn total_tokens(&self) -> u32 {
        self.messages.iter().map(estimate_message_tokens).sum()
    }
}

/// Global BPE tokenizer, initialized once on first use.
///
/// `cl100k_base` is the tokenizer used by Claude and GPT-4 models.  Loading it
/// takes ~10 ms and ~5 MB of memory; we pay that cost exactly once.
static BPE: OnceLock<CoreBPE> = OnceLock::new();

fn bpe() -> Option<&'static CoreBPE> {
    BPE.get_or_init(|| {
        tiktoken_rs::cl100k_base().expect("tiktoken cl100k_base data embedded in binary")
    });
    BPE.get()
}

/// Estimate the number of tokens in a message using tiktoken (`cl100k_base`).
///
/// Falls back to the 4-chars-per-token heuristic if the tokenizer cannot be
/// loaded (should never happen — the BPE data is embedded at compile time).
/// A per-message overhead of +4 tokens accounts for role/formatting framing.
fn estimate_message_tokens(msg: &Message) -> u32 {
    let text = msg.content.as_text();
    match bpe() {
        Some(enc) => enc.encode_ordinary(&text.to_string()).len() as u32 + 4,
        None => (text.len() as u32 / 4).max(1) + 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hone_proto::{Message, Role};

    #[test]
    fn test_add_and_retrieve_messages() {
        let mut ctx = ContextManager::new(8192);
        ctx.add_message(Message::user("hello"));
        ctx.add_message(Message::assistant("hi there"));
        ctx.add_message(Message::user("how are you?"));

        let messages = ctx.build_messages("You are a helpful assistant.");
        // system + 3 stored
        assert_eq!(messages.len(), 4);
    }

    #[test]
    fn test_system_message_always_present() {
        let mut ctx = ContextManager::new(8192);
        for i in 0..10 {
            ctx.add_message(Message::user(format!("message {i}")));
        }

        let messages = ctx.build_messages("System prompt here.");
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content.as_text(), "System prompt here.");
    }

    #[test]
    fn test_clear_resets() {
        let mut ctx = ContextManager::new(8192);
        ctx.add_message(Message::user("something"));
        ctx.add_message(Message::assistant("response"));
        ctx.clear();
        assert_eq!(ctx.message_count(), 0);
    }

    #[test]
    fn test_build_messages_includes_system_prompt() {
        let ctx = ContextManager::new(8192);
        let messages = ctx.build_messages("Be concise.");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content.as_text(), "Be concise.");
    }
}
