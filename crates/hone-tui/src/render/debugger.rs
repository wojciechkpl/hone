//! Agent Debugger overlay — shown when step mode is active and a tool call
//! is paused waiting for user input.
//!
//! The overlay occupies the full terminal area and shows:
//!   - The tool name (in accent colour)
//!   - Pretty-printed JSON arguments (capped at 20 lines)
//!   - A footer with keybinding hints

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::app::App;

/// Render the debugger overlay into `area`.
///
/// This is called from `render/root.rs` at the very end (after all other
/// widgets) so it appears on top of everything else.
pub fn render_debugger(app: &App, frame: &mut Frame, area: Rect) {
    let theme = app.theme();

    let border_style = Style::default().fg(theme.accent_hot);
    let title_style = Style::default()
        .fg(theme.accent_hot)
        .add_modifier(Modifier::BOLD);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled(
            " Agent Debugger \u{2014} Tool Call Paused ",
            title_style,
        ));

    // Inner area after the border.
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height < 3 {
        return;
    }

    // Build the content lines.
    let mut lines: Vec<Line> = Vec::new();

    // Tool name line.
    let tool_name = app.debugger_paused_name.as_deref().unwrap_or("(unknown)");

    let name_style = if theme.no_color {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme.accent_hot)
            .add_modifier(Modifier::BOLD)
    };

    lines.push(Line::from(vec![
        Span::raw("  Tool: "),
        Span::styled(tool_name, name_style),
    ]));

    // Blank separator.
    lines.push(Line::from(""));

    // Pretty-printed JSON args (capped at 20 lines).
    let args_str = app
        .debugger_paused_args
        .as_ref()
        .and_then(|v| serde_json::to_string_pretty(v).ok())
        .unwrap_or_else(|| "{}".to_owned());

    let text_style = Style::default().fg(theme.text_primary);
    for arg_line in args_str.lines().take(20) {
        lines.push(Line::from(Span::styled(arg_line.to_owned(), text_style)));
    }

    // Blank separator before footer.
    lines.push(Line::from(""));

    // Footer with keybinding hints.
    let footer_style = Style::default().fg(theme.text_muted);
    lines.push(Line::from(Span::styled(
        "  Space/Enter proceed  s skip (inject stub)  Esc proceed",
        footer_style,
    )));

    // Split inner area: all lines go into the top, footer pinned to bottom.
    // We simply render all lines via a Paragraph; scrolling is not needed for
    // a short overlay.
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(0)])
        .split(inner);

    let para = Paragraph::new(lines);
    frame.render_widget(para, chunks[0]);
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use hone_proto::StepAction;

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Space sends Proceed and closes the debugger.
    #[tokio::test]
    async fn debugger_space_sends_proceed_and_closes() {
        let mut app = App::new();
        app.debugger_open = true;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Char(' ')));

        assert!(!app.debugger_open, "debugger should be closed after Space");
        let action = rx.try_recv().expect("should have sent a StepAction");
        assert_eq!(action, StepAction::Proceed);
    }

    /// Enter also sends Proceed and closes the debugger.
    #[tokio::test]
    async fn debugger_enter_sends_proceed_and_closes() {
        let mut app = App::new();
        app.debugger_open = true;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Enter));

        assert!(!app.debugger_open, "debugger should be closed after Enter");
        let action = rx.try_recv().expect("should have sent a StepAction");
        assert_eq!(action, StepAction::Proceed);
    }

    /// 's' sends Skip and closes the debugger.
    #[tokio::test]
    async fn debugger_s_sends_skip_and_closes() {
        let mut app = App::new();
        app.debugger_open = true;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Char('s')));

        assert!(!app.debugger_open, "debugger should be closed after 's'");
        let action = rx.try_recv().expect("should have sent a StepAction");
        assert_eq!(action, StepAction::Skip);
    }

    /// Esc sends Proceed (don't leave agent stuck) and closes the debugger.
    #[tokio::test]
    async fn debugger_esc_sends_proceed_and_closes() {
        let mut app = App::new();
        app.debugger_open = true;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Esc));

        assert!(!app.debugger_open, "debugger should be closed after Esc");
        let action = rx.try_recv().expect("should have sent a StepAction");
        assert_eq!(action, StepAction::Proceed);
    }

    // -----------------------------------------------------------------------
    // Red-phase tests: new keys not yet handled by the production code.
    //
    // These tests will FAIL until the green-phase implementer adds 'y', 'a',
    // 'd', and 'n' handling to `App::handle_key_debugger`.
    // -----------------------------------------------------------------------

    /// 'y' must send `StepAction::Proceed` and close the debugger.
    ///
    /// Red phase: 'y' currently falls through the `_ => {}` arm and sends
    /// nothing.  The `try_recv()` will return `Err(Empty)`, causing the test
    /// to panic with "should have sent a StepAction".
    #[tokio::test]
    async fn debugger_y_sends_proceed() {
        let mut app = App::new();
        app.debugger_open = true;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Char('y')));

        assert!(!app.debugger_open, "debugger should be closed after 'y'");
        let action = rx.try_recv().expect("should have sent a StepAction");
        assert_eq!(action, StepAction::Proceed);
    }

    /// 'a' must send `StepAction::ApproveAlways(<paused_tool_name>)` and close
    /// the debugger.
    ///
    /// The test pre-loads `app.debugger_paused_name` so the handler knows which
    /// tool name to embed in the `ApproveAlways` payload.
    ///
    /// Red phase: 'a' falls through to `_ => {}`.  No action is sent.
    #[tokio::test]
    async fn debugger_a_sends_approve_always() {
        let mut app = App::new();
        app.debugger_open = true;
        // Pre-load the paused tool name — the handler reads this to build the payload.
        app.debugger_paused_name = Some("shell.run".to_owned());

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Char('a')));

        assert!(!app.debugger_open, "debugger should be closed after 'a'");
        let action = rx.try_recv().expect("should have sent a StepAction");
        assert_eq!(
            action,
            StepAction::ApproveAlways("shell.run".to_owned()),
            "'a' should send ApproveAlways with the paused tool name"
        );
    }

    /// 'd' must send `StepAction::Deny(reason)` and close the debugger.
    /// The deny reason must be non-empty (any user-facing string is acceptable).
    ///
    /// Red phase: 'd' falls through to `_ => {}`.  No action is sent.
    #[tokio::test]
    async fn debugger_d_sends_deny() {
        let mut app = App::new();
        app.debugger_open = true;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Char('d')));

        assert!(!app.debugger_open, "debugger should be closed after 'd'");
        let action = rx.try_recv().expect("should have sent a StepAction");
        match action {
            StepAction::Deny(reason) => {
                assert!(!reason.is_empty(), "'d' deny reason must not be empty")
            }
            other => panic!("expected StepAction::Deny, got {other:?}"),
        }
    }

    /// 'n' must also send `StepAction::Deny(reason)` and close the debugger.
    ///
    /// Red phase: 'n' falls through to `_ => {}`.  No action is sent.
    #[tokio::test]
    async fn debugger_n_sends_deny() {
        let mut app = App::new();
        app.debugger_open = true;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Char('n')));

        assert!(!app.debugger_open, "debugger should be closed after 'n'");
        let action = rx.try_recv().expect("should have sent a StepAction");
        match action {
            StepAction::Deny(reason) => {
                assert!(!reason.is_empty(), "'n' deny reason must not be empty")
            }
            other => panic!("expected StepAction::Deny, got {other:?}"),
        }
    }
}
