use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::{animation::AnimationState, theme::Theme};

/// The ASCII art mark for the splash screen.
///
/// Each row is represented as `(prefix_spaces, arrows, equals_count)` so the
/// renderer can colour `>` and `=` independently without storing raw strings.
const MARK_ROWS: &[&str] = &["  >=>", " >==>", ">===>", " >==>", "  >=>"];

/// Startup splash widget: centered ASCII mark + tagline.
pub struct SplashWidget<'a> {
    pub theme: &'a Theme,
    pub anim: &'a AnimationState,
}

impl<'a> SplashWidget<'a> {
    pub fn new(theme: &'a Theme, anim: &'a AnimationState) -> Self {
        Self { theme, anim }
    }

    /// Build all lines that make up the splash (mark + title + tagline).
    fn build_lines(&self) -> Vec<Line<'static>> {
        let theme = self.theme;
        let hot = Style::default()
            .fg(theme.accent_hot)
            .add_modifier(Modifier::BOLD);
        let deep = Style::default().fg(theme.accent_deep);
        let title_style = Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD);
        let sub_style = Style::default().fg(theme.text_secondary);

        let mut lines: Vec<Line<'static>> = Vec::new();

        // Blank line above the mark for vertical centering feel.
        lines.push(Line::default());

        for row in MARK_ROWS {
            let spans = colour_mark_row(row, hot, deep);
            lines.push(Line::from(spans).alignment(Alignment::Center));
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled("H O N E", title_style)).alignment(Alignment::Center));
        lines.push(
            Line::from(Span::styled("sharpen your code", sub_style)).alignment(Alignment::Center),
        );
        lines.push(Line::default());

        // Show a simple animated hint when the animation frame is visible.
        if self.anim.blink_visible() {
            lines.push(
                Line::from(Span::styled(
                    "press any key to begin",
                    Style::default().fg(theme.text_muted),
                ))
                .alignment(Alignment::Center),
            );
        } else {
            lines.push(Line::default());
        }

        lines
    }
}

/// Colour the `>` characters with `hot` style and `=` characters with `deep`.
/// Spaces are emitted with the default style.
fn colour_mark_row(row: &str, hot: Style, deep: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for ch in row.chars() {
        let span = match ch {
            '>' => Span::styled(">", hot),
            '=' => Span::styled("=", deep),
            ' ' => Span::styled(" ", Style::default()),
            other => Span::styled(other.to_string(), Style::default()),
        };
        spans.push(span);
    }
    spans
}

impl<'a> Widget for SplashWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let lines = self.build_lines();
        let total_lines = lines.len() as u16;

        // Vertical centering: start rendering so the block is centered.
        let start_y = if total_lines < area.height {
            area.top() + (area.height - total_lines) / 2
        } else {
            area.top()
        };

        for (idx, line) in lines.iter().enumerate() {
            let y = start_y + idx as u16;
            if y >= area.bottom() {
                break;
            }

            // Collect the full text to measure for centering.
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let text_len = text.chars().count() as u16;

            let start_x = if text_len < area.width {
                area.left() + (area.width - text_len) / 2
            } else {
                area.left()
            };

            let mut x = start_x;
            for span in &line.spans {
                for ch in span.content.chars() {
                    if x >= area.right() {
                        break;
                    }
                    buf[(x, y)].set_char(ch).set_style(span.style);
                    x += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn test_splash_contains_mark_chars() {
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let widget = SplashWidget::new(&theme, &anim);
        let lines = widget.build_lines();
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref().to_string()))
            .collect();
        assert!(all_text.contains(">=>"), "splash must contain >=> mark");
    }

    #[test]
    fn test_splash_contains_tagline() {
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let widget = SplashWidget::new(&theme, &anim);
        let lines = widget.build_lines();
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref().to_string()))
            .collect();
        assert!(all_text.contains("sharpen your code"));
    }
}
