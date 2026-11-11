#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! N9 of `docs/plans/fallback-notice-spec.md`: `App::handle_agent_event` shows
//! `AgentEvent::ModelFallback` as one system message
//! `<requested> failed (<reason>); switched to fallback <fallback>`, once per
//! (requested, fallback) pair per turn; the set resets when the turn ends (`Done` or `Error`).

use hone_proto::{AgentEvent, TokenUsage};
use hone_tui::app::App;
use hone_tui::message::DisplayRole;

fn fb(requested: &str, fallback: &str, reason: &str) -> AgentEvent {
    AgentEvent::ModelFallback {
        requested: requested.to_owned(),
        fallback: fallback.to_owned(),
        reason: reason.to_owned(),
    }
}

fn done() -> AgentEvent {
    AgentEvent::Done {
        usage: TokenUsage::default(),
        cost_usd: 0.0,
        plan: None,
    }
}

fn system_texts(app: &App) -> Vec<String> {
    app.messages()
        .iter()
        .filter(|m| matches!(m.role, DisplayRole::System))
        .map(|m| m.content.clone())
        .collect()
}

fn notices(app: &App) -> Vec<String> {
    system_texts(app)
        .into_iter()
        .filter(|t| t.contains("switched to fallback"))
        .collect()
}

const TEXT: &str = "anthropic:claude-x failed (overloaded); switched to fallback openai:gpt-4o";

#[test]
fn test_n9_a_model_fallback_pushes_one_system_message_with_the_exact_text() {
    let mut app = App::new();

    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));

    assert_eq!(notices(&app), vec![TEXT.to_owned()]);
    let last = app.messages().last().unwrap();
    assert!(matches!(last.role, DisplayRole::System), "{last:?}");
    assert_eq!(last.content, TEXT);
}

#[test]
fn test_n9_the_same_pair_in_one_turn_is_shown_once() {
    let mut app = App::new();

    for _ in 0..3 {
        app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));
        app.handle_agent_event(AgentEvent::Token { text: "x".into() });
    }

    assert_eq!(notices(&app), vec![TEXT.to_owned()]);
}

#[test]
fn test_n9_a_different_pair_in_the_same_turn_is_shown_too() {
    let mut app = App::new();

    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));
    app.handle_agent_event(fb("anthropic:claude-x", "ollama:fb", "overloaded"));
    app.handle_agent_event(fb("openai:other", "openai:gpt-4o", "down"));
    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));

    assert_eq!(
        notices(&app),
        vec![
            TEXT.to_owned(),
            "anthropic:claude-x failed (overloaded); switched to fallback ollama:fb".to_owned(),
            "openai:other failed (down); switched to fallback openai:gpt-4o".to_owned(),
        ]
    );
}

#[test]
fn test_n9_the_same_pair_after_done_within_one_user_turn_is_not_shown_again() {
    // An orchestrated turn sends a Done per worker.
    let mut app = App::new();
    app.skip_splash();
    app.type_text("do it");
    app.submit_input();
    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));
    app.handle_agent_event(done());

    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));

    assert_eq!(notices(&app), vec![TEXT.to_owned()]);
}

#[test]
fn test_n9_the_same_pair_after_error_within_one_user_turn_is_not_shown_again() {
    let mut app = App::new();
    app.skip_splash();
    app.type_text("do it");
    app.submit_input();
    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));
    app.handle_agent_event(AgentEvent::Error {
        message: "boom".into(),
    });

    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));

    assert_eq!(notices(&app), vec![TEXT.to_owned()]);
}

#[test]
fn test_n9_the_pair_is_shown_again_after_a_new_plain_prompt() {
    let mut app = App::new();
    app.skip_splash();
    app.type_text("first");
    app.submit_input();
    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));
    app.handle_agent_event(done());

    app.type_text("second");
    app.submit_input();
    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));

    assert_eq!(notices(&app), vec![TEXT.to_owned(), TEXT.to_owned()]);
}

#[test]
fn test_n9_the_pair_is_shown_again_after_a_new_inline_agent_prompt() {
    let mut app = App::new();
    app.skip_splash();
    app.type_text("first");
    app.submit_input();
    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));
    app.handle_agent_event(done());

    app.type_text("@reviewer look at this");
    app.submit_input();
    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));

    assert_eq!(notices(&app), vec![TEXT.to_owned(), TEXT.to_owned()]);
}

#[test]
fn test_n9_the_pair_is_shown_again_after_cancel_turn() {
    let mut app = App::new();
    app.skip_splash();
    app.type_text("first");
    app.submit_input();
    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));

    app.cancel_turn();
    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));

    assert_eq!(notices(&app), vec![TEXT.to_owned(), TEXT.to_owned()]);
}

#[test]
fn test_n9_a_streaming_assistant_message_stops_streaming_before_the_notice() {
    let mut app = App::new();
    app.handle_agent_event(AgentEvent::Token {
        text: "partial".into(),
    });
    assert!(app.messages().last().unwrap().is_streaming);

    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));
    app.handle_agent_event(AgentEvent::Token {
        text: "more".into(),
    });

    let msgs = app.messages();
    let n = msgs.len();
    assert_eq!(msgs[n - 3].content, "partial");
    assert!(!msgs[n - 3].is_streaming, "no stuck cursor: {msgs:?}");
    assert!(matches!(msgs[n - 2].role, DisplayRole::System), "{msgs:?}");
    assert!(
        matches!(msgs[n - 1].role, DisplayRole::Assistant),
        "{msgs:?}"
    );
    assert_eq!(msgs[n - 1].content, "more", "tokens go to a new message");
}

#[test]
fn test_n9_tokens_after_the_notice_start_a_new_assistant_message() {
    let mut app = App::new();
    app.handle_agent_event(AgentEvent::Token {
        text: "before".into(),
    });

    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));
    app.handle_agent_event(AgentEvent::Token {
        text: "after".into(),
    });

    let msgs = app.messages();
    let n = msgs.len();
    assert!(matches!(msgs[n - 2].role, DisplayRole::System), "{msgs:?}");
    assert!(
        matches!(msgs[n - 1].role, DisplayRole::Assistant),
        "{msgs:?}"
    );
    assert_eq!(msgs[n - 1].content, "after");
}

#[test]
fn test_n9_the_notice_changes_nothing_else_in_the_status() {
    let mut app = App::new();
    let before = format!("{:?}", app.status());

    app.handle_agent_event(fb("anthropic:claude-x", "openai:gpt-4o", "overloaded"));

    assert_eq!(format!("{:?}", app.status()), before);
}
