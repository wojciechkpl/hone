use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::theme::Theme;

/// Render the top header bar (1 row).
///
/// Left:  `>=>` accent_hot bold + " hone" text_primary bold
/// Right: cwd path + git branch in text_secondary
pub fn render_header(theme: &Theme, cwd: &str, git_branch: Option<&str>) -> Line<'static> {
    let accent = Style::default()
        .fg(theme.accent_hot)
        .bg(theme.surface_raised)
        .add_modifier(Modifier::BOLD);
    let title = Style::default()
        .fg(theme.text_primary)
        .bg(theme.surface_raised)
        .add_modifier(Modifier::BOLD);
    let secondary = Style::default()
        .fg(theme.text_secondary)
        .bg(theme.surface_raised);

    let right_text = match git_branch {
        Some(branch) => format!(" {}  {}", cwd, branch),
        None => format!(" {}", cwd),
    };

    Line::from(vec![
        Span::styled(">=>", accent),
        Span::styled(" hone", title),
        Span::styled(right_text, secondary),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    #[test]
    fn test_header_contains_mark() {
        let theme = Theme::crimson();
        let line = render_header(&theme, "/home/user/project", Some("main"));
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains(">=>"), "header must contain the >=> mark");
    }

    #[test]
    fn test_header_contains_cwd() {
        let theme = Theme::crimson();
        let line = render_header(&theme, "/some/path", None);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("/some/path"));
    }

    #[test]
    fn test_header_with_branch() {
        let theme = Theme::crimson();
        let line = render_header(&theme, "/repo", Some("feature-x"));
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("feature-x"));
    }
}
