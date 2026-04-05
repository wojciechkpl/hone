use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::theme::Theme;

/// Render a tool call (or tool result) panel as a list of lines.
///
/// A sharp-border header shows the tool name, with color indicating success or error.
pub fn render_tool_call(name: &str, content: &str, is_error: bool, theme: &Theme) -> Vec<Line<'static>> {
    let border_color = if is_error {
        theme.role_error
    } else {
        theme.role_tool_result
    };

    let border_style = Style::default().fg(border_color);
    let label_style = Style::default().fg(theme.role_tool);
    let content_style = Style::default().fg(theme.text_secondary);

    let title = format!(" {} ", name);
    // Build a header line: ┌─ <name> ───────
    let header = Line::from(vec![
        Span::styled("\u{250C}\u{2500}", border_style),
        Span::styled(title, label_style),
        Span::styled("\u{2500}\u{2500}\u{2500}", border_style),
    ]);

    let mut lines = vec![header];

    for text_line in content.lines() {
        lines.push(Line::from(vec![
            Span::styled("\u{2502} ", border_style),
            Span::styled(text_line.to_string(), content_style),
        ]));
    }

    // If content is empty, add a blank interior line so the box isn't just a header.
    if content.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("\u{2502}", border_style),
        ]));
    }

    // Footer line: └──────────────────
    lines.push(Line::from(vec![
        Span::styled("\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}", border_style),
    ]));

    lines
}
