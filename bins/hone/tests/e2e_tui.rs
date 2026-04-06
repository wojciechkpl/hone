//! End-to-end TUI simulation tests.
//!
//! These tests exercise the full App lifecycle using ratatui's `TestBackend`
//! so that no real terminal is required.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use hone_proto::{AgentEvent, TokenUsage};
use hone_tui::commands::Command;
use hone_tui::render::root;
use hone_tui::{App, AppState};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Create a terminal with a `TestBackend` at the given dimensions.
fn make_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    Terminal::new(backend).expect("failed to create test terminal")
}

/// Simulate pressing a plain character key.
fn char_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// Simulate pressing Enter.
fn enter_key() -> KeyEvent {
    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)
}

/// Type a string into the App one character at a time via `handle_key`.
fn type_string(app: &mut App, text: &str) {
    for c in text.chars() {
        app.handle_key(char_key(c));
    }
}

/// Draw the App into the terminal, panicking on failure.
fn draw(app: &App, terminal: &mut Terminal<TestBackend>) {
    terminal
        .draw(|frame| root::render(app, frame))
        .expect("render must not panic");
}

/// Extract all text from the terminal buffer as a single string.
fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
    let buf = terminal.backend().buffer();
    let mut text = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            let cell = &buf[(x, y)];
            text.push_str(cell.symbol());
        }
        text.push('\n');
    }
    text
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_full_lifecycle() {
    let mut terminal = make_terminal(120, 40);
    let mut app = App::new();
    app.skip_splash();

    // 1. Initial render: no panic
    draw(&app, &mut terminal);

    // 2. Type "hello world" via individual key events
    type_string(&mut app, "hello world");
    assert_eq!(app.input().buffer, "hello world");

    // 3. Render with input: no panic
    draw(&app, &mut terminal);

    // 4. Press Enter: state transitions to WaitingForResponse, user message appears
    app.handle_key(enter_key());
    assert_eq!(*app.state(), AppState::WaitingForResponse);
    assert_eq!(app.messages().len(), 1);
    assert_eq!(app.messages()[0].content, "hello world");

    // 5. Simulate receiving AgentEvent::Token
    app.handle_agent_event(AgentEvent::Token {
        text: "Hello! ".to_string(),
    });
    assert_eq!(app.messages().len(), 2);
    assert!(app.messages()[1].content.contains("Hello!"));
    assert!(app.messages()[1].is_streaming);

    // 6. Simulate more tokens
    app.handle_agent_event(AgentEvent::Token {
        text: "How can I help?".to_string(),
    });
    assert_eq!(app.messages()[1].content, "Hello! How can I help?");

    // 7. Simulate AgentEvent::Done
    app.handle_agent_event(AgentEvent::Done {
        usage: TokenUsage {
            input: 10,
            output: 5,
            cache_read: 0,
        },
    });
    assert_eq!(*app.state(), AppState::Idle);
    assert!(!app.messages()[1].is_streaming);

    // 8. Render after conversation: no panic, conversation text visible
    draw(&app, &mut terminal);
    let text = buffer_text(&terminal);
    assert!(
        text.contains("hello world"),
        "conversation area must contain user message"
    );
    assert!(
        text.contains("Hello!"),
        "conversation area must contain assistant response"
    );
}

#[test]
fn test_slash_help_command() {
    let mut app = App::new();
    app.skip_splash();

    type_string(&mut app, "/help");
    app.handle_key(enter_key());

    // /help should produce a system message and stay Idle
    assert_eq!(*app.state(), AppState::Idle);
    assert!(!app.messages().is_empty());
    let last = app.messages().last().unwrap();
    assert!(
        last.content.contains("/quit") || last.content.contains("/help"),
        "help message must contain command references"
    );
}

#[test]
fn test_slash_quit_command() {
    let mut app = App::new();
    app.skip_splash();

    type_string(&mut app, "/quit");
    app.handle_key(enter_key());

    assert!(app.should_quit(), "app must quit after /quit");
}

#[test]
fn test_command_completions() {
    let matches = Command::completions("mo");
    assert!(
        matches.contains(&"model"),
        "completions for 'mo' must include 'model'"
    );
}

#[test]
fn test_empty_enter_is_noop() {
    let mut app = App::new();
    app.skip_splash();

    // Pressing Enter on an empty input should not add a message.
    let msg_count_before = app.messages().len();
    app.handle_key(enter_key());
    assert_eq!(app.messages().len(), msg_count_before);
    assert_eq!(*app.state(), AppState::Idle);
}

#[test]
fn test_long_input_no_panic() {
    let mut terminal = make_terminal(120, 40);
    let mut app = App::new();
    app.skip_splash();

    // Type 500+ characters
    let long_text: String = "a".repeat(550);
    app.type_text(&long_text);
    assert_eq!(app.input().buffer.len(), 550);

    // Render must not panic
    draw(&app, &mut terminal);
}

#[test]
fn test_rapid_keys_no_state_corruption() {
    let mut app = App::new();
    app.skip_splash();

    // Rapid fire: type, backspace, type again
    for _ in 0..100 {
        app.handle_key(char_key('x'));
    }
    for _ in 0..50 {
        app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    }
    assert_eq!(app.input().buffer.len(), 50);

    // Type and submit
    type_string(&mut app, " done");
    app.handle_key(enter_key());
    assert_eq!(*app.state(), AppState::WaitingForResponse);
}

#[test]
fn test_tiny_terminal_no_panic() {
    let mut terminal = make_terminal(40, 10);
    let mut app = App::new();
    app.skip_splash();

    // Should render the "too small" message instead of panicking
    draw(&app, &mut terminal);

    let text = buffer_text(&terminal);
    assert!(
        text.contains("too small"),
        "small terminal must show the 'too small' message"
    );
}

#[test]
fn test_splash_auto_dismiss() {
    let mut app = App::new();
    assert!(app.show_splash(), "splash should show on startup");

    // Set splash start to far in the past so tick() will dismiss it.
    app.set_splash_start(std::time::Instant::now() - std::time::Duration::from_secs(5));
    app.tick();
    assert!(!app.show_splash(), "splash should auto-dismiss after duration");
}

#[test]
fn test_splash_render_no_panic() {
    let mut terminal = make_terminal(120, 40);
    let app = App::new();
    // Splash is shown by default
    assert!(app.show_splash());
    draw(&app, &mut terminal);
}

#[test]
fn test_render_with_messages_at_exact_boundary() {
    // Render with exactly 80x24 (the minimum size)
    let mut terminal = make_terminal(80, 24);
    let mut app = App::new();
    app.skip_splash();

    app.type_text("test message");
    app.handle_key(enter_key());
    app.handle_agent_event(AgentEvent::Token {
        text: "response text".to_string(),
    });
    app.handle_agent_event(AgentEvent::Done {
        usage: TokenUsage::default(),
    });

    draw(&app, &mut terminal);
}

#[test]
fn test_error_state_and_recovery() {
    let mut app = App::new();
    app.skip_splash();

    // Send a user message
    app.type_text("question");
    app.handle_key(enter_key());
    assert_eq!(*app.state(), AppState::WaitingForResponse);

    // Simulate an error
    app.handle_agent_event(AgentEvent::Error {
        message: "connection lost".to_string(),
    });
    assert!(matches!(app.state(), AppState::Error(_)));

    // User can type again and submit (Error state allows Enter in main.rs)
    app.type_text("retry");
    app.handle_key(enter_key());
    assert_eq!(*app.state(), AppState::WaitingForResponse);
}

#[test]
fn test_scroll_and_focus() {
    let mut app = App::new();
    app.skip_splash();

    // PageUp
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert_eq!(app.scroll_offset(), 10);

    // PageDown
    app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
    assert_eq!(app.scroll_offset(), 0);

    // Tab cycles focus
    use hone_tui::FocusTarget;
    assert_eq!(*app.focus(), FocusTarget::Input);
    app.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(*app.focus(), FocusTarget::Conversation);
}

#[test]
fn test_render_with_scroll_indicator() {
    let mut terminal = make_terminal(120, 40);
    let mut app = App::new();
    app.skip_splash();

    // Add many messages
    for i in 0..20 {
        app.type_text(&format!("message {i}"));
        app.handle_key(enter_key());
        app.handle_agent_event(AgentEvent::Token {
            text: format!("reply {i}"),
        });
        app.handle_agent_event(AgentEvent::Done {
            usage: TokenUsage::default(),
        });
    }

    // Scroll up
    app.handle_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
    assert!(app.scroll_offset() > 0);

    draw(&app, &mut terminal);
}

#[test]
fn test_theme_switch_command() {
    let mut app = App::new();
    app.skip_splash();

    type_string(&mut app, "/theme neon");
    app.handle_key(enter_key());

    assert_eq!(*app.state(), AppState::Idle);
    let last = app.messages().last().unwrap();
    assert!(last.content.contains("neon"));
}

#[test]
fn test_unknown_command() {
    let mut app = App::new();
    app.skip_splash();

    type_string(&mut app, "/foobar");
    app.handle_key(enter_key());

    let last = app.messages().last().unwrap();
    assert!(last.content.contains("Unknown command"));
}

#[test]
fn test_ctrl_c_quits() {
    let mut app = App::new();
    app.skip_splash();

    app.handle_key(KeyEvent::new(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
    ));
    assert!(app.should_quit());
}

#[test]
fn test_backspace_and_cursor_movement() {
    let mut app = App::new();
    app.skip_splash();

    type_string(&mut app, "abc");
    assert_eq!(app.input().buffer, "abc");

    // Backspace removes last char
    app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.input().buffer, "ab");

    // Left arrow, then type inserts in middle
    app.handle_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.handle_key(char_key('X'));
    assert_eq!(app.input().buffer, "aXb");
}

#[test]
fn test_help_overlay_toggle() {
    let mut terminal = make_terminal(120, 40);
    let mut app = App::new();
    app.skip_splash();

    assert!(!app.show_help());
    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(app.show_help());

    // Render with help overlay: no panic
    draw(&app, &mut terminal);

    app.handle_key(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert!(!app.show_help());
}

#[test]
fn test_split_layout_render() {
    let mut terminal = make_terminal(120, 40);
    let mut app = App::new();
    app.skip_splash();

    type_string(&mut app, "/split");
    app.handle_key(enter_key());

    draw(&app, &mut terminal);
}

#[test]
fn test_tool_call_events() {
    let mut terminal = make_terminal(120, 40);
    let mut app = App::new();
    app.skip_splash();

    app.type_text("do something");
    app.handle_key(enter_key());

    app.handle_agent_event(AgentEvent::ToolCallStart {
        id: "call_1".to_string(),
        name: "read_file".to_string(),
    });
    app.handle_agent_event(AgentEvent::ToolCallEnd {
        id: "call_1".to_string(),
        result: serde_json::json!("file contents here"),
        is_error: false,
    });
    app.handle_agent_event(AgentEvent::Token {
        text: "I read the file.".to_string(),
    });
    app.handle_agent_event(AgentEvent::Done {
        usage: TokenUsage::default(),
    });

    draw(&app, &mut terminal);
}

#[test]
fn test_bottom_panel_layout() {
    let mut terminal = make_terminal(120, 40);
    let mut app = App::new();
    app.skip_splash();

    type_string(&mut app, "/panel");
    app.handle_key(enter_key());

    draw(&app, &mut terminal);
}

#[test]
fn test_cost_command() {
    let mut app = App::new();
    app.skip_splash();

    type_string(&mut app, "/cost");
    app.handle_key(enter_key());

    let last = app.messages().last().unwrap();
    assert!(last.content.contains("Session cost"));
}
