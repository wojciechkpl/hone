use hone_proto::{AgentEvent, TokenUsage};
use hone_tui::{
    app::{App, AppState},
    commands::Command,
    input::InputState,
    message::DisplayRole,
    theme::Theme,
};

// ---------------------------------------------------------------------------
// InputState tests
// ---------------------------------------------------------------------------

#[test]
fn test_input_empty() {
    let input = InputState::new();
    assert!(input.is_empty());
    assert_eq!(input.buffer, "");
    assert_eq!(input.cursor_position, 0);
}

#[test]
fn test_input_insert_char() {
    let mut input = InputState::new();
    input.insert_char('h');
    input.insert_char('i');
    assert_eq!(input.buffer, "hi");
    assert_eq!(input.cursor_position, 2);
    assert!(!input.is_empty());
}

#[test]
fn test_input_delete_char() {
    let mut input = InputState::new();
    input.insert_char('h');
    input.insert_char('i');
    input.delete_char();
    assert_eq!(input.buffer, "h");
    assert_eq!(input.cursor_position, 1);
}

#[test]
fn test_input_delete_char_at_start_is_noop() {
    let mut input = InputState::new();
    input.delete_char(); // should not panic
    assert_eq!(input.buffer, "");
    assert_eq!(input.cursor_position, 0);
}

#[test]
fn test_input_cursor_movement() {
    let mut input = InputState::new();
    input.insert_char('a');
    input.insert_char('b');
    input.insert_char('c');
    // cursor at 3 (end)
    input.move_cursor_left();
    assert_eq!(input.cursor_position, 2);
    input.insert_char('X');
    // inserted at position 2, so "abXc"
    assert_eq!(input.buffer, "abXc");
    assert_eq!(input.cursor_position, 3);
    input.move_cursor_right();
    assert_eq!(input.cursor_position, 4);
}

#[test]
fn test_input_cursor_move_left_at_start_is_noop() {
    let mut input = InputState::new();
    input.move_cursor_left(); // should not panic
    assert_eq!(input.cursor_position, 0);
}

#[test]
fn test_input_cursor_move_right_at_end_is_noop() {
    let mut input = InputState::new();
    input.insert_char('a');
    input.move_cursor_right(); // already at end, should not panic
    assert_eq!(input.cursor_position, 1);
}

#[test]
fn test_input_take_clears_buffer() {
    let mut input = InputState::new();
    input.insert_char('h');
    input.insert_char('i');
    let taken = input.take_input();
    assert_eq!(taken, "hi");
    assert!(input.is_empty());
    assert_eq!(input.cursor_position, 0);
}

// ---------------------------------------------------------------------------
// Command tests
// ---------------------------------------------------------------------------

#[test]
fn test_parse_model_command() {
    let cmd = Command::parse("/model claude-opus");
    assert_eq!(cmd, Some(Command::Model("claude-opus".to_string())));
}

#[test]
fn test_parse_model_command_no_arg() {
    // "/model" with no argument produces Model with empty string
    let cmd = Command::parse("/model");
    assert_eq!(cmd, Some(Command::Model("".to_string())));
}

#[test]
fn test_parse_quit_command() {
    assert_eq!(Command::parse("/quit"), Some(Command::Quit));
    assert_eq!(Command::parse("/q"), Some(Command::Quit));
}

#[test]
fn test_parse_cost_command() {
    assert_eq!(Command::parse("/cost"), Some(Command::Cost));
}

#[test]
fn test_parse_help_command() {
    assert_eq!(Command::parse("/help"), Some(Command::Help));
}

#[test]
fn test_parse_undo_redo() {
    assert_eq!(Command::parse("/undo"), Some(Command::Undo));
    assert_eq!(Command::parse("/redo"), Some(Command::Redo));
}

#[test]
fn test_parse_unknown_command() {
    assert_eq!(
        Command::parse("/foobar"),
        Some(Command::Unknown("foobar".to_string()))
    );
}

#[test]
fn test_parse_non_command() {
    assert_eq!(Command::parse("hello world"), None);
    assert_eq!(Command::parse(""), None);
    assert_eq!(Command::parse("  /quit"), None); // leading space → not a command
}

// ---------------------------------------------------------------------------
// App state tests
// ---------------------------------------------------------------------------

#[test]
fn test_app_initial_state() {
    let app = App::new();
    assert!(matches!(app.state(), AppState::Idle));
    assert!(app.messages().is_empty());
    assert!(app.input().is_empty());
    assert!(!app.should_quit());
}

#[test]
fn test_app_handle_token_event() {
    let mut app = App::new();
    app.handle_agent_event(AgentEvent::Token {
        text: "Hello".to_string(),
    });
    app.handle_agent_event(AgentEvent::Token {
        text: ", world".to_string(),
    });
    let msgs = app.messages();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "Hello, world");
    assert!(msgs[0].is_streaming);
    assert!(matches!(msgs[0].role, DisplayRole::Assistant));
}

#[test]
fn test_app_handle_done_event() {
    let mut app = App::new();
    app.handle_agent_event(AgentEvent::Token {
        text: "Hello".to_string(),
    });
    app.handle_agent_event(AgentEvent::Done {
        usage: TokenUsage {
            input: 10,
            output: 5,
            cache_read: 0,
        },
    });
    assert!(matches!(app.state(), AppState::Idle));
    let msgs = app.messages();
    assert_eq!(msgs.len(), 1);
    assert!(!msgs[0].is_streaming);
}

#[test]
fn test_app_handle_error_event() {
    let mut app = App::new();
    app.handle_agent_event(AgentEvent::Error {
        message: "something broke".to_string(),
    });
    assert!(matches!(app.state(), AppState::Error(_)));
    let msgs = app.messages();
    assert_eq!(msgs.len(), 1);
    assert!(matches!(msgs[0].role, DisplayRole::Error));
    assert_eq!(msgs[0].content, "something broke");
}

#[test]
fn test_app_handle_tool_events() {
    let mut app = App::new();
    app.handle_agent_event(AgentEvent::ToolCallStart {
        id: "call_1".to_string(),
        name: "read_file".to_string(),
    });
    app.handle_agent_event(AgentEvent::ToolCallEnd {
        id: "call_1".to_string(),
        result: serde_json::json!({"content": "file content"}),
        is_error: false,
    });
    let msgs = app.messages();
    assert_eq!(msgs.len(), 2);
    assert!(matches!(&msgs[0].role, DisplayRole::ToolCall { name } if name == "read_file"));
    assert!(matches!(msgs[1].role, DisplayRole::ToolResult));
}

#[test]
fn test_app_submit_input() {
    let mut app = App::new();
    // Simulate typing and submitting
    app.type_text("Hello agent");
    app.submit_input();
    let msgs = app.messages();
    assert_eq!(msgs.len(), 1);
    assert!(matches!(msgs[0].role, DisplayRole::User));
    assert_eq!(msgs[0].content, "Hello agent");
    assert!(app.input().is_empty());
    assert!(matches!(app.state(), AppState::WaitingForResponse));
}

#[test]
fn test_app_submit_empty_input_is_noop() {
    let mut app = App::new();
    app.submit_input();
    assert!(app.messages().is_empty());
    assert!(matches!(app.state(), AppState::Idle));
}

#[test]
fn test_app_submit_command_does_not_add_user_message() {
    let mut app = App::new();
    app.type_text("/quit");
    app.submit_input();
    // /quit should set should_quit, not add a user message
    assert!(app.should_quit());
    assert!(app.messages().is_empty());
}

// ---------------------------------------------------------------------------
// Theme tests
// ---------------------------------------------------------------------------

#[test]
fn test_default_theme() {
    use ratatui::style::Color;
    let theme = Theme::default();
    // None of the colors should be plain Black (Color::Black)
    assert_ne!(theme.user_color, Color::Black);
    assert_ne!(theme.assistant_color, Color::Black);
    assert_ne!(theme.system_color, Color::Black);
    assert_ne!(theme.error_color, Color::Black);
    assert_ne!(theme.tool_color, Color::Black);
    // status_bg can be anything, just ensure Theme::default() doesn't panic
    let _ = theme.status_bg;
}
