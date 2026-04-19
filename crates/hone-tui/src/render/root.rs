use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use crate::{
    app::App,
    commands::Command,
    layout::LayoutMode,
    render::{
        agent_rail::{AgentRailState, AgentRailWidget, split_with_rail},
        conversation::ConversationWidget,
        file_preview::FilePreviewWidget,
        header::render_header,
        help::HelpOverlay,
        input_widget::InputWidget,
        splash::SplashWidget,
        status_bar::render_status_bar,
    },
};

/// Render the entire TUI into the given frame.
pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();

    if area.width < 80 || area.height < 24 {
        render_too_small(frame, area);
        return;
    }

    if app.show_splash {
        render_splash(app, frame, area);
        return;
    }

    // Main layout: header (1) + content (fill) + input (3) + status (1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(5),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(area);

    render_header_bar(app, frame, chunks[0]);
    render_content_area(app, frame, chunks[1]);
    render_input_area(app, frame, chunks[2]);
    render_status_bar_area(app, frame, chunks[3]);

    // Autocomplete dropdown above the input area
    render_autocomplete(app, frame, chunks[2]);

    if app.show_help() {
        render_help_overlay(app, frame, area);
    }
}

fn render_too_small(frame: &mut Frame, area: Rect) {
    let msg = "Terminal too small (min 80x24)";
    let style = Style::default();
    let line = Line::from(Span::styled(msg, style));
    let para = Paragraph::new(line);
    frame.render_widget(para, area);
}

fn render_splash(app: &App, frame: &mut Frame, area: Rect) {
    let widget = SplashWidget::new(app.theme(), app.animation());
    frame.render_widget(widget, area);
}

fn render_header_bar(app: &App, frame: &mut Frame, area: Rect) {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "~".to_string());

    let line = render_header(app.theme(), &cwd, None);
    let para = Paragraph::new(line)
        .style(Style::default().bg(app.theme().surface_raised));
    frame.render_widget(para, area);
}

fn render_content_area(app: &App, frame: &mut Frame, area: Rect) {
    // When the agent rail is visible and the terminal is wide enough, split off
    // the rail column from the right side of the content area first, then apply
    // the normal layout logic to the remaining area.
    let (main_area, rail_area) = if app.agent_rail_visible() {
        match split_with_rail(area) {
            Some([conv_area, rail_area]) => (conv_area, Some(rail_area)),
            None => (area, None),
        }
    } else {
        (area, None)
    };

    match app.layout() {
        LayoutMode::SinglePane => {
            render_conversation(app, frame, main_area);
        }
        LayoutMode::SplitHorizontal { ratio } => {
            let ratio = *ratio;
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(ratio),
                    Constraint::Percentage(100 - ratio),
                ])
                .split(main_area);

            render_conversation(app, frame, chunks[0]);
            render_file_preview(app, frame, chunks[1]);
        }
        LayoutMode::BottomPanel => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(main_area);

            render_conversation(app, frame, chunks[0]);
            render_tool_panel(app, frame, chunks[1]);
        }
    }

    // Render the agent rail if it is visible and space was allocated.
    if let Some(rail_rect) = rail_area {
        render_agent_rail(app, frame, rail_rect);
    }
}

fn render_agent_rail(app: &App, frame: &mut Frame, area: Rect) {
    let is_active = app.state() == &crate::app::AppState::WaitingForResponse;
    let ledger = app.active_ledger();
    let widget = AgentRailWidget::new(ledger, app.theme(), is_active);
    let mut state = AgentRailState {
        scroll: app.rail_scroll(),
    };
    frame.render_stateful_widget(widget, area, &mut state);
}

fn render_conversation(app: &App, frame: &mut Frame, area: Rect) {
    let scroll = app.scroll_offset();
    let widget = ConversationWidget::new(
        app.messages(),
        app.theme(),
        app.animation(),
        scroll,
    );

    // If we have a scroll offset, prepend a scroll indicator line.
    if scroll > 0 {
        // Split off a 1-row indicator at the top, leaving the rest for the widget.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        let indicator = format!("^ {scroll} lines above");
        let muted = Style::default().fg(app.theme().text_muted);
        let line = Line::from(Span::styled(indicator, muted));
        let para = Paragraph::new(line);
        frame.render_widget(para, chunks[0]);
        frame.render_widget(widget, chunks[1]);
    } else {
        frame.render_widget(widget, area);
    }
}

fn render_file_preview(app: &App, frame: &mut Frame, area: Rect) {
    let widget = FilePreviewWidget::new(
        app.preview_content.as_deref(),
        app.preview_path.as_deref(),
        app.theme(),
    );
    frame.render_widget(widget, area);
}

fn render_tool_panel(app: &App, frame: &mut Frame, area: Rect) {
    let theme = app.theme();
    let border_style = Style::default().fg(theme.surface_border);
    let title_style = Style::default().fg(theme.text_muted);
    let block = Block::default()
        .borders(ratatui::widgets::Borders::ALL)
        .border_type(ratatui::widgets::BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled(" tool output ", title_style));
    frame.render_widget(block, area);
}

fn render_input_area(app: &App, frame: &mut Frame, area: Rect) {
    let widget = InputWidget::new(
        app.input(),
        app.theme(),
        app.focus(),
        app.state(),
    );
    frame.render_widget(widget, area);

    // Position the terminal cursor inside the input box so the user can see
    // where they are typing. The input area has a 1-cell border on each side,
    // so the content starts at (area.x + 1, area.y + 1).
    let cursor_byte = app.input().cursor_position;
    let text_before_cursor = &app.input().buffer[..cursor_byte];
    let inner_width = area.width.saturating_sub(2) as usize;

    // Walk through the text before the cursor, tracking rows and columns.
    // Each newline resets the column and advances the row. Within a line,
    // wrapping occurs every `inner_width` characters.
    let mut row: u16 = 0;
    let mut col: usize = 0;
    for ch in text_before_cursor.chars() {
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
            if inner_width > 0 && col >= inner_width {
                row += 1;
                col = 0;
            }
        }
    }
    let x = area.x + 1 + col as u16;
    let y = area.y + 1 + row;
    if y < area.bottom() && x < area.right() {
        frame.set_cursor_position((x, y));
    }
}

fn render_status_bar_area(app: &App, frame: &mut Frame, area: Rect) {
    let line = render_status_bar(app.status(), app.theme(), app.animation(), area.width);
    let para = Paragraph::new(line)
        .style(Style::default().bg(app.theme().surface_base));
    frame.render_widget(para, area);
}

fn render_autocomplete(app: &App, frame: &mut Frame, input_area: Rect) {
    let input_buf = &app.input().buffer;

    let (matches, sigil): (Vec<&'static str>, char) = if input_buf.starts_with('/') {
        (Command::completions(&input_buf[1..]), '/')
    } else if input_buf.starts_with('@') {
        (Command::agent_completions(&input_buf[1..]), '@')
    } else {
        return;
    };

    // Hide when no completions or the single match is already exact.
    if matches.is_empty()
        || (matches.len() == 1 && format!("{sigil}{}", matches[0]) == *input_buf)
    {
        return;
    }

    let theme = app.theme();
    let count = matches.len().min(8) as u16;
    // Popup height: one row per suggestion + top/bottom border.
    let popup_height = count + 2;
    let popup_width = 34u16.min(input_area.width.saturating_sub(2));
    if popup_width < 6 || input_area.y < popup_height {
        return; // not enough space
    }
    let popup_y = input_area.y - popup_height;
    let popup_x = input_area.x + 1;
    let area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    let bg_style = Style::default().bg(theme.surface_overlay).fg(theme.text_primary);
    let border_style = Style::default().bg(theme.surface_overlay).fg(theme.surface_border);
    let buf = frame.buffer_mut();

    // Fill background.
    for row in 0..popup_height {
        for col in 0..popup_width {
            buf[(area.x + col, area.y + row)].set_char(' ').set_style(bg_style);
        }
    }

    // Draw rounded border.
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

    // Write suggestion lines inside the border.
    let inner_width = (popup_width.saturating_sub(2)) as usize;
    for (i, name) in matches.iter().take(count as usize).enumerate() {
        let row = area.y + 1 + i as u16;
        let text = format!(" {sigil}{name}");
        let mut col = area.x + 1;
        for ch in text.chars().take(inner_width) {
            buf[(col, row)].set_char(ch).set_style(bg_style);
            col += 1;
        }
    }
}

fn render_help_overlay(app: &App, frame: &mut Frame, area: Rect) {
    let widget = HelpOverlay::new(app.theme());
    frame.render_widget(widget, area);
}
