use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{syntax::lex_line, theme::Theme};

/// Render a code block with line numbers and syntax highlighting.
///
/// Returns one `Line` per source line, each prefixed with a line number.
/// A plain-border header shows the language label; a footer closes the box.
pub fn render_code_block(code: &str, language: &str, theme: &Theme) -> Vec<Line<'static>> {
    let border_style = Style::default().fg(theme.surface_border);
    let label_style = Style::default().fg(theme.syntax_type);
    let gutter_style = Style::default().fg(theme.text_muted);

    // Header: plain border with language label.
    let lang_label = format!(" {} ", language);
    let header = Line::from(vec![
        Span::styled("\u{250C}\u{2500}", border_style),
        Span::styled(lang_label, label_style),
        Span::styled("\u{2500}\u{2500}\u{2500}", border_style),
    ]);

    let source_lines: Vec<&str> = code.lines().collect();
    let line_num_width = source_lines.len().max(1).to_string().len();

    let mut lines = vec![header];

    for (idx, src_line) in source_lines.iter().enumerate() {
        let line_num = format!("{:>width$} | ", idx + 1, width = line_num_width);
        let mut spans = vec![Span::styled(line_num, gutter_style)];
        spans.extend(lex_line(src_line, language, theme));
        lines.push(Line::from(spans));
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

    #[test]
    fn test_code_block_has_line_numbers() {
        let theme = Theme::crimson();
        let code = "fn main() {\n    println!(\"hello\");\n}";
        let lines = render_code_block(code, "rust", &theme);
        let all_text: Vec<String> = lines
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
            .collect();
        let has_1 = all_text.iter().any(|l| l.contains("1 |"));
        let has_2 = all_text.iter().any(|l| l.contains("2 |"));
        assert!(has_1, "line 1 marker missing; got: {:?}", all_text);
        assert!(has_2, "line 2 marker missing; got: {:?}", all_text);
    }

    #[test]
    fn test_rust_keywords_highlighted() {
        let theme = Theme::crimson();
        let code = "fn greet() {}";
        let lines = render_code_block(code, "rust", &theme);

        // Find a span that contains exactly "fn" with the keyword color.
        let keyword_color = theme.syntax_keyword;
        let found = lines.iter().any(|l| {
            l.spans.iter().any(|s| {
                s.content.as_ref() == "fn" && s.style.fg == Some(keyword_color)
            })
        });
        assert!(found, "keyword 'fn' must be styled with syntax_keyword color");
    }

    #[test]
    fn test_non_rust_language_no_keyword_highlight() {
        let theme = Theme::crimson();
        // "fn" is not a keyword in the generic lexer.
        let code = "fn greet() {}";
        let lines = render_code_block(code, "text", &theme);
        let keyword_color = theme.syntax_keyword;
        let found = lines.iter().any(|l| {
            l.spans.iter().any(|s| s.style.fg == Some(keyword_color))
        });
        assert!(!found, "non-rust code must not use syntax_keyword color");
    }
}
