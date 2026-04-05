use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::theme::Theme;

/// Render a unified diff as styled lines.
///
/// Lines starting with `+` use diff_add colors, `-` use diff_remove colors,
/// `@@` use diff_hunk colors, and all others are rendered as context text.
pub fn render_diff_block(diff: &str, file_path: &str, theme: &Theme) -> Vec<Line<'static>> {
    let border_style = Style::default().fg(theme.surface_border);
    let label_style = Style::default().fg(theme.text_secondary);

    // Count added and removed lines for the summary header.
    let mut add_count: usize = 0;
    let mut remove_count: usize = 0;
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            add_count += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            remove_count += 1;
        }
    }

    let summary = format!(" {} +{} -{} ", file_path, add_count, remove_count);
    let header = Line::from(vec![
        Span::styled("\u{250C}\u{2500}", border_style),
        Span::styled(summary, label_style),
        Span::styled("\u{2500}\u{2500}\u{2500}", border_style),
    ]);

    let mut lines = vec![header];

    for src_line in diff.lines() {
        let line_out = if src_line.starts_with("+++") || src_line.starts_with("---") {
            // File name header lines — render as label.
            Line::from(Span::styled(src_line.to_string(), label_style))
        } else if src_line.starts_with('+') {
            Line::from(Span::styled(
                src_line.to_string(),
                Style::default()
                    .fg(theme.diff_add_fg)
                    .bg(theme.diff_add_bg),
            ))
        } else if src_line.starts_with('-') {
            Line::from(Span::styled(
                src_line.to_string(),
                Style::default()
                    .fg(theme.diff_remove_fg)
                    .bg(theme.diff_remove_bg),
            ))
        } else if src_line.starts_with("@@") {
            Line::from(Span::styled(
                src_line.to_string(),
                Style::default()
                    .fg(theme.diff_hunk_fg)
                    .bg(theme.diff_hunk_bg),
            ))
        } else {
            // Context line.
            Line::from(Span::styled(
                src_line.to_string(),
                Style::default().fg(theme.text_secondary),
            ))
        };
        lines.push(line_out);
    }

    // Footer.
    lines.push(Line::from(vec![
        Span::styled("\u{2514}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}", border_style),
    ]));

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    const SAMPLE_DIFF: &str = "\
@@ -1,3 +1,4 @@
 context line
-removed line
+added line one
+added line two";

    #[test]
    fn test_diff_counts_lines() {
        let theme = Theme::crimson();
        let lines = render_diff_block(SAMPLE_DIFF, "src/foo.rs", &theme);
        // Header should contain "+2 -1".
        let header_text: String = lines[0].spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            header_text.contains("+2") && header_text.contains("-1"),
            "diff summary must show +2 -1, got: {header_text}"
        );
    }

    #[test]
    fn test_diff_add_lines_use_add_fg() {
        let theme = Theme::crimson();
        let lines = render_diff_block(SAMPLE_DIFF, "src/foo.rs", &theme);
        let add_color = theme.diff_add_fg;
        let found = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.style.fg == Some(add_color) && s.content.starts_with('+'))
        });
        assert!(found, "added lines must use diff_add_fg color");
    }

    #[test]
    fn test_diff_remove_lines_use_remove_fg() {
        let theme = Theme::crimson();
        let lines = render_diff_block(SAMPLE_DIFF, "src/foo.rs", &theme);
        let remove_color = theme.diff_remove_fg;
        let found = lines.iter().any(|l| {
            l.spans
                .iter()
                .any(|s| s.style.fg == Some(remove_color) && s.content.starts_with('-'))
        });
        assert!(found, "removed lines must use diff_remove_fg color");
    }
}
