use ratatui::{Frame, layout::Rect, style::Style};

use crate::{app::App, theme::Theme};

/// Render the history-search overlay above the input area.
///
/// The overlay is shown when `app.history_search_open` is `true`.  It
/// displays up to 8 matching history entries, highlighted when selected, with
/// a one-line search query bar at the bottom (inside the border).
pub fn render_history_search(app: &App, frame: &mut Frame, input_area: Rect) {
    if !app.history_search_open {
        return;
    }

    let theme = app.theme();
    let query = &app.history_search_query;
    let matches = app.history.search(query, 8);
    let match_count = matches.len() as u16;

    // Layout: border top + match rows + query row + border bottom = count + 3
    let popup_height = match_count + 3;
    let popup_width = 60u16.min(input_area.width.saturating_sub(2));

    if popup_width < 10 || input_area.y < popup_height {
        return; // Not enough space.
    }

    let popup_y = input_area.y - popup_height;
    let popup_x = input_area.x + 1;
    let area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    let bg_style = Style::default().bg(theme.surface_overlay).fg(theme.text_primary);
    let border_style = Style::default().bg(theme.surface_overlay).fg(theme.surface_border);
    let muted_style = Style::default().bg(theme.surface_overlay).fg(theme.text_muted);
    let sel_style = Style::default().bg(theme.accent_hot).fg(theme.surface_base);

    let buf = frame.buffer_mut();

    // Fill background.
    for row in 0..popup_height {
        for col in 0..popup_width {
            buf[(area.x + col, area.y + row)]
                .set_char(' ')
                .set_style(bg_style);
        }
    }

    // Rounded border.
    let right = area.x + popup_width - 1;
    let bottom = area.y + popup_height - 1;
    buf[(area.x, area.y)].set_char('╭').set_style(border_style);
    buf[(right, area.y)].set_char('╮').set_style(border_style);
    buf[(area.x, bottom)].set_char('╰').set_style(border_style);
    buf[(right, bottom)].set_char('╯').set_style(border_style);
    for col in 1..popup_width - 1 {
        buf[(area.x + col, area.y)].set_char('─').set_style(border_style);
        buf[(area.x + col, bottom)].set_char('─').set_style(border_style);
    }
    for row in 1..popup_height - 1 {
        buf[(area.x, area.y + row)].set_char('│').set_style(border_style);
        buf[(right, area.y + row)].set_char('│').set_style(border_style);
    }

    // Match rows (most recent first).
    let inner_width = (popup_width.saturating_sub(2)) as usize;
    for (i, entry) in matches.iter().enumerate() {
        let row_y = area.y + 1 + i as u16;
        let is_sel = app.history_search_index == Some(i);
        let row_style = if is_sel { sel_style } else { bg_style };

        // Fill row background.
        for col in 0..inner_width as u16 {
            buf[(area.x + 1 + col, row_y)]
                .set_char(' ')
                .set_style(row_style);
        }

        // Write entry text (truncated to inner width, with 1-char left padding).
        let text = format!(" {entry}");
        let mut col = area.x + 1;
        for ch in text.chars().take(inner_width) {
            buf[(col, row_y)].set_char(ch).set_style(row_style);
            col += 1;
        }
    }

    // Query bar (second-to-last row, just above bottom border).
    let query_y = bottom - 1;
    // Draw a thin separator above the query bar.
    buf[(area.x, query_y - 1)].set_char('├').set_style(border_style);
    buf[(right, query_y - 1)].set_char('┤').set_style(border_style);
    for col in 1..popup_width - 1 {
        buf[(area.x + col, query_y - 1)]
            .set_char('─')
            .set_style(border_style);
    }

    // Fill query row background.
    for col in 0..inner_width as u16 {
        buf[(area.x + 1 + col, query_y)]
            .set_char(' ')
            .set_style(bg_style);
    }

    // "search: " label + query text.
    let label = "search: ";
    let query_display = format!("{label}{query}");
    let mut col = area.x + 1;
    for ch in query_display.chars().take(inner_width) {
        let style = if col < area.x + 1 + label.len() as u16 {
            muted_style
        } else {
            bg_style
        };
        buf[(col, query_y)].set_char(ch).set_style(style);
        col += 1;
    }
}

/// Compute the height of the history-search popup for the given number of
/// matches.  Exposed for layout calculations in tests.
pub fn popup_height_for(match_count: usize) -> u16 {
    match_count as u16 + 3
}

/// Compute the width of the history-search popup clamped to `max_width`.
pub fn popup_width_for(max_width: u16) -> u16 {
    60u16.min(max_width.saturating_sub(2))
}

// Public accessor used by render_history_search — keeps the render function
// independent of Theme internals.
#[allow(dead_code)]
fn _assert_theme_fields(theme: &Theme) {
    let _ = theme.surface_overlay;
    let _ = theme.text_primary;
    let _ = theme.surface_border;
    let _ = theme.text_muted;
    let _ = theme.accent_hot;
    let _ = theme.surface_base;
}
