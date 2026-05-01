//! Agent Rail — narrow right-side sidebar showing per-turn observability data.
//!
//! The rail is toggled with `Ctrl+R` and renders three stacked sections:
//!   1. **Files** — files touched this turn with `[R]`/`[W]`/`[X]` badges
//!   2. **Context** — injected skeleton entries ranked by BM25 score
//!   3. **Memory** — memory recalls: key + truncated snippet
//!
//! Width is 18 columns (Minimal display in Standard+ tier), scrollable with j/k.
//! Falls back to ASCII badges when `theme.no_color` is true.

use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, StatefulWidget, Widget},
};

use crate::{
    theme::Theme,
    turn_ledger::{FileOp, TurnLedger, WorkerStatus},
};

/// Minimum terminal width before the rail is suppressed.
pub const RAIL_MIN_WIDTH: u16 = 100;
/// Default rail column width.
pub const RAIL_WIDTH: u16 = 18;

// ---------------------------------------------------------------------------
// Rail state (kept in App)
// ---------------------------------------------------------------------------

/// Scrollable state for the agent rail.
#[derive(Debug, Default, Clone)]
pub struct AgentRailState {
    /// Vertical scroll offset (in rendered lines).
    pub scroll: usize,
}

// ---------------------------------------------------------------------------
// Widget
// ---------------------------------------------------------------------------

/// Agent Rail widget — renders the per-turn observability sidebar.
pub struct AgentRailWidget<'a> {
    ledger: Option<&'a TurnLedger>,
    theme: &'a Theme,
    is_active_turn: bool,
}

impl<'a> AgentRailWidget<'a> {
    pub fn new(ledger: Option<&'a TurnLedger>, theme: &'a Theme, is_active_turn: bool) -> Self {
        Self {
            ledger,
            theme,
            is_active_turn,
        }
    }

    /// Build the full list of lines to display, then render the visible slice.
    fn build_lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        let theme = self.theme;
        let nc = theme.no_color;

        // Status indicator in the section header when a turn is running.
        let state_label = if self.is_active_turn { " (live)" } else { "" };

        // ----------------------------------------------------------------
        // FILES section
        // ----------------------------------------------------------------
        let files_header = if nc { "[FILES]" } else { "[FILES]" };
        lines.push(header_line(format!("{files_header}{state_label}"), theme));

        if let Some(ledger) = self.ledger {
            if ledger.files_touched.is_empty() {
                lines.push(dim_line("  —", theme));
            } else {
                for ft in &ledger.files_touched {
                    let badge = if nc {
                        ft.op.badge_ascii()
                    } else {
                        ft.op.badge()
                    };
                    // Truncate path from the left so the filename is always visible.
                    let display_path = truncate_path_left(&ft.path, RAIL_WIDTH as usize - 5);
                    let (badge_color, path_color) = match ft.op {
                        FileOp::Read => (theme.role_tool, theme.text_primary),
                        FileOp::Write => (theme.diff_add_fg, theme.text_primary),
                        FileOp::Exec => (theme.syntax_number, theme.text_primary),
                    };
                    let line_range_suffix = ft
                        .line_range
                        .as_ref()
                        .map(|r| format!(":{}", r.start + 1))
                        .unwrap_or_default();

                    let badge_span = if nc {
                        Span::raw(badge.to_string())
                    } else {
                        Span::styled(badge.to_string(), Style::default().fg(badge_color))
                    };

                    let path_text = if line_range_suffix.is_empty() {
                        format!(" {display_path}")
                    } else {
                        format!(" {display_path}{line_range_suffix}")
                    };

                    let path_span = if nc {
                        Span::raw(path_text)
                    } else {
                        Span::styled(path_text, Style::default().fg(path_color))
                    };

                    lines.push(Line::from(vec![badge_span, path_span]));
                }

                // Summary footer.
                let s = ledger.summary();
                let footer = format!(
                    "  {} files  {} ops",
                    s.files_touched,
                    s.read_ops + s.write_ops + s.exec_ops
                );
                lines.push(dim_line(footer, theme));
            }

            // ----------------------------------------------------------------
            // AGENTS section
            // ----------------------------------------------------------------
            lines.push(Line::from(""));
            lines.push(header_line("[AGENTS]".to_string(), theme));

            if ledger.worker_invocations.is_empty() {
                lines.push(dim_line("  —", theme));
            } else {
                for w in &ledger.worker_invocations {
                    let glyph = if nc {
                        match w.status {
                            WorkerStatus::Running => "…",
                            WorkerStatus::Succeeded => "ok",
                            WorkerStatus::Failed => "X",
                            WorkerStatus::TimedOut => "!",
                        }
                    } else {
                        // `⏱` (U+23F1 STOPWATCH) avoided: emoji-presentation
                        // terminals render it as 2 cells, overflowing the rail.
                        match w.status {
                            WorkerStatus::Running => "…",
                            WorkerStatus::Succeeded => "✓",
                            WorkerStatus::Failed => "✗",
                            WorkerStatus::TimedOut => "T",
                        }
                    };
                    // Budget: glyph + space + agent name, left-truncated to RAIL_WIDTH - 2 borders
                    let inner_width = RAIL_WIDTH as usize - 2; // account for block borders
                    let glyph_len = glyph.chars().count();
                    let agent_max = inner_width.saturating_sub(glyph_len + 1);
                    let agent_display = truncate_path_left(&w.agent, agent_max);
                    let line_text = format!("{glyph} {agent_display}");
                    lines.push(Line::from(Span::raw(line_text)));
                }
            }

            // ----------------------------------------------------------------
            // CONTEXT section
            // ----------------------------------------------------------------
            lines.push(Line::from(""));
            lines.push(header_line("[CONTEXT]".to_string(), theme));

            let top_skel = ledger.top_skeleton(5);
            if top_skel.is_empty() {
                lines.push(dim_line("  —", theme));
            } else {
                for entry in top_skel {
                    let strategy_short = abbrev_strategy(&entry.strategy);
                    let score_text = entry
                        .bm25_score
                        .map(|s| format!("{:.2}", s))
                        .unwrap_or_else(|| "—".to_string());
                    let tokens_text = format!("{}t", entry.tokens);

                    // Color token count by threshold.
                    let tok_color = if nc || entry.tokens < 1000 {
                        theme.budget_low
                    } else if entry.tokens < 2000 {
                        theme.budget_mid
                    } else {
                        theme.budget_high
                    };

                    let display_path = truncate_path_left(&entry.path, RAIL_WIDTH as usize - 6);
                    let label = format!(
                        "  {} {} {} {}",
                        strategy_short, display_path, score_text, tokens_text
                    );

                    if nc {
                        lines.push(Line::from(Span::raw(label)));
                    } else {
                        // Highlight the tokens in a budget-aware colour.
                        let plain_len = label.len().saturating_sub(tokens_text.len());
                        let plain_part = &label[..plain_len];
                        let tokens_part = label[plain_len..].to_string();
                        lines.push(Line::from(vec![
                            Span::styled(
                                plain_part.to_string(),
                                Style::default().fg(theme.text_secondary),
                            ),
                            Span::styled(tokens_part, Style::default().fg(tok_color)),
                        ]));
                    }
                }
            }

            // ----------------------------------------------------------------
            // MEMORY section
            // ----------------------------------------------------------------
            lines.push(Line::from(""));
            lines.push(header_line("[MEMORY]".to_string(), theme));

            if ledger.memory_recalls.is_empty() {
                lines.push(dim_line("  —", theme));
            } else {
                for recall in &ledger.memory_recalls {
                    let key_display = truncate_path_left(&recall.key, RAIL_WIDTH as usize - 2);
                    if nc {
                        lines.push(Line::from(format!("  {key_display}")));
                        lines.push(dim_line(
                            format!(
                                "    {}",
                                truncate_str(&recall.snippet, RAIL_WIDTH as usize - 4)
                            ),
                            theme,
                        ));
                    } else {
                        lines.push(Line::from(vec![Span::styled(
                            format!("  {key_display}"),
                            Style::default()
                                .fg(theme.accent_mid)
                                .add_modifier(Modifier::BOLD),
                        )]));
                        lines.push(dim_line(
                            format!(
                                "    {}",
                                truncate_str(&recall.snippet, RAIL_WIDTH as usize - 4)
                            ),
                            theme,
                        ));
                    }
                }
            }
        } else {
            lines.push(dim_line("  Waiting…", theme));
            lines.push(Line::from(""));
            lines.push(header_line("[AGENTS]".to_string(), theme));
            lines.push(dim_line("  —", theme));
            lines.push(Line::from(""));
            lines.push(header_line("[CONTEXT]".to_string(), theme));
            lines.push(dim_line("  —", theme));
            lines.push(Line::from(""));
            lines.push(header_line("[MEMORY]".to_string(), theme));
            lines.push(dim_line("  —", theme));
        }

        lines
    }
}

impl<'a> StatefulWidget for AgentRailWidget<'a> {
    type State = AgentRailState;

    fn render(self, area: Rect, buf: &mut Buffer, state: &mut AgentRailState) {
        let theme = self.theme;
        let border_style = Style::default().fg(theme.surface_border);
        let title_style = Style::default().fg(theme.text_muted);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(if theme.no_color {
                BorderType::Plain
            } else {
                BorderType::Rounded
            })
            .border_style(border_style)
            .title(Span::styled(" rail ", title_style));

        let inner = block.inner(area);
        block.render(area, buf);

        let all_lines = self.build_lines();
        let total = all_lines.len();
        let visible_height = inner.height as usize;

        // Clamp scroll.
        state.scroll = state.scroll.min(total.saturating_sub(visible_height));

        let visible: Vec<Line<'static>> = all_lines
            .into_iter()
            .skip(state.scroll)
            .take(visible_height)
            .collect();

        // We wrap each line in a Paragraph with a fixed width.
        for (i, line) in visible.into_iter().enumerate() {
            let row_area = Rect {
                x: inner.x,
                y: inner.y + i as u16,
                width: inner.width,
                height: 1,
            };
            Paragraph::new(line).render(row_area, buf);
        }
    }
}

// ---------------------------------------------------------------------------
// Layout helper — computes chunk rects when the rail is visible
// ---------------------------------------------------------------------------

/// Split the content area into [conversation, rail] when the rail is enabled
/// and the terminal is wide enough.  Returns `None` if the terminal is too
/// narrow for the rail (< `RAIL_MIN_WIDTH`).
pub fn split_with_rail(area: Rect) -> Option<[Rect; 2]> {
    if area.width < RAIL_MIN_WIDTH {
        return None;
    }
    let rail_width = RAIL_WIDTH.min(area.width / 4); // never more than 25% of width
    let conversation_width = area.width.saturating_sub(rail_width);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(conversation_width),
            Constraint::Length(rail_width),
        ])
        .split(area);
    Some([chunks[0], chunks[1]])
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn header_line(text: impl Into<String>, theme: &Theme) -> Line<'static> {
    let text = text.into();
    if theme.no_color {
        Line::from(Span::raw(text))
    } else {
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(theme.accent_hot)
                .add_modifier(Modifier::BOLD),
        ))
    }
}

fn dim_line(text: impl Into<String>, theme: &Theme) -> Line<'static> {
    let text = text.into();
    if theme.no_color {
        Line::from(Span::raw(text))
    } else {
        Line::from(Span::styled(text, Style::default().fg(theme.text_muted)))
    }
}

/// Abbreviate a strategy name to ≤4 chars.
fn abbrev_strategy(s: &str) -> &str {
    match s.to_ascii_lowercase().as_str() {
        "callgraph" | "call_graph" | "call-graph" => "call",
        "flat" => "flat",
        "adaptive" => "adpt",
        "hybrid" => "hybr",
        "bm25" => "bm25",
        _ => &s[..s.len().min(4)],
    }
}

/// Truncate a string to `max_chars`, adding `…` if truncated.
fn truncate_str(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = chars[..max_chars.saturating_sub(1)].iter().collect();
        format!("{truncated}…")
    }
}

/// Truncate a path from the left so the filename is always visible.
///
/// `…ong/path/auth.rs` instead of `long/path/auth.r…`
pub fn truncate_path_left(path: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    // Use just the file name component if the full path doesn't fit.
    let chars: Vec<char> = path.chars().collect();
    if chars.len() <= max_chars {
        return path.to_string();
    }
    // Try filename only.
    if let Some(name) = std::path::Path::new(path).file_name() {
        let name_str = name.to_string_lossy();
        let name_chars: Vec<char> = name_str.chars().collect();
        if name_chars.len() <= max_chars {
            return name_str.into_owned();
        }
        // Even filename is too long — truncate from the right.
        let truncated: String = name_chars[..max_chars.saturating_sub(1)].iter().collect();
        return format!("{truncated}…");
    }
    // Fallback: truncate whole path from the left.
    let start = chars.len() - max_chars + 1;
    let rest: String = chars[start..].iter().collect();
    format!("…{rest}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn_ledger::{FileOp, TurnLedger};
    use ratatui::{Terminal, backend::TestBackend};

    fn make_theme() -> Theme {
        Theme::crimson()
    }

    fn make_no_color_theme() -> Theme {
        Theme::minimal()
    }

    #[test]
    fn truncate_path_left_short_path() {
        assert_eq!(truncate_path_left("auth.rs", 20), "auth.rs");
    }

    #[test]
    fn truncate_path_left_long_path_shows_filename() {
        let path = "some/very/long/nested/path/to/auth.rs";
        let result = truncate_path_left(path, 10);
        assert!(
            result.contains("auth.rs"),
            "filename should be visible: {result}"
        );
        assert!(result.len() <= 10 || result == "auth.rs");
    }

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_long() {
        let result = truncate_str("hello world", 6);
        assert_eq!(result.chars().count(), 6);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn split_with_rail_too_narrow() {
        let narrow = Rect::new(0, 0, 80, 24);
        assert!(split_with_rail(narrow).is_none());
    }

    #[test]
    fn split_with_rail_wide_enough() {
        let wide = Rect::new(0, 0, 120, 24);
        let chunks = split_with_rail(wide);
        assert!(chunks.is_some());
        let [conv, rail] = chunks.unwrap();
        assert_eq!(conv.width + rail.width, 120);
        assert_eq!(rail.width, RAIL_WIDTH);
    }

    #[test]
    fn rail_renders_without_panic_empty_ledger() {
        let theme = make_theme();
        let widget = AgentRailWidget::new(None, &theme, false);
        let mut state = AgentRailState::default();
        let backend = TestBackend::new(RAIL_WIDTH, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                f.render_stateful_widget(widget, f.area(), &mut state);
            })
            .unwrap();
    }

    #[test]
    fn rail_renders_with_ledger_data() {
        let mut ledger = TurnLedger::new(0);
        ledger.record_file_touch("src/auth.rs", FileOp::Read, None);
        ledger.record_file_touch("src/session.rs", FileOp::Write, Some(45..60));
        ledger.record_skeleton_entry("src/auth.rs", "CallGraph", Some(0.91), 120);
        ledger.record_memory_recall("auth-pattern", "Use new_gen() for tokens");

        let theme = make_theme();
        let widget = AgentRailWidget::new(Some(&ledger), &theme, true);
        let mut state = AgentRailState::default();
        let backend = TestBackend::new(RAIL_WIDTH, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                f.render_stateful_widget(widget, f.area(), &mut state);
            })
            .unwrap();
        // Just check it didn't panic and rendered something.
        let buf = terminal.backend().buffer().clone();
        // The rail block border should be on the left edge.
        assert!(buf.area().width > 0);
    }

    #[test]
    fn rail_renders_no_color_mode() {
        let mut ledger = TurnLedger::new(0);
        ledger.record_file_touch("src/auth.rs", FileOp::Read, None);
        ledger.record_file_touch("src/main.rs", FileOp::Write, None);
        ledger.record_file_touch("run.sh", FileOp::Exec, None);

        let theme = make_no_color_theme();
        let widget = AgentRailWidget::new(Some(&ledger), &theme, false);
        let mut state = AgentRailState::default();
        let backend = TestBackend::new(RAIL_WIDTH, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                f.render_stateful_widget(widget, f.area(), &mut state);
            })
            .unwrap();
    }

    // -----------------------------------------------------------------------
    // [AGENTS] section render tests (RED — the section does not exist yet;
    // tests will fail to compile / fail assertions until the implementation
    // lands).
    // -----------------------------------------------------------------------

    /// Helper: render the rail with the given ledger into a no-color
    /// TestBackend and return all rendered rows as a Vec<String>.
    fn render_rail_no_color(ledger: &TurnLedger, height: u16) -> Vec<String> {
        let theme = make_no_color_theme();
        let widget = AgentRailWidget::new(Some(ledger), &theme, false);
        let mut state = AgentRailState::default();
        let backend = TestBackend::new(RAIL_WIDTH, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|f| {
                f.render_stateful_widget(widget, f.area(), &mut state);
            })
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..height)
            .map(|row| {
                (0..RAIL_WIDTH)
                    .map(|col| {
                        buf.cell((col, row))
                            .map(|c| c.symbol().chars().next().unwrap_or(' '))
                            .unwrap_or(' ')
                    })
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn agents_section_header_appears_when_rail_visible() {
        use crate::turn_ledger::WorkerStatus;

        let mut ledger = TurnLedger::new(0);
        // One running worker is enough to trigger the section.
        ledger.record_worker_started("t1", "rust-expert", "implement feature");

        let rows = render_rail_no_color(&ledger, 30);

        // Find the row indices for each section header.
        let files_row = rows
            .iter()
            .position(|r| r.contains("[FILES]"))
            .expect("[FILES] header not found");
        let agents_row = rows
            .iter()
            .position(|r| r.contains("[AGENTS]"))
            .expect("[AGENTS] header not found — section not rendered");
        let context_row = rows
            .iter()
            .position(|r| r.contains("[CONTEXT]"))
            .expect("[CONTEXT] header not found");

        // Order constraint: FILES < AGENTS < CONTEXT.
        assert!(
            files_row < agents_row,
            "[AGENTS] must appear after [FILES]: files={files_row} agents={agents_row}"
        );
        assert!(
            agents_row < context_row,
            "[AGENTS] must appear before [CONTEXT]: agents={agents_row} context={context_row}"
        );
    }

    #[test]
    fn agents_section_renders_running_succeeded_failed_and_timed_out() {
        use crate::turn_ledger::WorkerStatus;

        let mut ledger = TurnLedger::new(0);
        // Running
        ledger.record_worker_started("t1", "rust-expert", "task a");
        // Succeeded
        ledger.record_worker_started("t2", "python-expert", "task b");
        ledger.record_worker_completed("t2", "completed", 0.01);
        // Failed
        ledger.record_worker_started("t3", "debugger", "task c");
        ledger.record_worker_completed("t3", "failed: crash", 0.0);
        // TimedOut
        ledger.record_worker_started("t4", "ai-researcher", "task d");
        ledger.record_worker_completed("t4", "timed_out", 0.0);

        let rows = render_rail_no_color(&ledger, 40);
        let combined = rows.join("\n");

        // Agent names must appear (possibly truncated by truncate_path_left).
        // "rust-expert" is 11 chars; rail inner width = RAIL_WIDTH - 2 borders = 16.
        // The full name fits, so it must appear verbatim.
        assert!(
            combined.contains("rust-expert"),
            "rust-expert not found in:\n{combined}"
        );
        assert!(
            combined.contains("python-expert"),
            "python-expert not found in:\n{combined}"
        );
        assert!(
            combined.contains("debugger"),
            "debugger not found in:\n{combined}"
        );
        assert!(
            combined.contains("ai-researcher"),
            "ai-researcher not found in:\n{combined}"
        );

        // ASCII status glyphs pinned here — the implementer must use these exact
        // strings in no-color mode:
        //   Running   → "…"
        //   Succeeded → "ok"
        //   Failed    → "X"
        //   TimedOut  → "!"
        assert!(
            combined.contains("ok"),
            "Succeeded glyph 'ok' not found in:\n{combined}"
        );
        assert!(
            combined.contains('X'),
            "Failed glyph 'X' not found in:\n{combined}"
        );
        assert!(
            combined.contains('!'),
            "TimedOut glyph '!' not found in:\n{combined}"
        );
        // Running glyph '…' — the ellipsis char U+2026.
        assert!(
            combined.contains('…'),
            "Running glyph '…' not found in:\n{combined}"
        );
    }

    #[test]
    fn agents_section_shows_em_dash_when_empty() {
        // When no workers have been recorded, the [AGENTS] section must show
        // the standard placeholder "  —" (matching the convention used by
        // FILES / CONTEXT / MEMORY for empty sections).
        let ledger = TurnLedger::new(0); // no workers

        let rows = render_rail_no_color(&ledger, 30);
        let combined = rows.join("\n");

        // Section must still render.
        assert!(
            combined.contains("[AGENTS]"),
            "[AGENTS] header missing for empty ledger:\n{combined}"
        );

        // Find the [AGENTS] header row then look for the em-dash on a nearby row.
        let agents_row = rows
            .iter()
            .position(|r| r.contains("[AGENTS]"))
            .unwrap();
        // The placeholder should be within the next 2 rows after the header.
        let nearby = rows[agents_row..].iter().take(3).any(|r| r.contains('—'));
        assert!(
            nearby,
            "em-dash placeholder '—' not found near [AGENTS] header:\n{combined}"
        );
    }

    #[test]
    fn layout_chunking_with_rail_visible() {
        // Simulate the root.rs layout split when rail is visible.
        let area = Rect::new(0, 0, 120, 24);
        let maybe_chunks = split_with_rail(area);
        assert!(maybe_chunks.is_some());
        let [conv_area, rail_area] = maybe_chunks.unwrap();
        // Conversation gets most of the width.
        assert!(conv_area.width > rail_area.width);
        // Rail is exactly RAIL_WIDTH.
        assert_eq!(rail_area.width, RAIL_WIDTH);
        // Together they fill the full area width.
        assert_eq!(conv_area.width + rail_area.width, area.width);
    }
}
