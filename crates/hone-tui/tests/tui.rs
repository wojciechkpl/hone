use hone_proto::{AgentEvent, TokenUsage};
use hone_tui::{
    AnimationState, FocusTarget, LayoutMode,
    app::{App, AppState},
    commands::Command,
    input::InputState,
    layout,
    message::DisplayRole,
    render::root,
    theme::Theme,
};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

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
        args: serde_json::json!({"path": "src/main.rs"}),
    });
    app.handle_agent_event(AgentEvent::ToolCallEnd {
        id: "call_1".to_string(),
        result: serde_json::json!({"content": "file content"}),
        is_error: false,
        duration_ms: 100,
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
    assert_ne!(theme.role_user, Color::Black);
    assert_ne!(theme.role_assistant, Color::Black);
    assert_ne!(theme.role_system, Color::Black);
    assert_ne!(theme.role_error, Color::Black);
    assert_ne!(theme.role_tool, Color::Black);
    assert_ne!(theme.surface_raised, Color::Black);
    assert_eq!(theme.accent_hot, Color::Indexed(204));
    assert!(!theme.no_color);
}

#[test]
fn test_theme_crimson_accent() {
    use ratatui::style::Color;
    assert_eq!(Theme::crimson().accent_hot, Color::Indexed(204));
    assert_eq!(Theme::crimson().accent_mid, Color::Indexed(197));
}

#[test]
fn test_theme_minimal_no_color() {
    let theme = Theme::minimal();
    assert!(theme.no_color);
    assert_eq!(theme.accent_hot, ratatui::style::Color::Reset);
}

#[test]
fn test_theme_all_presets_construct() {
    let _ = Theme::crimson();
    let _ = Theme::neon();
    let _ = Theme::chalk();
    let _ = Theme::minimal();
}

// ---------------------------------------------------------------------------
// Animation tests
// ---------------------------------------------------------------------------

#[test]
fn test_app_tick_increments_frame() {
    let mut app = App::new();
    assert_eq!(app.animation().frame(), 0);
    app.tick();
    assert_eq!(app.animation().frame(), 1);
    app.tick();
    assert_eq!(app.animation().frame(), 2);
}

// ---------------------------------------------------------------------------
// Layout tests
// ---------------------------------------------------------------------------

#[test]
fn test_app_default_layout() {
    let app = App::new();
    assert_eq!(*app.layout(), LayoutMode::SinglePane);
}

// ---------------------------------------------------------------------------
// New command tests
// ---------------------------------------------------------------------------

#[test]
fn test_command_split() {
    assert_eq!(Command::parse("/split"), Some(Command::Split));
}

#[test]
fn test_command_panel() {
    assert_eq!(Command::parse("/panel"), Some(Command::Panel));
}

#[test]
fn test_command_theme() {
    assert_eq!(
        Command::parse("/theme neon"),
        Some(Command::ThemeSwitch("neon".to_string()))
    );
}

#[test]
fn test_command_completions() {
    let matches = Command::completions("mo");
    assert!(matches.contains(&"model"));
    assert!(!matches.contains(&"quit"));
}

// ---------------------------------------------------------------------------
// App integration: new features
// ---------------------------------------------------------------------------

#[test]
fn test_help_toggle() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App::new();
    assert!(!app.show_help());
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.show_help());
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(!app.show_help());
}

#[test]
fn test_tab_cycles_focus() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App::new();
    assert_eq!(*app.focus(), FocusTarget::Input);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(*app.focus(), FocusTarget::Conversation);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(*app.focus(), FocusTarget::Preview);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(*app.focus(), FocusTarget::Input);
}

#[test]
fn test_split_command_toggles_layout() {
    let mut app = App::new();
    app.type_text("/split");
    app.submit_input();
    assert_eq!(*app.layout(), LayoutMode::SplitHorizontal { ratio: 50 });
    app.type_text("/split");
    app.submit_input();
    assert_eq!(*app.layout(), LayoutMode::SinglePane);
}

#[test]
fn test_theme_command_switches() {
    let mut app = App::new();
    app.type_text("/theme neon");
    app.submit_input();
    assert_eq!(app.theme().accent_hot, ratatui::style::Color::Indexed(197));
}

#[test]
fn test_page_up_down_scroll() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let mut app = App::new();
    assert_eq!(app.scroll_offset(), 0);
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert_eq!(app.scroll_offset(), 10);
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert_eq!(app.scroll_offset(), 0);
}

// ---------------------------------------------------------------------------
// Phase 7-10 tests
// ---------------------------------------------------------------------------

#[test]
fn test_root_render_does_not_panic() {
    let mut app = App::new();
    app.skip_splash();
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| root::render(&app, frame))
        .expect("draw");
}

#[test]
fn test_too_small_terminal() {
    let mut app = App::new();
    app.skip_splash();
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| root::render(&app, frame))
        .expect("draw");
}

#[test]
fn test_split_layout_render() {
    let mut app = App::new();
    app.skip_splash();
    app.type_text("/split");
    app.submit_input();
    assert_eq!(*app.layout(), LayoutMode::SplitHorizontal { ratio: 50 });
    let backend = TestBackend::new(120, 40);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| root::render(&app, frame))
        .expect("draw");
}

#[test]
fn test_splash_auto_dismiss() {
    use std::time::{Duration, Instant};

    let mut app = App::new();
    assert!(app.show_splash());

    // Force splash_start to 700ms ago so the next tick triggers dismissal.
    app.set_splash_start(Instant::now() - Duration::from_millis(700));

    app.tick();
    assert!(!app.show_splash(), "splash should be dismissed after 700ms");
}

#[test]
fn test_skip_splash() {
    let mut app = App::new();
    assert!(app.show_splash());
    app.skip_splash();
    assert!(!app.show_splash());
}

#[test]
fn test_file_preview_state() {
    let mut app = App::new();
    assert!(app.preview_content().is_none());
    app.set_preview(Some("fn main() {}".to_string()), None);
    assert_eq!(app.preview_content(), Some("fn main() {}"));
}

#[test]
fn test_theme_preview_command() {
    assert_eq!(Command::parse("/theme-preview"), Some(Command::ThemePreview));
}

#[test]
fn test_theme_preview_command_pushes_system_message() {
    let mut app = App::new();
    app.type_text("/theme-preview");
    app.submit_input();
    let msgs = app.messages();
    assert_eq!(msgs.len(), 1);
    assert!(matches!(msgs[0].role, DisplayRole::System));
    assert!(msgs[0].content.contains("Theme preview"));
}

// ---------------------------------------------------------------------------
// Cost and active_tool tracking tests
// ---------------------------------------------------------------------------

#[test]
fn test_cost_updates_on_done() {
    let mut app = App::new();
    app.handle_agent_event(AgentEvent::Token {
        text: "Hello".to_string(),
    });
    app.handle_agent_event(AgentEvent::Done {
        usage: TokenUsage {
            input: 1000,
            output: 500,
            cache_read: 0,
        },
    });
    // input: 1000 * $3/M = $0.003, output: 500 * $15/M = $0.0075 → total $0.0105
    assert!(
        app.status().session_cost_usd > 0.0,
        "session_cost_usd should be positive after Done"
    );
    assert_eq!(app.status().token_count, 1500);
}

#[test]
fn test_active_tool_tracked() {
    let mut app = App::new();
    app.handle_agent_event(AgentEvent::ToolCallStart {
        id: "call_1".to_string(),
        name: "read_file".to_string(),
        args: serde_json::json!({"path": "src/lib.rs"}),
    });
    assert_eq!(
        app.status().active_tool.as_deref(),
        Some("read_file"),
        "active_tool should be set after ToolCallStart"
    );
    app.handle_agent_event(AgentEvent::ToolCallEnd {
        id: "call_1".to_string(),
        result: serde_json::json!("ok"),
        is_error: false,
        duration_ms: 50,
    });
    assert!(
        app.status().active_tool.is_none(),
        "active_tool should be cleared after ToolCallEnd"
    );
}
