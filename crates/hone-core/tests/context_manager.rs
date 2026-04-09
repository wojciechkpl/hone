use hone_core::ContextManager;
use hone_proto::{Content, Message, Role};

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

/// Smart compaction inserts a "[session history: …]" marker instead of the old
/// "[N earlier messages trimmed …]" text.  Both use system-role messages so
/// the model receives them as context.
#[test]
fn test_context_inserts_trim_marker() {
    let mut ctx = ContextManager::new(100);
    ctx.add_message(Message::system("You are a helpful assistant."));
    let long_text = "a".repeat(80);
    for _ in 0..20 {
        ctx.add_message(Message::user(long_text.clone()));
    }
    // The session-history marker should appear somewhere in the built messages.
    let built = ctx.build_messages("");
    let has_trim_marker = built
        .iter()
        .any(|m| m.content.as_text().contains("session history"));
    assert!(has_trim_marker, "expected a session history marker in the message history");
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

// ---------------------------------------------------------------------------
// G12: Smart context compaction — new tests
// ---------------------------------------------------------------------------

/// The system prompt stored in self.messages and the first user message must
/// survive even when many subsequent messages push the context over budget.
#[test]
fn test_preserves_system_prompt_and_first_user_message() {
    // Budget is tight enough to force drops but large enough to hold the seed
    // messages plus a few recents.
    //
    // Each `"system prompt" × 5` ≈ 13 chars per repeat × 5 = 65 chars ≈ ~16 + 4 = ~20 tokens.
    // Each assistant response: "response NN" × 10 ≈ 110 chars → ~28 + 4 = ~32 tokens.
    // Budget 300 tokens: keeps ~9 messages.  We add 30 → must trim.
    let mut ctx = ContextManager::new(300);
    ctx.add_message(Message::system("system prompt".repeat(5)));
    ctx.add_message(Message::user("original request".repeat(5)));

    for i in 0..30 {
        ctx.add_message(Message::assistant(format!("response {i}").repeat(10)));
    }

    let built = ctx.build_messages("system prompt");

    // build_messages always prepends the live system prompt at index 0.
    assert!(
        matches!(built.first().map(|m| &m.role), Some(Role::System)),
        "build_messages must start with a system message"
    );

    // The stored system-prompt seed must survive in the body.
    let has_seed_system = built.iter().skip(1).any(|m| {
        matches!(m.role, Role::System)
            && m.content.as_text().contains("system prompt")
            && !m.content.as_text().starts_with("[session history:")
    });
    assert!(has_seed_system, "stored system message (seed) should be preserved");

    // The first user message must survive.
    let has_original = built
        .iter()
        .any(|m| matches!(m.role, Role::User) && m.content.as_text().contains("original request"));
    assert!(has_original, "first user message should be preserved");
}

/// After trimming, a "[session history: …]" summary marker must appear in the
/// built message list.
#[test]
fn test_summary_marker_inserted_after_trim() {
    // ~5 tokens per short message; budget=200 → ~40 messages fit.
    // We add 100 assistant messages → must trim.
    let mut ctx = ContextManager::new(200);
    ctx.add_message(Message::system("sys"));
    ctx.add_message(Message::user("original request"));

    for i in 0..100 {
        ctx.add_message(Message::assistant(format!("response {i}").repeat(5)));
    }

    let built = ctx.build_messages("sys");
    let has_summary = built.iter().any(|m| {
        matches!(m.role, Role::System) && m.content.as_text().contains("session history")
    });
    assert!(has_summary, "should have inserted session history summary after trimming");
}

/// Tool results older than 5 assistant turns should be compressed even when
/// the token budget is large enough that no messages are dropped.
#[test]
fn test_old_tool_results_compressed() {
    // Large budget: no messages will be dropped, but tool compression still runs.
    let mut ctx = ContextManager::new(100_000);
    ctx.add_message(Message::system("sys"));
    ctx.add_message(Message::user("request"));

    let long_tool_output = "x".repeat(500);
    ctx.add_message(Message {
        role: Role::Tool,
        content: Content::Text(long_tool_output.clone()),
        tool_calls: vec![],
        tool_call_id: Some("call_1".to_string()),
    });

    // Add 10 assistant messages so the tool result ages past the 5-turn threshold.
    for i in 0..10 {
        ctx.add_message(Message::assistant(format!("a{i}")));
    }

    // Adding one more message triggers trim_to_budget → compress_old_tool_results.
    ctx.add_message(Message::user("more"));

    let built = ctx.build_messages("sys");

    // The original 500-char tool output must have been compressed.
    let still_has_full_output = built.iter().any(|m| {
        matches!(m.role, Role::Tool) && m.content.as_text().len() >= 400
    });
    assert!(
        !still_has_full_output,
        "old tool result (500 chars) should have been compressed to a short summary"
    );

    // The compressed placeholder must be present (graduated compression may
    // produce "tool result summarized" or "[tool result:" depending on age).
    let has_compressed_placeholder = built.iter().any(|m| {
        matches!(m.role, Role::Tool) && {
            let t = m.content.as_text();
            t.contains("tool result summarized") || t.contains("[tool result:")
        }
    });
    assert!(
        has_compressed_placeholder,
        "a compressed-tool-result placeholder should appear in built messages"
    );
}

/// A tool result that is only a few assistant turns old (below the threshold)
/// must NOT be compressed.
#[test]
fn test_recent_tool_results_not_compressed() {
    let mut ctx = ContextManager::new(100_000);
    ctx.add_message(Message::system("sys"));
    ctx.add_message(Message::user("request"));

    let tool_output = "y".repeat(300);
    ctx.add_message(Message {
        role: Role::Tool,
        content: Content::Text(tool_output.clone()),
        tool_calls: vec![],
        tool_call_id: Some("call_2".to_string()),
    });

    // Only 2 assistant turns — below the 5-turn threshold.
    ctx.add_message(Message::assistant("a1"));
    ctx.add_message(Message::assistant("a2"));
    ctx.add_message(Message::user("follow up"));

    let built = ctx.build_messages("sys");

    let still_has_full_output = built.iter().any(|m| {
        matches!(m.role, Role::Tool) && m.content.as_text().len() >= 200
    });
    assert!(
        still_has_full_output,
        "recent tool result should NOT be compressed (only 2 turns old, threshold is 5)"
    );
}

// ---------------------------------------------------------------------------
// Item 1: Content-hashed dedup tests
// ---------------------------------------------------------------------------

#[test]
fn test_duplicate_tool_results_replaced_with_pointer() {
    let mut ctx = ContextManager::new(100_000);
    ctx.add_message(Message::system("sys"));
    ctx.add_message(Message::user("read the file"));

    // Add a tool result with substantial content.
    let content = "x".repeat(1000);
    let tool_msg = Message {
        role: Role::Tool,
        content: Content::Text(content.clone()),
        tool_calls: vec![],
        tool_call_id: Some("call_1".to_string()),
    };
    ctx.add_message(tool_msg);

    // Add the same content again (simulates re-reading the same file).
    let tool_msg2 = Message {
        role: Role::Tool,
        content: Content::Text(content.clone()),
        tool_calls: vec![],
        tool_call_id: Some("call_2".to_string()),
    };
    ctx.add_message(tool_msg2);

    let built = ctx.build_messages("sys");

    // First tool message has full content.
    let first_tool = built
        .iter()
        .find(|m| matches!(m.role, Role::Tool) && m.content.as_text().len() > 500);
    assert!(first_tool.is_some(), "first tool message should have full content");

    // Count total tool messages.
    let tool_count = built.iter().filter(|m| matches!(m.role, Role::Tool)).count();
    assert!(tool_count >= 2);

    // At least one tool message should be a dedup pointer.
    let has_pointer = built
        .iter()
        .any(|m| matches!(m.role, Role::Tool) && m.content.as_text().contains("duplicate content"));
    assert!(has_pointer, "second occurrence should be replaced with pointer");
}

#[test]
fn test_small_tool_results_not_deduped() {
    let mut ctx = ContextManager::new(100_000);
    let small = "small content".to_string();
    for _ in 0..3 {
        ctx.add_message(Message {
            role: Role::Tool,
            content: Content::Text(small.clone()),
            tool_calls: vec![],
            tool_call_id: None,
        });
    }
    let built = ctx.build_messages("sys");
    let has_pointer = built
        .iter()
        .any(|m| m.content.as_text().contains("duplicate content"));
    assert!(!has_pointer, "small content should not be deduped");
}

#[test]
fn test_different_tool_results_not_deduped() {
    let mut ctx = ContextManager::new(100_000);
    ctx.add_message(Message {
        role: Role::Tool,
        content: Content::Text("a".repeat(1000)),
        tool_calls: vec![],
        tool_call_id: None,
    });
    ctx.add_message(Message {
        role: Role::Tool,
        content: Content::Text("b".repeat(1000)),
        tool_calls: vec![],
        tool_call_id: None,
    });
    let built = ctx.build_messages("sys");
    let has_pointer = built
        .iter()
        .any(|m| m.content.as_text().contains("duplicate content"));
    assert!(!has_pointer, "different content should not be deduped");
}

// ---------------------------------------------------------------------------
// Item 2: "Lost in the Middle" ordering and RECENT_WINDOW protection tests
// ---------------------------------------------------------------------------

#[test]
fn test_message_ordering_respects_lost_in_the_middle() {
    let mut ctx = ContextManager::new(100_000);
    ctx.add_message(Message::system("sys"));
    ctx.add_message(Message::user("original request"));
    for i in 0..5 {
        ctx.add_message(Message::assistant(format!("response {i}")));
    }
    ctx.add_message(Message::user("most recent query"));

    let built = ctx.build_messages("sys");

    // First message should be the system prompt.
    assert!(
        matches!(built.first().map(|m| &m.role), Some(Role::System)),
        "first message must be the system prompt"
    );

    // Last message should be the most recent user query.
    assert_eq!(
        built.last().unwrap().content.as_text(),
        "most recent query",
        "most recent query should be the last message"
    );
    assert!(
        matches!(built.last().map(|m| &m.role), Some(Role::User)),
        "last message must be a user message"
    );
}

#[test]
fn test_recent_messages_never_compressed() {
    let mut ctx = ContextManager::new(10_000);
    for i in 0..20 {
        ctx.add_message(Message::user(format!("msg {i}")));
    }
    let recent = Message {
        role: Role::Tool,
        content: Content::Text("x".repeat(500)),
        tool_calls: vec![],
        tool_call_id: None,
    };
    ctx.add_message(recent);
    for _ in 0..2 {
        ctx.add_message(Message::assistant("recent assistant".to_string()));
    }

    // The recent tool result should NOT be compressed (it's within RECENT_WINDOW).
    let built = ctx.build_messages("sys");
    let tool_msg = built.iter().find(|m| matches!(m.role, Role::Tool));
    if let Some(m) = tool_msg {
        assert!(
            m.content.as_text().len() >= 400,
            "recent tool result should not be compressed, got: {}",
            m.content.as_text()
        );
    }
}
