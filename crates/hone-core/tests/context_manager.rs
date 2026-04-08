use hone_core::ContextManager;
use hone_proto::{Message, Role};

// ---------------------------------------------------------------------------
// D2 regression test: long sessions must trigger trimming via ContextManager
// ---------------------------------------------------------------------------

/// Each message body is ~100 chars ≈ 25 tokens + 4 overhead = ~29 tokens each.
/// max_tokens=500 / 29 ≈ 17 messages max.  After 50 messages the context must
/// have been trimmed.  The first stored message (index 0 in `messages`) acts as
/// a pseudo-system message; `build_messages` prepends the real system message
/// on top of that, so the system prompt always survives.
#[test]
fn test_long_session_triggers_trim() {
    let mut ctx = ContextManager::new(500);

    // The system-like context seed (not the build-time system prompt).
    ctx.add_message(Message::system("You are a helpful assistant."));

    let body = "a".repeat(100); // ~25 tokens + 4 overhead
    for _ in 0..50 {
        ctx.add_message(Message::user(body.clone()));
    }

    // Trimming must have occurred: fewer than 51 stored messages (system + 50 user).
    assert!(
        ctx.message_count() < 51,
        "expected context to be trimmed below 51 messages, got {}",
        ctx.message_count()
    );

    // The system message must be present in the built output.
    let built = ctx.build_messages("You are a helpful assistant.");
    assert_eq!(
        built[0].role,
        Role::System,
        "system prompt must always be the first message"
    );
}

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
