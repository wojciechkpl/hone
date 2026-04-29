//! Session Anatomy overlay — full-screen Ctrl+A panel.
//!
//! Two tabs:
//!   - **Tab A: Phase Timeline** — one row per completed turn: phase, tool
//!     count, token/cost placeholders, outcome tick/cross.
//!   - **Tab B: Memory Graph** — ASCII visualisation of memory recalls across
//!     turns.
//!
//! Data is sourced exclusively from `App::completed_turns: Vec<TurnLedger>`,
//! which is populated at the end of every turn.

use ratatui::{
    Frame,
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, BorderType, Borders, Clear, Widget},
};

use crate::{app::App, theme::Theme, turn_ledger::TurnLedger};

// ---------------------------------------------------------------------------
// Public entry-point
// ---------------------------------------------------------------------------

/// Render the Session Anatomy overlay, covering the full `area`.
///
/// Must be called **after** all other widgets so it paints on top.
pub fn render_session_anatomy(app: &App, frame: &mut Frame, area: Rect) {
    let overlay = SessionAnatomyOverlay {
        app,
        theme: app.theme(),
    };
    frame.render_widget(overlay, area);
}

// ---------------------------------------------------------------------------
// Widget struct
// ---------------------------------------------------------------------------

struct SessionAnatomyOverlay<'a> {
    app: &'a App,
    theme: &'a Theme,
}

impl<'a> Widget for SessionAnatomyOverlay<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // Clear the full area so the overlay is opaque.
        Clear.render(area, buf);

        let theme = self.theme;
        let border_style = Style::default().fg(theme.surface_focus);
        let bg_style = Style::default()
            .bg(theme.surface_overlay)
            .fg(theme.text_primary);

        // Fill background.
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_char(' ').set_style(bg_style);
            }
        }

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(Span::styled(
                " Session Anatomy ",
                Style::default()
                    .fg(theme.accent_hot)
                    .add_modifier(Modifier::BOLD),
            ));

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height < 4 || inner.width < 20 {
            return; // too small to render content
        }

        // ------------------------------------------------------------------
        // Tab bar  (row 0 of inner)
        // ------------------------------------------------------------------
        let tab_y = inner.top();
        render_tab_bar(self.app, theme, buf, inner, tab_y);

        // Divider (row 1 of inner)
        let divider_y = tab_y + 1;
        if divider_y < inner.bottom() {
            for x in inner.left()..inner.right() {
                buf[(x, divider_y)]
                    .set_char('─')
                    .set_style(Style::default().fg(theme.surface_border));
            }
        }

        // Footer (last row of inner)
        let footer_y = inner.bottom().saturating_sub(1);
        if footer_y > divider_y + 1 {
            render_footer(theme, buf, inner, footer_y);
        }

        // Content area between divider+1 and footer-1.
        let content_top = divider_y + 1;
        let content_bottom = footer_y;
        if content_top >= content_bottom {
            return;
        }
        let content_area = Rect::new(
            inner.left(),
            content_top,
            inner.width,
            content_bottom - content_top,
        );

        match self.app.anatomy_tab {
            0 => render_phase_timeline(self.app, theme, buf, content_area),
            _ => render_memory_graph(self.app, theme, buf, content_area),
        }
    }
}

// ---------------------------------------------------------------------------
// Tab bar
// ---------------------------------------------------------------------------

fn render_tab_bar(app: &App, theme: &Theme, buf: &mut Buffer, inner: Rect, y: u16) {
    let active_style = Style::default()
        .fg(theme.accent_hot)
        .add_modifier(Modifier::BOLD);
    let inactive_style = Style::default().fg(theme.text_muted);

    let tabs = [(0usize, "[A Phase Timeline]"), (1usize, "[B Memory Graph]")];

    let mut x = inner.left() + 1;
    for (idx, label) in &tabs {
        let style = if *idx == app.anatomy_tab {
            active_style
        } else {
            inactive_style
        };
        for ch in label.chars() {
            if x >= inner.right() {
                break;
            }
            buf[(x, y)].set_char(ch).set_style(style);
            x += 1;
        }
        // Two spaces between tabs.
        if x + 2 < inner.right() {
            buf[(x, y)].set_char(' ').set_style(inactive_style);
            x += 1;
            buf[(x, y)].set_char(' ').set_style(inactive_style);
            x += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Footer
// ---------------------------------------------------------------------------

fn render_footer(theme: &Theme, buf: &mut Buffer, inner: Rect, y: u16) {
    let footer_style = Style::default().fg(theme.text_muted);
    let text = "  Tab switch  j/k scroll  Esc close";
    let mut x = inner.left();
    for ch in text.chars() {
        if x >= inner.right() {
            break;
        }
        buf[(x, y)].set_char(ch).set_style(footer_style);
        x += 1;
    }
}

// ---------------------------------------------------------------------------
// Tab A — Phase Timeline
// ---------------------------------------------------------------------------

fn render_phase_timeline(app: &App, theme: &Theme, buf: &mut Buffer, area: Rect) {
    if area.height == 0 {
        return;
    }

    let header_style = Style::default()
        .fg(theme.text_secondary)
        .add_modifier(Modifier::BOLD);
    let header = "  Turn  Phase          Tools  Tokens   Cost      Elapsed  Outcome";

    // Header row (first row of content area, always visible).
    let mut x = area.left();
    for ch in header.chars() {
        if x >= area.right() {
            break;
        }
        buf[(x, area.top())].set_char(ch).set_style(header_style);
        x += 1;
    }

    if area.height < 2 {
        return;
    }

    let data_area = Rect::new(area.left(), area.top() + 1, area.width, area.height - 1);
    let turns = app.completed_turns();

    // Compute total data rows so we can clamp anatomy_scroll.
    let total_rows = turns.len();
    let visible = data_area.height as usize;

    // Clamp scroll so we never go past the last page.
    let max_scroll = total_rows.saturating_sub(visible);
    let scroll = app.anatomy_scroll.min(max_scroll);

    for (row_idx, ledger) in turns.iter().skip(scroll).take(visible).enumerate() {
        let y = data_area.top() + row_idx as u16;
        if y >= data_area.bottom() {
            break;
        }
        render_timeline_row(ledger, theme, buf, area.left(), y, area.right(), row_idx);
    }
}

fn render_timeline_row(
    ledger: &TurnLedger,
    theme: &Theme,
    buf: &mut Buffer,
    x_start: u16,
    y: u16,
    x_end: u16,
    row_idx: usize,
) {
    let has_error = ledger.tools_invoked.iter().any(|t| t.error.is_some());

    // Alternate row dimming.
    let base_style = if row_idx % 2 == 0 {
        Style::default().fg(theme.text_primary)
    } else {
        Style::default().fg(theme.text_secondary)
    };
    let outcome_style = if has_error {
        Style::default().fg(theme.role_error)
    } else {
        Style::default().fg(theme.budget_low)
    };

    // Phase label: last entry in phase_transitions or "—".
    let phase_label = ledger
        .phase_transitions
        .last()
        .map(|(p, _)| p.label().to_string())
        .unwrap_or_else(|| "—".to_string());

    let tool_count = ledger.tools_invoked.len();
    let outcome = if has_error { "✗" } else { "✓" };

    // Build the row string with fixed column widths that match the header.
    // Header: "  Turn  Phase          Tools  Tokens   Cost      Elapsed  Outcome"
    //          0123456789...
    let row_text = format!(
        "  {:>4}  {:<12}   {:>3}     —        —           —    {}",
        ledger.turn_index + 1,
        truncate_str(&phase_label, 12),
        tool_count,
        outcome,
    );

    let mut x = x_start;
    let outcome_start = row_text.rfind(outcome).unwrap_or(row_text.len());

    for (byte_idx, ch) in row_text.char_indices() {
        if x >= x_end {
            break;
        }
        // Use error/success color for the outcome character.
        let style = if byte_idx >= outcome_start {
            outcome_style
        } else {
            base_style
        };
        buf[(x, y)].set_char(ch).set_style(style);
        x += 1;
    }
}

// ---------------------------------------------------------------------------
// Tab B — Memory Graph
// ---------------------------------------------------------------------------

fn render_memory_graph(app: &App, theme: &Theme, buf: &mut Buffer, area: Rect) {
    if area.height == 0 {
        return;
    }

    let turns = app.completed_turns();
    let total_rows = turns.len();
    let visible = area.height as usize;

    let max_scroll = total_rows.saturating_sub(visible);
    let scroll = app.anatomy_scroll.min(max_scroll);

    let nc = theme.no_color;
    let recall_glyph = if nc { "[M]" } else { "◆" };

    let no_recall_style = Style::default().fg(theme.text_muted);
    let label_style = Style::default().fg(theme.text_secondary);
    let recall_style = Style::default().fg(theme.accent_mid);
    let snippet_style = Style::default().fg(theme.text_primary);

    for (row_idx, ledger) in turns.iter().skip(scroll).take(visible).enumerate() {
        let y = area.top() + row_idx as u16;
        if y >= area.bottom() {
            break;
        }

        let turn_label = format!("T{:<3} ", ledger.turn_index + 1);
        let mut x = area.left();

        // Turn label.
        for ch in turn_label.chars() {
            if x >= area.right() {
                break;
            }
            buf[(x, y)].set_char(ch).set_style(label_style);
            x += 1;
        }

        if ledger.memory_recalls.is_empty() {
            // Dash placeholder.
            let placeholder = "·  (no recalls)";
            for ch in placeholder.chars() {
                if x >= area.right() {
                    break;
                }
                buf[(x, y)].set_char(ch).set_style(no_recall_style);
                x += 1;
            }
        } else {
            for (recall_idx, recall) in ledger.memory_recalls.iter().enumerate() {
                if recall_idx > 0 {
                    // Gap between recalls.
                    if x + 3 >= area.right() {
                        break;
                    }
                    buf[(x, y)].set_char(' ').set_style(no_recall_style);
                    x += 1;
                    buf[(x, y)].set_char(' ').set_style(no_recall_style);
                    x += 1;
                }

                // Glyph.
                for ch in recall_glyph.chars() {
                    if x >= area.right() {
                        break;
                    }
                    buf[(x, y)].set_char(ch).set_style(recall_style);
                    x += 1;
                }
                if x >= area.right() {
                    break;
                }
                buf[(x, y)].set_char(' ').set_style(no_recall_style);
                x += 1;

                // Snippet, truncated to 40 chars.
                let snippet = truncate_str(&recall.snippet, 40);
                for ch in snippet.chars() {
                    if x >= area.right() {
                        break;
                    }
                    buf[(x, y)].set_char(ch).set_style(snippet_style);
                    x += 1;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `max` chars, appending `…` when truncated.
fn truncate_str(s: &str, max: usize) -> String {
    let mut count = 0;
    for (byte_idx, _) in s.char_indices() {
        if count == max {
            return format!("{}…", &s[..byte_idx]);
        }
        count += 1;
    }
    s.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn_ledger::{Phase, TurnLedger};

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn make_app_with_turns(turns: Vec<TurnLedger>) -> App {
        let mut app = App::new();
        app.show_splash = false;
        app.anatomy_open = true;
        app.completed_turns = turns;
        app
    }

    fn render_to_string(app: &App) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let backend = TestBackend::new(120, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| render_session_anatomy(app, f, f.area()))
            .unwrap();
        // Collect all cell chars into a flat string for assertions.
        let buf = terminal.backend().buffer().clone();
        let mut result = String::new();
        for y in 0..30 {
            for x in 0..120 {
                let cell = buf
                    .cell((x, y))
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .unwrap_or(' ');
                result.push(cell);
            }
            result.push('\n');
        }
        result
    }

    // -----------------------------------------------------------------------
    // anatomy_empty_turns_renders_header_row
    // -----------------------------------------------------------------------

    #[test]
    fn anatomy_empty_turns_renders_header_row() {
        let app = make_app_with_turns(vec![]);
        let output = render_to_string(&app);
        assert!(
            output.contains("Turn"),
            "header row must contain 'Turn'; got:\n{output}"
        );
        assert!(
            output.contains("Phase"),
            "header row must contain 'Phase'; got:\n{output}"
        );
        assert!(
            output.contains("Tools"),
            "header row must contain 'Tools'; got:\n{output}"
        );
    }

    // -----------------------------------------------------------------------
    // anatomy_outcome_tick_when_no_errors
    // -----------------------------------------------------------------------

    #[test]
    fn anatomy_outcome_tick_when_no_errors() {
        let mut ledger = TurnLedger::new(0);
        ledger.record_tool_call("read_file", 100, false, None);
        ledger.record_phase_transition(Phase::Exploration);

        let app = make_app_with_turns(vec![ledger]);
        let output = render_to_string(&app);

        assert!(
            output.contains('✓'),
            "expected tick for error-free turn; got:\n{output}"
        );
        assert!(
            !output.contains('✗'),
            "unexpected cross in error-free turn; got:\n{output}"
        );
    }

    // -----------------------------------------------------------------------
    // anatomy_outcome_cross_when_tool_error
    // -----------------------------------------------------------------------

    #[test]
    fn anatomy_outcome_cross_when_tool_error() {
        let mut ledger = TurnLedger::new(0);
        ledger.record_tool_call("shell", 50, false, Some("exit code 1".to_string()));
        ledger.record_phase_transition(Phase::Verification);

        let app = make_app_with_turns(vec![ledger]);
        let output = render_to_string(&app);

        assert!(
            output.contains('✗'),
            "expected cross for turn with tool error; got:\n{output}"
        );
    }

    // -----------------------------------------------------------------------
    // anatomy_memory_graph_shows_recall_snippet
    // -----------------------------------------------------------------------

    #[test]
    fn anatomy_memory_graph_shows_recall_snippet() {
        let mut ledger = TurnLedger::new(0);
        ledger.record_memory_recall("key1", "fix the foo bug");

        let mut app = make_app_with_turns(vec![ledger]);
        app.anatomy_tab = 1; // Memory Graph tab

        let output = render_to_string(&app);

        assert!(
            output.contains("fix the foo bug"),
            "memory graph must render recall snippet; got:\n{output}"
        );
    }

    // -----------------------------------------------------------------------
    // anatomy_tab_cycle_wraps
    // -----------------------------------------------------------------------

    #[test]
    fn anatomy_tab_cycle_wraps() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

        let mut app = App::new();
        app.show_splash = false;
        app.anatomy_open = true;
        assert_eq!(app.anatomy_tab, 0);

        // Tab → 1.
        app.handle_key_anatomy(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.anatomy_tab, 1);

        // Tab → wraps back to 0.
        app.handle_key_anatomy(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(app.anatomy_tab, 0);
    }
}
