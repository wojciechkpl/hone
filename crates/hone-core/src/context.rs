use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};

use hone_proto::{Content, Message, Role};
use tiktoken_rs::CoreBPE;

use crate::metrics::ContextMetrics;

/// Number of most-recent messages that are never compressed or dropped.
///
/// Keeping fresh tail context verbatim improves answer quality per "Lost in
/// the Middle" research (Liu et al., TACL 2024): LLMs attend most strongly to
/// the start and end of the context window.
const RECENT_WINDOW: usize = 3;

/// Manages conversation history within a token budget.
pub struct ContextManager {
    max_context_tokens: u32,
    messages: Vec<Message>,
    // State for smart compaction: accumulates dropped message metadata across
    // one trim_to_budget pass so we can emit a single summary marker.
    dropped_count: usize,
    dropped_role_hints: Vec<String>,
    /// Optional metrics sink. When set, trim and compression events are recorded.
    metrics: Option<Arc<ContextMetrics>>,
    /// Maps content hash → first message index (in `self.messages`) that had
    /// that hash.  Used to detect duplicate tool results (e.g., the same file
    /// read twice) and replace later occurrences with a lightweight pointer.
    content_hashes: HashMap<u64, usize>,
}

impl ContextManager {
    pub fn new(max_context_tokens: u32) -> Self {
        Self {
            max_context_tokens,
            messages: Vec::new(),
            dropped_count: 0,
            dropped_role_hints: Vec::new(),
            metrics: None,
            content_hashes: HashMap::new(),
        }
    }

    /// Attach a metrics sink. Trim and compression events will be recorded on it.
    pub fn with_metrics(mut self, metrics: Arc<ContextMetrics>) -> Self {
        self.metrics = Some(metrics);
        self
    }

    /// Append a message to the conversation history, trimming old messages if
    /// the estimated token count exceeds the budget.
    ///
    /// Tool messages whose content matches a previously seen hash are replaced
    /// with a lightweight pointer to save tokens.
    pub fn add_message(&mut self, message: Message) {
        let idx = self.messages.len();

        // Dedup: for Tool messages with substantial content, check whether we
        // have already seen identical content.  If so, replace the incoming
        // message with a one-line pointer instead of storing the full text again.
        let message = if matches!(message.role, Role::Tool) {
            let text = message.content.as_text();
            if text.len() > 500 {
                let hash = hash_content(&text);
                if let Some(&first_idx) = self.content_hashes.get(&hash) {
                    // Duplicate detected — replace content with a pointer.
                    let summary = format!(
                        "[duplicate content: identical to tool result at message {first_idx} (hash {:016x})]",
                        hash
                    );
                    if let Some(m) = &self.metrics {
                        m.record_dedup_hit();
                    }
                    Message {
                        content: Content::Text(summary),
                        ..message
                    }
                } else {
                    self.content_hashes.insert(hash, idx);
                    message
                }
            } else {
                message
            }
        } else {
            message
        };

        self.messages.push(message);
        self.trim_to_budget();
    }

    /// Preservation-aware compaction.
    ///
    /// 1. Always preserve the first stored system message (index 0 if present)
    ///    and the first user message — these are the most critical anchors.
    /// 2. Compress old tool results (older than 5 assistant turns) to a short
    ///    summary — runs unconditionally every call so stale results are always
    ///    shrunk even when the budget is not yet exhausted.
    /// 3. Drop the oldest non-preserved, non-summary messages until under
    ///    budget, recording what was dropped.
    /// 4. Flush a synthetic "[session history: …]" system message so the model
    ///    knows context was compacted.
    fn trim_to_budget(&mut self) {
        // Step 1: compress stale tool results unconditionally — this runs even
        // when we are under budget so old outputs are always shrunk proactively.
        self.compress_old_tool_results(5);

        if self.total_tokens() <= self.max_context_tokens {
            return;
        }

        // Step 2: drop oldest non-preserved messages until under budget.
        let dropped_before = self.dropped_count;
        loop {
            if self.total_tokens() <= self.max_context_tokens {
                break;
            }

            // Recompute preserved set each iteration because indices shift after
            // each removal.
            let preserved = preserved_indices(&self.messages);

            // Never drop from the protected tail (RECENT_WINDOW most-recent messages).
            let protected_from = self.messages.len().saturating_sub(RECENT_WINDOW);
            let drop_idx = (0..protected_from)
                .find(|i| !preserved.contains(i) && !is_summary_marker_at(&self.messages, *i));

            let Some(idx) = drop_idx else {
                // Nothing left to drop (everything is preserved or a marker).
                break;
            };

            let removed = self.messages.remove(idx);
            self.record_dropped_message(removed);
        }

        // Record trim event if any messages were dropped in this pass.
        let newly_dropped = self.dropped_count - dropped_before;
        if newly_dropped > 0 {
            if let Some(m) = &self.metrics {
                m.record_trim(newly_dropped as u32);
            }
        }

        // Step 3: emit a summary marker for everything we dropped this pass.
        self.flush_dropped_summary();
    }

    /// Compress tool-result messages whose "age" (measured by how many
    /// assistant turns have elapsed since them) is ≥ `max_age_turns`.
    ///
    /// Compressed content keeps the first 80 chars of the original output plus
    /// a header indicating the original length, so the model knows the result
    /// existed but is not spending tokens on stale detail.
    ///
    /// Messages in the protected tail (`RECENT_WINDOW` most-recent messages) are
    /// never compressed, preserving fresh context at the end of the window.
    fn compress_old_tool_results(&mut self, max_age_turns: usize) {
        // The last RECENT_WINDOW messages are sacred — never compress them.
        let protected_from = self.messages.len().saturating_sub(RECENT_WINDOW);

        let mut assistant_turns_since = 0usize;
        for i in (0..protected_from).rev() {
            match self.messages[i].role {
                Role::Assistant => {
                    assistant_turns_since += 1;
                }
                Role::Tool if assistant_turns_since >= max_age_turns => {
                    let text = self.messages[i].content.as_text().to_string();
                    if text.starts_with("[tool result compressed") {
                        // Already compressed — skip.
                        continue;
                    }
                    let preview_end = 80.min(text.len());
                    let summary = format!(
                        "[tool result compressed, original {} chars: {}…]",
                        text.len(),
                        &text[..preview_end]
                    );
                    self.messages[i].content = Content::Text(summary);
                    if let Some(m) = &self.metrics {
                        m.record_compression();
                    }
                }
                _ => {}
            }
        }
    }

    /// Record metadata about a message we are about to drop.
    fn record_dropped_message(&mut self, msg: Message) {
        self.dropped_count += 1;
        let role_hint = format!("{:?}", msg.role).to_lowercase();
        self.dropped_role_hints.push(role_hint);
    }

    /// Emit a "[session history: …]" system message summarising every message
    /// dropped in this trim pass.  Replaces any existing summary marker to
    /// avoid duplicates.
    fn flush_dropped_summary(&mut self) {
        if self.dropped_count == 0 {
            return;
        }

        let summary = format!(
            "[session history: {} earlier messages compressed to fit context window — {}]",
            self.dropped_count,
            self.dropped_role_hints.join(", ")
        );

        // Remove any pre-existing summary marker first.
        if let Some(existing) = (0..self.messages.len()).find(|i| is_summary_marker_at(&self.messages, *i)) {
            self.messages.remove(existing);
        }

        // Insert after the first stored system message (index 0) if one is
        // present; otherwise at the front.
        let insert_at =
            if self.messages.first().is_some_and(|m| matches!(m.role, Role::System)) {
                1
            } else {
                0
            };

        self.messages.insert(
            insert_at,
            Message {
                role: Role::System,
                content: Content::Text(summary),
                tool_calls: vec![],
                tool_call_id: None,
            },
        );

        self.dropped_count = 0;
        self.dropped_role_hints.clear();
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

// ---------------------------------------------------------------------------
// Free functions (no self borrow — avoids lifetime conflicts inside closures)
// ---------------------------------------------------------------------------

/// Returns the indices of messages that must never be dropped:
/// - The first non-summary stored system message (typically the context seed).
/// - The first user message (the original request).
fn preserved_indices(messages: &[Message]) -> HashSet<usize> {
    let mut preserved = HashSet::new();

    // First system message that is NOT our synthetic summary marker.
    if let Some(idx) = messages.iter().position(|m| {
        matches!(m.role, Role::System) && !m.content.as_text().starts_with("[session history:")
    }) {
        preserved.insert(idx);
    }

    // First user message — the original request.
    if let Some(idx) = messages.iter().position(|m| matches!(m.role, Role::User)) {
        preserved.insert(idx);
    }

    preserved
}

/// Returns `true` if the message at `idx` is the synthetic session-history
/// summary marker inserted by `flush_dropped_summary`.
fn is_summary_marker_at(messages: &[Message], idx: usize) -> bool {
    match messages.get(idx) {
        Some(msg) if matches!(msg.role, Role::System) => {
            msg.content.as_text().starts_with("[session history:")
        }
        _ => false,
    }
}

/// Compute a 64-bit hash of a string for content-identity comparisons.
///
/// Uses `DefaultHasher` — non-cryptographic but fast and stable within a
/// single process run, which is all we need for dedup within a session.
fn hash_content(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
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
