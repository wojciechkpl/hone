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
