use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Paragraph},
};

use crate::{
    app::App,
    layout::LayoutMode,
    render::{
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
    match app.layout() {
        LayoutMode::SinglePane => {
            render_conversation(app, frame, area);
        }
        LayoutMode::SplitHorizontal { ratio } => {
            let ratio = *ratio;
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(ratio),
                    Constraint::Percentage(100 - ratio),
                ])
                .split(area);

            render_conversation(app, frame, chunks[0]);
            render_file_preview(app, frame, chunks[1]);
        }
        LayoutMode::BottomPanel => {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
                .split(area);

            render_conversation(app, frame, chunks[0]);
            render_tool_panel(app, frame, chunks[1]);
        }
    }
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
}

fn render_status_bar_area(app: &App, frame: &mut Frame, area: Rect) {
    let line = render_status_bar(app.status(), app.theme(), app.animation());
    let para = Paragraph::new(line)
        .style(Style::default().bg(app.theme().surface_base));
    frame.render_widget(para, area);
}

fn render_help_overlay(app: &App, frame: &mut Frame, area: Rect) {
    let widget = HelpOverlay::new(app.theme());
    frame.render_widget(widget, area);
}
