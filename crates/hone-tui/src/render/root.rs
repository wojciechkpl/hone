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
        command_palette::render_command_palette,
        conversation::ConversationWidget,
        debugger::render_debugger,
        file_preview::FilePreviewWidget,
        header::render_header,
        help::HelpOverlay,
        history_search::render_history_search,
        input_widget::InputWidget,
        prompt_mirror::render_prompt_mirror,
        search_bar::render_search_bar,
        session_anatomy::render_session_anatomy,
        session_switcher::render_session_switcher,
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

    // History-search overlay above the input area (shown when Ctrl+F is active)
    render_history_search(app, frame, chunks[2]);

    // Scrollback search bar at bottom of content area
    if app.search_open {
        render_search_bar(app, frame, chunks[1]);
    }

    if app.show_help() {
        render_help_overlay(app, frame, area);
    }

    // Command palette — full-area overlay
    if app.palette_open {
        render_command_palette(app, frame, area);
    }

    // Session Anatomy — full-area overlay, rendered last
    if app.anatomy_open {
        render_session_anatomy(app, frame, area);
    }

    // Prompt Mirror — full-area overlay
    if app.mirror_open {
        render_prompt_mirror(app, frame, area);
    }

    // Agent Debugger — full-area overlay, highest z-order
    if app.debugger_open {
        render_debugger(app, frame, area);
    }

    // Session Switcher — full-area overlay
    if app.sessions_open {
        render_session_switcher(app, frame, area);
    }

    // Mouse-drag text selection highlight — drawn last so it overlays every
    // widget (selection is indicated by flipping cells to reverse-video).
    if let Some(sel) = app.selection() {
        crate::selection::apply_highlight(frame.buffer_mut(), sel);
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
    let para = Paragraph::new(line).style(Style::default().bg(app.theme().surface_raised));
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
    let widget = ConversationWidget::new(app.messages(), app.theme(), app.animation(), scroll);

    // If we have a scroll offset, prepend a scroll indicator line.
    if scroll > 0 {
        // Split off a 1-row indicator at the top, leaving the rest for the widget.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);

        let indicator = format!("↓ {scroll} lines below  (G to jump to latest)");
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
    let widget = InputWidget::new(app.input(), app.theme(), app.focus(), app.state());
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
    let is_waiting = matches!(app.state(), crate::app::AppState::WaitingForResponse);
    let line = render_status_bar(
        app.status(),
        app.theme(),
        app.animation(),
        area.width,
        is_waiting,
    );
    let para = Paragraph::new(line).style(Style::default().bg(app.theme().surface_base));
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
    if matches.is_empty() || (matches.len() == 1 && format!("{sigil}{}", matches[0]) == *input_buf)
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

    let bg_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.text_primary);
    let border_style = Style::default()
        .bg(theme.surface_overlay)
        .fg(theme.surface_border);
    let buf = frame.buffer_mut();

    // Fill background.
    for row in 0..popup_height {
        for col in 0..popup_width {
            buf[(area.x + col, area.y + row)]
                .set_char(' ')
                .set_style(bg_style);
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
        buf[(area.x + col, area.y)]
            .set_char('─')
            .set_style(border_style);
        buf[(area.x + col, bottom)]
            .set_char('─')
            .set_style(border_style);
    }
    for row in 1..popup_height - 1 {
        buf[(area.x, area.y + row)]
            .set_char('│')
            .set_style(border_style);
        buf[(right, area.y + row)]
            .set_char('│')
            .set_style(border_style);
    }

    // Write suggestion lines inside the border.
    let selected = app.completion_index();
    let sel_style = Style::default().bg(theme.accent_hot).fg(theme.surface_base);
    let inner_width = (popup_width.saturating_sub(2)) as usize;
    for (i, name) in matches.iter().take(count as usize).enumerate() {
        let row = area.y + 1 + i as u16;
        let is_sel = selected == Some(i);
        let row_style = if is_sel { sel_style } else { bg_style };
        // Fill the whole inner row with the row background first.
        for col in 0..inner_width as u16 {
            buf[(area.x + 1 + col, row)]
                .set_char(' ')
                .set_style(row_style);
        }
        let text = format!(" {sigil}{name}");
        let mut col = area.x + 1;
        for ch in text.chars().take(inner_width) {
            buf[(col, row)].set_char(ch).set_style(row_style);
            col += 1;
        }
    }
}

fn render_help_overlay(app: &App, frame: &mut Frame, area: Rect) {
    let widget = HelpOverlay::new(app.theme());
    frame.render_widget(widget, area);
}

#[cfg(test)]
mod tests {
    use super::render;
    use crate::app::App;
    use ratatui::{Terminal, backend::TestBackend};

    fn app_with_input(text: &str) -> App {
        let mut app = App::new();
        app.show_splash = false;
        for ch in text.chars() {
            app.input.insert_char(ch);
        }
        app
    }

    fn cell_char(terminal: &Terminal<TestBackend>, x: u16, y: u16) -> char {
        terminal
            .backend()
            .buffer()
            .cell((x, y))
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .unwrap_or(' ')
    }

    #[test]
    fn autocomplete_slash_renders_border() {
        let app = app_with_input("/");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(&app, f)).unwrap();

        // Layout: header(1) + content(19) + input(3) + status(1) on 24-row term.
        // Input area y = 20. With 8 matches, popup_height = 10, popup_y = 10.
        // popup_x = input_area.x + 1 = 1.
        let popup_x: u16 = 1;
        let popup_y: u16 = 10;
        assert_eq!(
            cell_char(&terminal, popup_x, popup_y),
            '╭',
            "expected top-left corner of autocomplete popup at ({popup_x},{popup_y})"
        );
        assert_eq!(
            cell_char(&terminal, popup_x + 1, popup_y + 1),
            ' ',
            "expected leading space of first suggestion row"
        );
    }

    #[test]
    fn autocomplete_at_renders_border() {
        let app = app_with_input("@");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(&app, f)).unwrap();

        let popup_x: u16 = 1;
        let popup_y: u16 = 10; // 12 agents → count=8 → popup_height=10 → y=20-10=10
        assert_eq!(
            cell_char(&terminal, popup_x, popup_y),
            '╭',
            "expected top-left corner of @-agent popup at ({popup_x},{popup_y})"
        );
    }

    fn cell_bg(terminal: &Terminal<TestBackend>, x: u16, y: u16) -> ratatui::style::Color {
        terminal
            .backend()
            .buffer()
            .cell((x, y))
            .map(|c| c.style().bg.unwrap_or(ratatui::style::Color::Reset))
            .unwrap_or(ratatui::style::Color::Reset)
    }

    #[test]
    fn autocomplete_exact_match_hides_popup() {
        // "/quit" is an exact match → popup suppressed (1 match, input == "/quit").
        let app = app_with_input("/quit");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(&app, f)).unwrap();
        // 1 match → popup_height=3, popup_y=20-3=17
        assert_ne!(
            cell_char(&terminal, 1, 17),
            '╭',
            "popup must not render for exact match"
        );
    }

    #[test]
    fn autocomplete_partial_match_shows_popup() {
        // "/qu" → ["quit"] — one non-exact match → popup shown
        let app = app_with_input("/qu");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(&app, f)).unwrap();
        // 1 match → popup_height=3, popup_y=20-3=17
        assert_eq!(
            cell_char(&terminal, 1, 17),
            '╭',
            "popup must render for partial match"
        );
    }

    #[test]
    fn autocomplete_empty_input_shows_no_popup() {
        let app = app_with_input("");
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(&app, f)).unwrap();
        assert_ne!(cell_char(&terminal, 1, 10), '╭', "no popup for empty input");
    }

    #[test]
    fn autocomplete_selected_row_has_accent_bg() {
        // "/" → 8 matches, popup_height=10, popup_y=10. Row 0 is at y=11.
        let mut app = app_with_input("/");
        app.completion_index = Some(0);
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| render(&app, f)).unwrap();

        let theme = crate::theme::Theme::crimson();
        let selected_bg = cell_bg(&terminal, 2, 11); // x=popup_x+1=2, y=popup_y+1=11
        assert_eq!(
            selected_bg, theme.accent_hot,
            "selected row must have accent_hot bg"
        );
        let unselected_bg = cell_bg(&terminal, 2, 12);
        assert_ne!(
            unselected_bg, theme.accent_hot,
            "unselected row must not have accent_hot bg"
        );
    }
}
