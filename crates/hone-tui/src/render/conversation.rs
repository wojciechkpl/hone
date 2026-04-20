use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Widget,
};

use crate::{
    animation::AnimationState,
    message::{DisplayMessage, DisplayRole},
    theme::Theme,
};

use super::markdown::markdown_to_lines;
use super::tool_call::render_tool_call;

/// Maximum number of lines rendered for a ToolResult message before the
/// output is truncated and a "[+N more lines]" trailer is appended.
const MAX_TOOL_PREVIEW_LINES: usize = 40;

/// Widget that renders the full conversation message list.
pub struct ConversationWidget<'a> {
    pub messages: &'a [DisplayMessage],
    pub theme: &'a Theme,
    pub anim: &'a AnimationState,
    pub scroll_offset: usize,
}

impl<'a> ConversationWidget<'a> {
    pub fn new(
        messages: &'a [DisplayMessage],
        theme: &'a Theme,
        anim: &'a AnimationState,
        scroll_offset: usize,
    ) -> Self {
        Self { messages, theme, anim, scroll_offset }
    }

    /// Convert all messages to a flat list of `Line`s for rendering.
    fn build_lines(&self) -> Vec<Line<'static>> {
        let mut output: Vec<Line<'static>> = Vec::new();

        for msg in self.messages {
            let msg_lines = self.render_message(msg);
            output.extend(msg_lines);
            // Blank line between messages.
            output.push(Line::default());
        }

        output
    }

    fn render_message(&self, msg: &DisplayMessage) -> Vec<Line<'static>> {
        let theme = self.theme;

        match &msg.role {
            DisplayRole::User => self.render_user(msg),
            DisplayRole::Assistant => self.render_assistant(msg),
            DisplayRole::ToolCall { name } => {
                let spinner = if msg.is_active {
                    Some(self.anim.spinner_char())
                } else {
                    None
                };
                render_tool_call(name, &msg.content, false, theme, spinner)
            }
            DisplayRole::ToolResult => {
                let is_error = msg.content.starts_with("error:");
                if msg.is_collapsed {
                    render_tool_result_collapsed(&msg.content, is_error, theme)
                } else {
                    let content = truncate_tool_output(&msg.content, theme);
                    render_tool_call("tool-result", &*content, is_error, theme, None)
                }
            }
            DisplayRole::Error => {
                let style = Style::default().fg(theme.role_error);
                msg.content
                    .lines()
                    .map(|l| Line::from(Span::styled(l.to_string(), style)))
                    .collect()
            }
            DisplayRole::System => {
                let style = Style::default().fg(theme.role_system);
                msg.content
                    .lines()
                    .map(|l| Line::from(Span::styled(l.to_string(), style)))
                    .collect()
            }
        }
    }

    fn render_user(&self, msg: &DisplayMessage) -> Vec<Line<'static>> {
        let theme = self.theme;
        let border_style = Style::default().fg(theme.role_user);
        let label_style = Style::default().fg(theme.role_user);
        let content_style = Style::default().fg(theme.text_primary);

        // Role label.
        let label = Line::from(Span::styled("you", label_style));

        // Rounded-border box.
        // Top: ╭──────────
        let top = Line::from(vec![
            Span::styled("\u{256D}", border_style),
            Span::styled("\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}", border_style),
        ]);

        let mut lines = vec![label, top];

        for text_line in msg.content.lines() {
            lines.push(Line::from(vec![
                Span::styled("\u{2502} ", border_style),
                Span::styled(text_line.to_string(), content_style),
            ]));
        }

        // Bottom: ╰──────────
        lines.push(Line::from(vec![
            Span::styled("\u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}", border_style),
        ]));

        lines
    }

    fn render_assistant(&self, msg: &DisplayMessage) -> Vec<Line<'static>> {
        let theme = self.theme;
        let label_style = Style::default().fg(theme.role_assistant);

        let label = Line::from(Span::styled("hone", label_style));
        let mut lines = vec![label];

        let content = &msg.content;
        let mut md_lines = markdown_to_lines(content, theme);

        // If streaming, append a blinking cursor on the last content line.
        if msg.is_streaming && self.anim.blink_visible() {
            let cursor = Span::styled("_", Style::default().fg(theme.accent_hot));
            if let Some(last) = md_lines.last_mut() {
                last.spans.push(cursor);
            } else {
                md_lines.push(Line::from(cursor));
            }
        }

        // If content is empty but still streaming, show just the cursor.
        if content.is_empty() && msg.is_streaming && self.anim.blink_visible() {
            md_lines.push(Line::from(Span::styled("_", Style::default().fg(theme.accent_hot))));
        }

        lines.extend(md_lines);
        lines
    }
}

/// Render a collapsed ToolResult as a single summary line.
///
/// Shows `▶ <first line of content> [enter to expand]` (or `> ` in NO_COLOR mode).
fn render_tool_result_collapsed(content: &str, is_error: bool, theme: &Theme) -> Vec<Line<'static>> {
    let border_color = if is_error {
        theme.role_error
    } else {
        theme.role_tool_result
    };

    let border_style = Style::default().fg(border_color);
    let content_style = Style::default().fg(theme.text_secondary);
    let hint_style = Style::default().fg(theme.text_muted);

    let arrow = if theme.no_color { "> " } else { "▶ " };
    let first_line = content.lines().next().unwrap_or("").to_string();
    let hint = " [enter to expand]";

    let line = Line::from(vec![
        Span::styled(arrow.to_string(), border_style),
        Span::styled(first_line, content_style),
        Span::styled(hint.to_string(), hint_style),
    ]);

    vec![line]
}

/// Truncate tool output to `MAX_TOOL_PREVIEW_LINES` lines.
///
/// If the content has more than `MAX_TOOL_PREVIEW_LINES` lines, return a new
/// `String` with only the first `MAX_TOOL_PREVIEW_LINES` lines followed by a
/// dim "[+N more lines]" trailer line.  Otherwise return the original content
/// unchanged (no allocation).
fn truncate_tool_output<'a>(content: &'a str, _theme: &Theme) -> std::borrow::Cow<'a, str> {
    let total: usize = content.lines().count();
    if total <= MAX_TOOL_PREVIEW_LINES {
        return std::borrow::Cow::Borrowed(content);
    }
    let truncated: String = content
        .lines()
        .take(MAX_TOOL_PREVIEW_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    let remaining = total - MAX_TOOL_PREVIEW_LINES;
    std::borrow::Cow::Owned(format!("{truncated}\n[+{remaining} more lines]"))
}

impl<'a> Widget for ConversationWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let all_lines = self.build_lines();

        // scroll_offset is "distance from bottom": 0 = newest messages visible,
        // increasing = scrolled further up into history.
        let total = all_lines.len();
        let available = area.height as usize;
        let start = if total > available {
            (total - available).saturating_sub(self.scroll_offset)
        } else {
            0
        };
        let visible: Vec<&Line<'static>> = all_lines
            .iter()
            .skip(start)
            .take(available)
            .collect();

        for (row, line) in visible.iter().enumerate() {
            let y = area.top() + row as u16;
            let mut x = area.left();

            for span in &line.spans {
                let style = span.style;
                for ch in span.content.chars() {
                    if x >= area.right() {
                        break;
                    }
                    buf[(x, y)].set_char(ch).set_style(style);
                    x += 1;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect, widgets::Widget};
    use crate::{
        animation::AnimationState,
        message::{DisplayMessage, DisplayRole},
        theme::Theme,
    };
    use super::ConversationWidget;

    fn render_conversation_to_string(messages: &[DisplayMessage], width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = Theme::crimson();
        let anim = AnimationState::new();

        terminal.draw(|f| {
            let widget = ConversationWidget::new(messages, &theme, &anim, 0);
            f.render_widget(widget, Rect::new(0, 0, width, height));
        }).unwrap();

        let buf = terminal.backend().buffer().clone();
        let mut result = String::new();
        for row in 0..height {
            for col in 0..width {
                let ch = buf.cell((col, row))
                    .map(|c| c.symbol().chars().next().unwrap_or(' '))
                    .unwrap_or(' ');
                result.push(ch);
            }
            result.push('\n');
        }
        result
    }

    /// A collapsed ToolResult shows the triangle arrow and first line only.
    #[test]
    fn collapsed_tool_result_shows_arrow_and_first_line() {
        let mut msg = DisplayMessage::new(DisplayRole::ToolResult, "first line\nsecond line\nthird line");
        msg.is_collapsed = true;

        let rendered = render_conversation_to_string(&[msg], 80, 5);

        assert!(rendered.contains('▶'), "collapsed tool result must show '▶' arrow");
        assert!(rendered.contains("first line"), "collapsed tool result must show first line");
    }

    /// A collapsed ToolResult does NOT show lines beyond the first.
    #[test]
    fn collapsed_tool_result_hides_subsequent_lines() {
        let mut msg = DisplayMessage::new(DisplayRole::ToolResult, "first line\nsecond line\nthird line");
        msg.is_collapsed = true;

        let rendered = render_conversation_to_string(&[msg], 80, 5);

        assert!(!rendered.contains("second line"), "collapsed tool result must not show second line");
        assert!(!rendered.contains("third line"), "collapsed tool result must not show third line");
    }

    /// A collapsed ToolResult shows the expand hint.
    #[test]
    fn collapsed_tool_result_shows_expand_hint() {
        let mut msg = DisplayMessage::new(DisplayRole::ToolResult, "output");
        msg.is_collapsed = true;

        let rendered = render_conversation_to_string(&[msg], 80, 5);

        assert!(rendered.contains("enter to expand"), "collapsed tool result must show expand hint");
    }

    /// An expanded ToolResult shows all content lines.
    #[test]
    fn expanded_tool_result_shows_all_lines() {
        let mut msg = DisplayMessage::new(DisplayRole::ToolResult, "first line\nsecond line");
        msg.is_collapsed = false;

        let rendered = render_conversation_to_string(&[msg], 80, 10);

        assert!(rendered.contains("first line"), "expanded tool result must show first line");
        assert!(rendered.contains("second line"), "expanded tool result must show second line");
    }

    /// An expanded ToolResult does NOT show the `▶` collapse arrow or the expand hint.
    #[test]
    fn expanded_tool_result_does_not_show_collapsed_indicator() {
        let mut msg = DisplayMessage::new(DisplayRole::ToolResult, "output content");
        msg.is_collapsed = false;

        let rendered = render_conversation_to_string(&[msg], 80, 10);

        assert!(!rendered.contains('▶'), "expanded tool result must not show '▶'");
        assert!(!rendered.contains("enter to expand"), "expanded tool result must not show expand hint");
    }
}
