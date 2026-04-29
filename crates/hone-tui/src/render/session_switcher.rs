use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};

use crate::{app::App, session_switcher::format_relative_time};

/// Render the full-screen session-switcher overlay.
///
/// Draws a rounded-border popup over `area`, listing all available sessions
/// with j/k navigation and Enter to switch.
pub fn render_session_switcher(app: &App, frame: &mut Frame, area: Rect) {
    let theme = app.theme();

    // ---- Outer block (border + title) ----------------------------------------
    let border_style = Style::default().fg(theme.accent_mid);
    let title_style = Style::default()
        .fg(theme.accent_hot)
        .add_modifier(Modifier::BOLD);
    let bg_style = Style::default().bg(theme.surface_overlay);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .style(bg_style)
        .title(Span::styled(" Sessions (Ctrl+O) ", title_style));

    // Inner area inside the border.
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // ---- Column widths -------------------------------------------------------
    // The inner area is split into three logical columns:
    //   NAME (left-aligned, fills remaining space)  |  UPDATED (22 cols)  |  MSGS (6 cols)
    // We always reserve space for the fixed-width columns.
    let fixed_right_width: usize = 22 + 1 + 6; // "UPDATED" col + separator + "MSGS" col

    // Minimum inner width to render meaningfully.
    if inner.width < (fixed_right_width + 8) as u16 {
        return;
    }
    let name_width = (inner.width as usize).saturating_sub(fixed_right_width + 2); // +2 leading spaces

    // ---- Header --------------------------------------------------------------
    let header_style = Style::default()
        .fg(theme.text_muted)
        .add_modifier(Modifier::BOLD);

    let header_text = format!(
        "  {:<name_w$}  {:<22}  {:>6}",
        "NAME",
        "UPDATED",
        "MSGS",
        name_w = name_width,
    );
    let header_line = Line::from(Span::styled(header_text, header_style));

    // Separator line.
    let sep: String = "─".repeat(inner.width as usize);
    let sep_line = Line::from(Span::styled(sep, Style::default().fg(theme.surface_border)));

    // ---- Session rows --------------------------------------------------------
    let sessions = &app.sessions_list;
    let cursor = app.sessions_cursor;

    let mut lines: Vec<Line<'_>> = Vec::new();
    lines.push(header_line);
    lines.push(sep_line);

    if sessions.is_empty() {
        let empty_msg = "  No saved sessions. Use --session <name> to start one.";
        let empty_line = Line::from(Span::styled(
            empty_msg,
            Style::default().fg(theme.text_muted),
        ));
        // Vertically centre the message by padding with blank lines.
        let available_rows = inner.height.saturating_sub(2) as usize; // subtract header+sep
        let pad_top = available_rows / 2;
        for _ in 0..pad_top {
            lines.push(Line::from(""));
        }
        lines.push(empty_line);
    } else {
        // How many rows can we display?
        let available_rows = inner.height.saturating_sub(3) as usize; // header + sep + footer

        // Scroll to keep cursor visible.
        let offset = scroll_offset(cursor, available_rows);

        for (list_idx, session) in sessions
            .iter()
            .enumerate()
            .skip(offset)
            .take(available_rows)
        {
            let is_selected = list_idx == cursor;

            let relative_time = format_relative_time(session.updated_at);

            // Truncate session name if necessary.
            let name: String = if session.name.len() > name_width {
                format!("{}…", &session.name[..name_width.saturating_sub(1)])
            } else {
                session.name.clone()
            };

            let row_text = format!(
                "  {:<name_w$}  {:<22}  {:>6}",
                name,
                relative_time,
                session.message_count,
                name_w = name_width,
            );

            let row_style = if is_selected {
                Style::default()
                    .bg(theme.accent_hot)
                    .fg(theme.surface_base)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text_primary)
            };

            lines.push(Line::from(Span::styled(row_text, row_style)));
        }
    }

    // ---- Footer --------------------------------------------------------------
    let footer_style = Style::default().fg(theme.text_muted);
    let footer_text = "  j/k navigate  Enter switch  Esc cancel";
    let footer_line = Line::from(Span::styled(footer_text, footer_style));
    lines.push(Line::from("")); // blank spacer
    lines.push(footer_line);

    // ---- Render --------------------------------------------------------------
    let para = Paragraph::new(lines);
    frame.render_widget(para, inner);
}

/// Compute a scroll offset so `cursor` stays within [offset, offset + visible).
fn scroll_offset(cursor: usize, visible: usize) -> usize {
    if visible == 0 {
        return 0;
    }
    // Keep cursor near the centre of the visible window.
    cursor.saturating_sub(visible.saturating_sub(1) / 2)
}
