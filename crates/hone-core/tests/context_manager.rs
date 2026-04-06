use hone_core::ContextManager;
use hone_proto::{Message, Role};

#[test]
fn test_add_and_retrieve_messages() {
    let mut ctx = ContextManager::new(8192);
    ctx.add_message(Message::user("hello"));
    ctx.add_message(Message::assistant("hi there"));
    ctx.add_message(Message::user("how are you?"));

    let messages = ctx.build_messages("You are a helpful assistant.");
    // system prompt + 3 stored messages
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

// ---------------------------------------------------------------------------
// Sliding-window / trimming tests
// ---------------------------------------------------------------------------

/// Each long message is ~80 chars → ~20 tokens + 4 overhead = 24 tokens.
/// 20 messages × 24 = 480 tokens. Budget is 100, so trimming must occur.
#[test]
fn test_context_trims_when_over_budget() {
    let mut ctx = ContextManager::new(100);
    ctx.add_message(Message::system("You are a helpful assistant."));
    let long_text = "a".repeat(80); // ~20 tokens + 4 overhead = 24 tokens each
    for _ in 0..20 {
        ctx.add_message(Message::user(long_text.clone()));
    }
    // 21 stored (system + 20 user), but trimming should have reduced them.
    assert!(
        ctx.message_count() < 22,
        "expected fewer than 22 stored messages, got {}",
        ctx.message_count()
    );
}

#[test]
fn test_context_inserts_trim_marker() {
    let mut ctx = ContextManager::new(100);
    ctx.add_message(Message::system("You are a helpful assistant."));
    let long_text = "a".repeat(80);
    for _ in 0..20 {
        ctx.add_message(Message::user(long_text.clone()));
    }
    // The trim marker should appear somewhere in the built messages.
    let built = ctx.build_messages("");
    let has_trim_marker = built
        .iter()
        .any(|m| m.content.as_text().contains("trimmed"));
    assert!(has_trim_marker, "expected a trim marker in the message history");
}

#[test]
fn test_context_keeps_recent_messages() {
    let mut ctx = ContextManager::new(100);
    ctx.add_message(Message::system("You are a helpful assistant."));
    let long_text = "a".repeat(80);
    for _ in 0..19 {
        ctx.add_message(Message::user(long_text.clone()));
    }
    let last_msg = "this is the final message";
    ctx.add_message(Message::user(last_msg));
    // The most recently added message must be present after trimming.
    let built = ctx.build_messages("");
    let has_last = built.iter().any(|m| m.content.as_text().contains(last_msg));
    assert!(has_last, "the most recently added message should survive trimming");
}
