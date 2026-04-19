use ratatui::{
    buffer::Buffer,
    layout::{Alignment, Rect},
    style::Style,
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Widget},
};

use crate::theme::Theme;

const KEYBINDINGS: &[(&str, &str)] = &[
    // Input
    ("Enter", "send message"),
    ("Shift+Enter / Alt+Enter", "insert newline"),
    ("Up / Down", "prompt history"),
    ("Ctrl+F", "history search"),
    ("Ctrl+K", "command palette"),
    ("Esc (typing)", "clear input"),
    ("", ""),
    // Navigation
    ("Ctrl+/", "scrollback search"),
    ("j / k (conv focus)", "navigate messages"),
    ("Enter / Space", "expand/collapse tool card"),
    ("PageUp / PageDown", "scroll conversation"),
    ("Tab", "cycle focus panes"),
    ("", ""),
    // Overlays
    ("Ctrl+A", "session anatomy (turn stats)"),
    ("Ctrl+M", "prompt mirror (context preview)"),
    ("Ctrl+O", "session switcher"),
    ("Ctrl+R", "toggle agent rail sidebar"),
    ("F1", "toggle this help"),
    ("", ""),
    // Agent control
    ("Esc (mid-turn)", "cancel current turn"),
    ("--step flag", "step-through debugger"),
    ("", ""),
    // Commands
    ("/help", "show help text"),
    ("/model <id>", "switch model"),
    ("/cost", "show session cost"),
    ("/split", "toggle split layout"),
    ("/quit", "quit"),
    ("Ctrl+C", "quit"),
];

/// Help overlay widget: a centered floating panel with keybinding reference.
pub struct HelpOverlay<'a> {
    pub theme: &'a Theme,
}

impl<'a> HelpOverlay<'a> {
    pub fn new(theme: &'a Theme) -> Self {
        Self { theme }
    }

    fn build_lines(&self) -> Vec<Line<'static>> {
        let theme = self.theme;
        let key_style = Style::default().fg(theme.accent_hot);
        let desc_style = Style::default().fg(theme.text_secondary);
        let sep_style = Style::default().fg(theme.text_muted);

        let mut lines = vec![
            Line::from(Span::styled("  keybindings  ", Style::default().fg(theme.text_primary))),
            Line::default(),
        ];

        for (key, desc) in KEYBINDINGS {
            if key.is_empty() {
                lines.push(Line::default());
                continue;
            }
            lines.push(Line::from(vec![
                Span::styled("  ", sep_style),
                Span::styled(key.to_string(), key_style),
                Span::styled("  —  ", sep_style),
                Span::styled(desc.to_string(), desc_style),
            ]));
        }

        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "  press F1 or Esc to close  ",
            Style::default().fg(theme.text_muted),
        )));

        lines
    }
}

impl<'a> Widget for HelpOverlay<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let theme = self.theme;
        let lines = self.build_lines();

        // Determine overlay dimensions.
        let content_height = lines.len() as u16 + 2; // +2 for top/bottom border
        let content_width: u16 = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.chars().count()).sum::<usize>() as u16)
            .max()
            .unwrap_or(40)
            + 4; // padding

        let height = content_height.min(area.height);
        let width = content_width.min(area.width);

        // Center the overlay.
        let x = area.left() + area.width.saturating_sub(width) / 2;
        let y = area.top() + area.height.saturating_sub(height) / 2;

        let overlay_area = Rect::new(x, y, width, height);

        // Clear the area behind the overlay so it floats cleanly.
        Clear.render(overlay_area, buf);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.surface_focus))
            .title(Span::styled(" help ", Style::default().fg(theme.accent_hot)))
            .title_alignment(Alignment::Center);

        let inner = block.inner(overlay_area);
        block.render(overlay_area, buf);

        for (idx, line) in lines.iter().enumerate() {
            let row_y = inner.top() + idx as u16;
            if row_y >= inner.bottom() {
                break;
            }
            let mut x = inner.left();
            for span in &line.spans {
                for ch in span.content.chars() {
                    if x >= inner.right() {
                        break;
                    }
                    buf[(x, row_y)].set_char(ch).set_style(span.style);
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

    fn all_keys_text() -> String {
        KEYBINDINGS.iter().map(|(k, _)| k.to_string()).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn test_help_contains_quit_binding() {
        let theme = Theme::crimson();
        let overlay = HelpOverlay::new(&theme);
        let lines = overlay.build_lines();
        let all_text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref().to_string()))
            .collect();
        assert!(all_text.contains("Ctrl+C"), "help must mention Ctrl+C");
        assert!(all_text.contains("quit"));
    }

    #[test]
    fn help_keybindings_contain_ctrl_k() {
        assert!(all_keys_text().contains("Ctrl+K"), "KEYBINDINGS must include Ctrl+K");
    }

    #[test]
    fn help_keybindings_contain_ctrl_a() {
        assert!(all_keys_text().contains("Ctrl+A"), "KEYBINDINGS must include Ctrl+A");
    }
}
