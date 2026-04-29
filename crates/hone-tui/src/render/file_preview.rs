use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Widget},
};

use crate::{syntax::lex_line, theme::Theme};

/// Widget that renders a file preview pane with syntax-highlighted content.
pub struct FilePreviewWidget<'a> {
    pub content: Option<&'a str>,
    pub file_path: Option<&'a str>,
    pub theme: &'a Theme,
}

impl<'a> FilePreviewWidget<'a> {
    pub fn new(content: Option<&'a str>, file_path: Option<&'a str>, theme: &'a Theme) -> Self {
        Self {
            content,
            file_path,
            theme,
        }
    }

    /// Infer language from file extension.
    fn language(path: &str) -> &'static str {
        if path.ends_with(".rs") {
            "rust"
        } else if path.ends_with(".py") {
            "python"
        } else {
            "text"
        }
    }
}

impl<'a> Widget for FilePreviewWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = self.theme;
        let border_style = Style::default().fg(theme.surface_border);
        let title_style = Style::default().fg(theme.text_secondary);

        let title = self
            .file_path
            .map(|p| format!(" {p} "))
            .unwrap_or_else(|| " preview ".to_string());

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(border_style)
            .title(Span::styled(title, title_style))
            .title_alignment(Alignment::Left);

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        match self.content {
            None => {
                // Centered "No file loaded" placeholder.
                let msg = "No file loaded";
                let muted = Style::default().fg(theme.text_muted);
                let x_offset = inner.width.saturating_sub(msg.len() as u16) / 2;
                let y_offset = inner.height / 2;
                let x = inner.left() + x_offset;
                let y = inner.top() + y_offset;
                let mut cur_x = x;
                for ch in msg.chars() {
                    if cur_x >= inner.right() {
                        break;
                    }
                    buf[(cur_x, y)].set_char(ch).set_style(muted);
                    cur_x += 1;
                }
            }
            Some(content) => {
                let language = self.file_path.map(Self::language).unwrap_or("text");

                for (row, raw_line) in content.lines().enumerate() {
                    let y = inner.top() + row as u16;
                    if y >= inner.bottom() {
                        break;
                    }
                    let spans = lex_line(raw_line, language, theme);
                    let line = Line::from(spans);
                    let mut x = inner.left();
                    for span in &line.spans {
                        for ch in span.content.chars() {
                            if x >= inner.right() {
                                break;
                            }
                            buf[(x, y)].set_char(ch).set_style(span.style);
                            x += 1;
                        }
                    }
                }
            }
        }
    }
}
