use hone_proto::Message;

/// Manages conversation history within a token budget.
///
/// For now the budget is tracked by message count only; actual token-based
/// trimming is left for a future pass.
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

    /// Append a non-system message to the conversation history.
    pub fn add_message(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Build the full message list for a completion request.
    ///
    /// Always inserts a system message first (derived from `system_prompt`),
    /// followed by all stored messages.  When the stored history would
    /// overflow `max_context_tokens` (using a simple 4-chars-per-token
    /// heuristic), older messages are dropped from the front until the budget
    /// is satisfied — the system message is always retained.
    pub fn build_messages(&self, system_prompt: &str) -> Vec<Message> {
        let system = Message::system(system_prompt);
        let mut result = Vec::with_capacity(self.messages.len() + 1);
        result.push(system);

        // Simple heuristic: count characters / 4 as tokens.
        let system_tokens =
            (system_prompt.len() as u32 / 4).max(1);

        let available = self.max_context_tokens.saturating_sub(system_tokens);

        // Walk messages from oldest to newest, dropping from the front when
        // we would exceed the budget.
        let mut start = 0;
        let tokens_per_msg: Vec<u32> = self
            .messages
            .iter()
            .map(|m| (m.content.as_text().len() as u32 / 4).max(1))
            .collect();

        let total_history: u32 = tokens_per_msg.iter().sum();
        let mut running = total_history;

        while running > available && start < self.messages.len() {
            running = running.saturating_sub(tokens_per_msg[start]);
            start += 1;
        }

        // Suppress unused-variable warning: available is intentional API
        let _ = available;

        result.extend_from_slice(&self.messages[start..]);
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
