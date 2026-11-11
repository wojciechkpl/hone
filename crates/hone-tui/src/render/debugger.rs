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
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
};

use crate::app::App;

/// Rows a bulk argument value (`content`, `new_string`, ...) may take before it
/// is cut.
const ARG_VALUE_ROWS: usize = 6;

/// Rows an identifying value (`command`, `path`, ...) may take: it decides what
/// runs or is touched, so its tail must stay visible far longer.
const ARG_IDENTIFYING_ROWS: usize = 20;

/// Keys listed before the others: what the call touches or runs.
const IDENTIFYING_KEYS: [&str; 5] = ["path", "file_path", "command", "url", "pattern"];

/// Cut `text` to `cap` chars; the cut part is counted in a trailing note.
fn capped(text: String, cap: usize) -> String {
    let total = text.chars().count();
    if total <= cap {
        return text;
    }
    let mut out: String = text.chars().take(cap).collect();
    out.push_str(&format!("\u{2026} ({} more chars)", total - cap));
    out
}

/// The argument lines of the menu: one `key: value` line per field of an
/// object (identifying keys first, capped at `ARG_IDENTIFYING_ROWS` rows of
/// `row_width` chars, the others at `ARG_VALUE_ROWS`); anything else is shown
/// as capped JSON.
fn arg_lines(args: Option<&serde_json::Value>, row_width: usize) -> Vec<String> {
    let cap = row_width * ARG_VALUE_ROWS;
    let id_cap = row_width * ARG_IDENTIFYING_ROWS;
    let render = |v: &serde_json::Value| serde_json::to_string(v).unwrap_or_default();
    match args {
        Some(serde_json::Value::Object(map)) if !map.is_empty() => {
            let first = IDENTIFYING_KEYS
                .iter()
                .filter_map(|k| map.get_key_value(*k))
                .map(|(k, v)| format!("  {k}: {}", capped(render(v), id_cap)));
            let rest = map
                .iter()
                .filter(|(k, _)| !IDENTIFYING_KEYS.contains(&k.as_str()))
                .map(|(k, v)| format!("  {k}: {}", capped(render(v), cap)));
            first.chain(rest).collect()
        }
        Some(v) => vec![format!("  {}", capped(render(v), cap))],
        None => vec!["  {}".to_owned()],
    }
}

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

    // Inner area after the border. Clear first: the menu sits over the
    // conversation, whose text must not show through.
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
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

    // The worker that asks (orchestrated runs): agent, subtask id and the
    // subtask's description when seen, cut to the menu width.
    if let Some(w) = &app.debugger_paused_worker {
        let mut text = format!("  Worker: {} ({})", w.agent, w.task_id);
        if let Some(d) = app.worker_descriptions.get(&w.task_id) {
            let d = d.split_whitespace().collect::<Vec<_>>().join(" ");
            // The description is planner-written task text: show it quoted.
            text.push_str(" \u{2014} \u{201c}");
            text.push_str(&d);
            text.push('\u{201d}');
        }
        let width = usize::from(inner.width);
        if text.chars().count() > width {
            text = text
                .chars()
                .take(width.saturating_sub(1))
                .collect::<String>()
                + "\u{2026}";
        }
        lines.push(Line::from(Span::styled(
            text,
            Style::default().fg(theme.text_muted),
        )));
    }

    // Blank separator.
    lines.push(Line::from(""));

    // Arguments: the fields that say what the call touches come first, and
    // every value is capped at a few wrapped rows, so a long `content` can't
    // push the target out of view.
    let text_style = Style::default().fg(theme.text_primary);
    let row_width = usize::from(inner.width.saturating_sub(4)).max(20);
    for arg_line in arg_lines(app.debugger_paused_args.as_ref(), row_width) {
        lines.push(Line::from(Span::styled(arg_line, text_style)));
    }

    // Footer with keybinding hints, pinned to the bottom row: the body above
    // is cut first when the terminal is short, so the hints always show.
    let footer_style = Style::default().fg(theme.text_muted);
    let footer = Paragraph::new(Line::from(Span::styled(
        "  y/Enter proceed  a always  n deny  s skip  x stop turn  Esc skip",
        footer_style,
    )));

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    // Long argument lines wrap rather than being cut.
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, chunks[0]);
    frame.render_widget(footer, chunks[1]);
}

#[cfg(test)]
mod tests {
    use crate::app::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use hone_proto::StepAction;

    fn make_key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Space no longer approves (typing a space must not answer the menu).
    #[tokio::test]
    async fn debugger_space_does_not_send_proceed() {
        let mut app = App::new();
        app.debugger_open = true;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Char(' ')));

        assert!(app.debugger_open, "Space must leave the menu open");
        assert!(rx.try_recv().is_err(), "Space must not answer the menu");
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

    /// Esc sends Skip (dismissing an approval prompt never runs the call, and the
    /// agent is not left stuck) and closes the debugger.
    #[tokio::test]
    async fn debugger_esc_sends_skip_and_closes() {
        let mut app = App::new();
        app.debugger_open = true;

        let (tx, mut rx) = tokio::sync::mpsc::channel::<StepAction>(4);
        app.set_step_tx(tx);

        app.handle_key_debugger(make_key(KeyCode::Esc));

        assert!(!app.debugger_open, "debugger should be closed after Esc");
        let action = rx.try_recv().expect("should have sent a StepAction");
        assert_eq!(action, StepAction::Skip);
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
