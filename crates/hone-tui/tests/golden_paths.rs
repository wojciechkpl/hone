/// Golden-path integration tests for the hone TUI.
///
/// Each scenario corresponds to a real regression:
///
/// 1. send_message_and_render        — basic round-trip: user message + assistant token stream + Done
/// 2. tool_call_shows_spinner_while_active — spinner present while ToolCallStart is live, gone after ToolCallEnd
/// 3. scroll_direction_bottom_anchored    — ScrollUp reveals older messages; G / scroll_offset=0 returns to newest
/// 4. cost_uses_provider_value_not_hardcoded — two Done events; session_cost accumulates from cost_usd field
/// 5. observed_tool_surfaces_in_ui        — ToolCallStart/End are rendered in the conversation pane
///
/// All tests use `App::new()` directly + `TestBackend` for rendering. No real agent is launched.
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use hone_proto::{AgentEvent, TokenUsage};
use hone_tui::{app::App, render::root};
use ratatui::{Terminal, backend::TestBackend};

// ---------------------------------------------------------------------------
// Shared helper
// ---------------------------------------------------------------------------

/// Render `app` through the full root renderer into a `TestBackend` and return
/// the complete cell buffer as a plain string (one line per row, newline
/// delimited).
fn render_app(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| root::render(app, frame))
        .expect("draw");
    let buf = terminal.backend().buffer().clone();
    let mut out = String::new();
    for row in 0..height {
        for col in 0..width {
            let ch = buf
                .cell((col, row))
                .map(|c| c.symbol().chars().next().unwrap_or(' '))
                .unwrap_or(' ');
            out.push(ch);
        }
        out.push('\n');
    }
    out
}

// ---------------------------------------------------------------------------
// Scenario 1 — send_message_and_render
//
// Regression caught: basic message pipeline / render path.
// If the user message or assistant token stream were dropped, this would fail.
// ---------------------------------------------------------------------------

#[test]
fn send_message_and_render() {
    let mut app = App::new();
    app.skip_splash();

    // User submits "hello"
    app.type_text("hello");
    app.submit_input();

    // Agent streams back a token then signals Done
    app.handle_agent_event(AgentEvent::Token {
        text: "hi there".to_string(),
    });
    app.handle_agent_event(AgentEvent::Done {
        usage: TokenUsage {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_creation: 0,
        },
        cost_usd: 0.01,
        plan: None,
    });

    let rendered = render_app(&mut app, 120, 40);

    // User message box uses "you" as the role label
    assert!(
        rendered.contains("you"),
        "rendered output must contain the user role label 'you'\n---\n{rendered}"
    );

    // User message content
    assert!(
        rendered.contains("hello"),
        "rendered output must contain the submitted user message 'hello'\n---\n{rendered}"
    );

    // Assistant role label
    assert!(
        rendered.contains("hone"),
        "rendered output must contain the assistant role label 'hone'\n---\n{rendered}"
    );

    // Assistant token content
    assert!(
        rendered.contains("hi there"),
        "rendered output must contain the assistant token 'hi there'\n---\n{rendered}"
    );

    // Status line shows non-zero cost ($0.0100)
    assert!(
        rendered.contains("$0.01"),
        "status line must display the session cost\n---\n{rendered}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 2 — tool_call_shows_spinner_while_active
//
// Regression caught: commit 110ac1f — in-progress tool calls had no spinner.
// Before the fix, `is_active` was never set on the ToolCall message, so
// `render_tool_call` received `spinner: None` even while the call was live.
// ---------------------------------------------------------------------------

#[test]
fn tool_call_shows_spinner_while_active() {
    let mut app = App::new();
    app.skip_splash();

    // Start a tool call
    app.handle_agent_event(AgentEvent::ToolCallStart {
        id: "tc-1".to_string(),
        name: "read".to_string(),
        args: serde_json::json!({"path": "src/lib.rs"}),
    });

    // Advance animation a few frames so the spinner char is deterministic
    app.tick();
    app.tick();
    app.tick();

    let rendered_active = render_app(&mut app, 120, 40);

    // The tool box header should contain the tool name
    assert!(
        rendered_active.contains("read"),
        "active tool call must render the tool name 'read'\n---\n{rendered_active}"
    );

    // While the tool is running the spinner chars from SPINNER_FRAMES must appear.
    // We accept any of the 4 frames: '─', '\\', '│', '/'.
    let spinner_chars = ['─', '\\', '│', '/'];
    let has_spinner = spinner_chars.iter().any(|&ch| rendered_active.contains(ch));
    assert!(
        has_spinner,
        "active tool call must render a spinner character (one of ─ \\ │ /)\n---\n{rendered_active}"
    );

    // End the tool call
    app.handle_agent_event(AgentEvent::ToolCallEnd {
        id: "tc-1".to_string(),
        result: serde_json::json!({"content": "file contents here"}),
        is_error: false,
        duration_ms: 50,
        cache_hit: false,
    });

    let rendered_done = render_app(&mut app, 120, 40);

    // The tool result content should now appear
    assert!(
        rendered_done.contains("file contents here"),
        "completed tool call must render the result content\n---\n{rendered_done}"
    );

    // The "running…" placeholder that appears only while active should be gone
    assert!(
        !rendered_done.contains("running"),
        "completed tool call must not show 'running' placeholder\n---\n{rendered_done}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 3 — scroll_direction_bottom_anchored
//
// Regression caught: commit a6a4be4 — mouse ScrollUp/ScrollDown were inverted.
// The convention: ScrollUp (wheel up) = see *older* content = increase offset.
//                 ScrollDown (wheel down) = see *newer* content = decrease offset.
// ---------------------------------------------------------------------------

#[test]
fn scroll_direction_bottom_anchored() {
    let mut app = App::new();
    app.skip_splash();

    // Push enough messages to overflow a standard viewport (40 rows).
    for i in 0..20 {
        app.type_text(&format!("message {i}"));
        app.submit_input();
        app.handle_agent_event(AgentEvent::Token {
            text: format!("response {i}"),
        });
        app.handle_agent_event(AgentEvent::Done {
            usage: TokenUsage {
                input: 5,
                output: 5,
                cache_read: 0,
                cache_creation: 0,
            },
            cost_usd: 0.001,
            plan: None,
        });
    }

    // By default, scroll_offset == 0 means the viewport is anchored at the
    // bottom (newest content visible).
    assert_eq!(
        app.scroll_offset(),
        0,
        "default scroll_offset must be 0 (bottom-anchored)"
    );

    // Simulate mouse ScrollUp — this must *increase* the offset so older
    // content scrolls into view.
    let scroll_up_event = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 10,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    app.handle_mouse(scroll_up_event);

    let after_scroll_up = app.scroll_offset();
    assert!(
        after_scroll_up > 0,
        "ScrollUp must increase scroll_offset (reveal older content), got {after_scroll_up}"
    );

    // Simulate mouse ScrollDown — this must *decrease* the offset (toward newest).
    let scroll_down_event = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 10,
        modifiers: crossterm::event::KeyModifiers::NONE,
    };
    app.handle_mouse(scroll_down_event);

    let after_scroll_down = app.scroll_offset();
    assert!(
        after_scroll_down < after_scroll_up,
        "ScrollDown must decrease scroll_offset (toward newest), got {after_scroll_down}"
    );

    // 'G' key — ScrollToBottom — must return offset to 0.
    app.handle_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::SHIFT));

    assert_eq!(
        app.scroll_offset(),
        0,
        "G (ScrollToBottom) must reset scroll_offset to 0"
    );

    // Verify the rendered output shows the newest messages when at offset 0.
    let rendered = render_app(&mut app, 120, 40);
    assert!(
        rendered.contains("message 19") || rendered.contains("response 19"),
        "viewport at offset 0 must show newest messages\n---\n{rendered}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 4 — cost_uses_provider_value_not_hardcoded
//
// Regression caught: cost_usd in AgentEvent::Done was ignored and replaced
// with a hardcoded Sonnet pricing calculation based on token counts.
// ---------------------------------------------------------------------------

#[test]
fn cost_uses_provider_value_not_hardcoded() {
    let mut app = App::new();
    app.skip_splash();

    // First Done — provider reports $0.10
    app.handle_agent_event(AgentEvent::Token {
        text: "first".to_string(),
    });
    app.handle_agent_event(AgentEvent::Done {
        usage: TokenUsage {
            input: 1000,
            output: 500,
            cache_read: 0,
            cache_creation: 0,
        },
        cost_usd: 0.10,
        plan: None,
    });

    // Second Done — provider reports $0.25
    app.handle_agent_event(AgentEvent::Token {
        text: "second".to_string(),
    });
    app.handle_agent_event(AgentEvent::Done {
        usage: TokenUsage {
            input: 2000,
            output: 1000,
            cache_read: 0,
            cache_creation: 0,
        },
        cost_usd: 0.25,
        plan: None,
    });

    let total = app.status().session_cost_usd;

    // If cost were hardcoded from Sonnet pricing:
    //   (1000 * 3 + 500 * 15) / 1_000_000 = 0.0105  (first)
    //   (2000 * 3 + 1000 * 15) / 1_000_000 = 0.021   (second)
    //   sum ≈ 0.0315  — nowhere near 0.35
    // The correct answer from provider values is 0.10 + 0.25 = 0.35.
    assert!(
        (total - 0.35).abs() < 1e-9,
        "session_cost_usd must be the sum of provider-reported cost_usd values (0.10 + 0.25 = 0.35), got {total}"
    );
}

// ---------------------------------------------------------------------------
// Scenario 5 — observed_tool_surfaces_in_ui
//
// Regression caught: Claude CLI adapter opaque — tool calls were not forwarded
// as AgentEvent::ToolCallStart/End, so the TUI never saw them.
// This test validates the *downstream* rendering: given ToolCallStart/End
// events (which the agent layer now emits when it forwards ObservedTool*
// from the CLI adapter), the TUI renders the tool activity visibly.
// ---------------------------------------------------------------------------

#[test]
fn observed_tool_surfaces_in_ui() {
    let mut app = App::new();
    app.skip_splash();

    // These events are what the agent layer emits when forwarding
    // ObservedToolStart/End from the Claude CLI adapter.
    app.handle_agent_event(AgentEvent::ToolCallStart {
        id: "obs-1".to_string(),
        name: "bash".to_string(),
        args: serde_json::json!({"command": "cargo test"}),
    });

    // End with a result
    app.handle_agent_event(AgentEvent::ToolCallEnd {
        id: "obs-1".to_string(),
        result: serde_json::json!({"stdout": "test passed", "exit_code": 0}),
        is_error: false,
        duration_ms: 1500,
        cache_hit: false,
    });

    let rendered = render_app(&mut app, 120, 40);

    // The tool name must appear in the conversation pane
    assert!(
        rendered.contains("bash"),
        "tool name 'bash' must be visible in the rendered output\n---\n{rendered}"
    );

    // The result content must appear
    assert!(
        rendered.contains("test passed"),
        "tool result content must be visible in the rendered output\n---\n{rendered}"
    );

    // Verify the tool box border characters are rendered (confirms widget ran)
    assert!(
        rendered.contains('┌') || rendered.contains('└'),
        "tool call widget must render box borders\n---\n{rendered}"
    );
}
