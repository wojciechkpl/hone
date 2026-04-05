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

use super::tool_call::render_tool_call;

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
                render_tool_call(name, &msg.content, false, theme)
            }
            DisplayRole::ToolResult => {
                let is_error = msg.content.starts_with("error:");
                render_tool_call("tool-result", &msg.content, is_error, theme)
            }
            DisplayRole::Error => {
                vec![Line::from(Span::styled(
                    msg.content.clone(),
                    Style::default().fg(theme.role_error),
                ))]
            }
            DisplayRole::System => {
                vec![Line::from(Span::styled(
                    msg.content.clone(),
                    Style::default().fg(theme.role_system),
                ))]
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
        let content_style = Style::default().fg(theme.text_primary);

        let label = Line::from(Span::styled("hone", label_style));
        let mut lines = vec![label];

        let content = &msg.content;
        let source_lines: Vec<&str> = content.lines().collect();

        for (idx, text_line) in source_lines.iter().enumerate() {
            let is_last = idx == source_lines.len().saturating_sub(1);
            let show_cursor = is_last && msg.is_streaming && self.anim.blink_visible();

            let mut spans: Vec<Span<'static>> = vec![
                Span::styled((*text_line).to_string(), content_style),
            ];
            if show_cursor {
                spans.push(Span::styled("_", Style::default().fg(theme.accent_hot)));
            }
            lines.push(Line::from(spans));
        }

        // If content is empty but still streaming, show just the cursor.
        if content.is_empty() && msg.is_streaming && self.anim.blink_visible() {
            lines.push(Line::from(Span::styled("_", Style::default().fg(theme.accent_hot))));
        }

        lines
    }
}

impl<'a> Widget for ConversationWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let all_lines = self.build_lines();

        // Apply scroll: skip `scroll_offset` lines from the top.
        let visible: Vec<&Line<'static>> = all_lines
            .iter()
            .skip(self.scroll_offset)
            .take(area.height as usize)
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
