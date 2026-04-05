use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};

use crate::{
    app::AppState,
    input::InputState,
    layout::FocusTarget,
    theme::Theme,
};

/// Widget that renders the user-input area with a rounded border.
pub struct InputWidget<'a> {
    pub input: &'a InputState,
    pub theme: &'a Theme,
    pub focus: &'a FocusTarget,
    pub state: &'a AppState,
}

impl<'a> InputWidget<'a> {
    pub fn new(
        input: &'a InputState,
        theme: &'a Theme,
        focus: &'a FocusTarget,
        state: &'a AppState,
    ) -> Self {
        Self { input, theme, focus, state }
    }
}

impl<'a> Widget for InputWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = self.theme;

        // Border color depends on app state and focus.
        let border_color = match self.state {
            AppState::Error(_) => theme.role_error,
            _ if *self.focus == FocusTarget::Input => theme.surface_focus,
            _ => theme.surface_border,
        };

        let title_style = Style::default().fg(theme.accent_hot);
        let border_style = Style::default().fg(border_color);
        let content_style = Style::default().fg(theme.text_primary);
        let hint_style = Style::default().fg(theme.text_muted);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(Span::styled(" > ", title_style));

        // The inner area is what remains after the block border.
        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 {
            return;
        }

        // Render the content lines within the inner area.
        let content = &self.input.buffer;

        if content.is_empty() {
            // Hint line when nothing has been typed.
            let hint = Line::from(Span::styled("/help for commands", hint_style));
            let y = inner.top();
            let mut x = inner.left();
            for span in &hint.spans {
                for ch in span.content.chars() {
                    if x >= inner.right() {
                        break;
                    }
                    buf[(x, y)].set_char(ch).set_style(span.style);
                    x += 1;
                }
            }
        } else {
            // Render the input buffer, wrapping at the inner width.
            let width = inner.width as usize;
            let mut row = 0u16;

            for line_str in content.lines() {
                if row >= inner.height {
                    break;
                }
                // Chunk into screen-width segments.
                let mut remaining = line_str;
                loop {
                    if row >= inner.height {
                        break;
                    }
                    let chunk_len = remaining.len().min(width);
                    let chunk = &remaining[..chunk_len];
                    let y = inner.top() + row;
                    let mut x = inner.left();
                    for ch in chunk.chars() {
                        if x >= inner.right() {
                            break;
                        }
                        buf[(x, y)].set_char(ch).set_style(content_style);
                        x += 1;
                    }
                    remaining = &remaining[chunk_len..];
                    row += 1;
                    if remaining.is_empty() {
                        break;
                    }
                }
            }
        }
    }
}
