use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::theme::Theme;

/// Render a tool call (or tool result) panel as a list of lines.
///
/// A sharp-border header shows the tool name, with color indicating success or error.
/// When `spinner` is `Some(ch)` the tool is still executing: the spinner character
/// is prepended to the name and a "running…" body line is shown instead of empty content.
pub fn render_tool_call(
    name: &str,
    content: &str,
    is_error: bool,
    theme: &Theme,
    spinner: Option<char>,
) -> Vec<Line<'static>> {
    let border_color = if is_error {
        theme.role_error
    } else {
        theme.role_tool_result
    };

    let border_style = Style::default().fg(border_color);
    let label_style = Style::default().fg(theme.role_tool);
    let content_style = Style::default().fg(theme.text_secondary);
    let muted_style = Style::default().fg(theme.text_muted);

    let title = match spinner {
        Some(ch) => format!(" {ch} {name} "),
        None => format!(" {name} "),
    };

    let header = Line::from(vec![
        Span::styled("\u{250C}\u{2500}", border_style),
        Span::styled(title, label_style),
        Span::styled("\u{2500}\u{2500}\u{2500}", border_style),
    ]);

    let mut lines = vec![header];

    if spinner.is_some() && content.is_empty() {
        // Tool is running — show an animated "running…" placeholder.
        let ellipsis = if theme.no_color { "..." } else { "…" };
        lines.push(Line::from(vec![
            Span::styled("\u{2502} ", border_style),
            Span::styled(format!("running{ellipsis}"), muted_style),
        ]));
    } else {
        for text_line in content.lines() {
            lines.push(Line::from(vec![
                Span::styled("\u{2502} ", border_style),
                Span::styled(text_line.to_string(), content_style),
            ]));
        }

        if content.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("\u{2502}", border_style),
            ]));
        }
    }

    lines.push(Line::from(vec![
        Span::styled("\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}", border_style),
    ]));

    lines
}
