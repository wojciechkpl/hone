use ratatui::style::Style;
use ratatui::text::Span;

use crate::theme::Theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Keyword,
    String,
    Number,
    Comment,
    Type,
    Function,
    Plain,
}

/// Lex a single line of code and return styled spans.
pub fn lex_line(line: &str, language: &str, theme: &Theme) -> Vec<Span<'static>> {
    match language {
        "rust" | "rs" => lex_rust(line, theme),
        "python" | "py" => lex_python(line, theme),
        _ => lex_generic(line, theme),
    }
}

const RUST_KEYWORDS: &[&str] = &[
    "fn", "let", "pub", "use", "struct", "enum", "impl", "trait", "async", "await", "if", "else",
    "match", "return", "mut", "self", "super", "crate", "mod", "where", "for", "while", "loop",
    "break", "continue", "type", "const", "static", "ref", "move", "unsafe", "extern", "dyn",
    "as", "in", "true", "false",
];

const PYTHON_KEYWORDS: &[&str] = &[
    "def", "class", "if", "else", "elif", "for", "while", "return", "import", "from", "as",
    "with", "try", "except", "raise", "pass", "break", "continue", "lambda", "yield", "async",
    "await", "True", "False", "None", "self", "not", "and", "or", "in", "is",
];

fn make_span(text: String, kind: TokenKind, theme: &Theme) -> Span<'static> {
    let color = match kind {
        TokenKind::Keyword => theme.syntax_keyword,
        TokenKind::String => theme.syntax_string,
        TokenKind::Number => theme.syntax_number,
        TokenKind::Comment => theme.syntax_comment,
        TokenKind::Type => theme.syntax_type,
        TokenKind::Function => theme.syntax_function,
        TokenKind::Plain => theme.text_primary,
    };
    Span::styled(text, Style::default().fg(color))
}

/// Returns true if `c` is a valid identifier character (letter, digit, or underscore).
fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Returns true if the character at `pos` is at a word boundary (pos == 0 or previous char is
/// not an identifier character).
fn at_word_boundary(chars: &[char], pos: usize) -> bool {
    pos == 0 || !is_ident(chars[pos - 1])
}

fn flush_plain(plain_buf: &mut String, spans: &mut Vec<Span<'static>>, theme: &Theme) {
    if !plain_buf.is_empty() {
        spans.push(make_span(std::mem::take(plain_buf), TokenKind::Plain, theme));
    }
}

fn lex_with_config(
    line: &str,
    theme: &Theme,
    keywords: &[&str],
    comment_prefix: &str,
) -> Vec<Span<'static>> {
    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut plain_buf = String::new();
    let mut pos = 0;

    let prefix_chars: Vec<char> = comment_prefix.chars().collect();
    let prefix_len = prefix_chars.len();

    while pos < len {
        // --- Comment detection ---
        let remaining_len = len - pos;
        if remaining_len >= prefix_len && chars[pos..pos + prefix_len] == prefix_chars[..] {
            flush_plain(&mut plain_buf, &mut spans, theme);
            let comment_text: String = chars[pos..].iter().collect();
            spans.push(make_span(comment_text, TokenKind::Comment, theme));
            break;
        }

        // --- String literal (double-quote) ---
        if chars[pos] == '"' {
            flush_plain(&mut plain_buf, &mut spans, theme);
            let mut s = String::new();
            s.push('"');
            pos += 1;
            while pos < len {
                if chars[pos] == '\\' && pos + 1 < len {
                    s.push(chars[pos]);
                    s.push(chars[pos + 1]);
                    pos += 2;
                } else if chars[pos] == '"' {
                    s.push('"');
                    pos += 1;
                    break;
                } else {
                    s.push(chars[pos]);
                    pos += 1;
                }
            }
            spans.push(make_span(s, TokenKind::String, theme));
            continue;
        }

        // --- Number literal ---
        if chars[pos].is_ascii_digit() && at_word_boundary(&chars, pos) {
            flush_plain(&mut plain_buf, &mut spans, theme);
            let mut n = String::new();
            while pos < len && (chars[pos].is_ascii_digit() || chars[pos] == '.') {
                n.push(chars[pos]);
                pos += 1;
            }
            spans.push(make_span(n, TokenKind::Number, theme));
            continue;
        }

        // --- Identifier / keyword / type ---
        if chars[pos].is_alphabetic() || chars[pos] == '_' {
            flush_plain(&mut plain_buf, &mut spans, theme);
            let word_start = pos;
            let mut word = String::new();
            while pos < len && is_ident(chars[pos]) {
                word.push(chars[pos]);
                pos += 1;
            }
            let kind = if keywords.contains(&word.as_str()) {
                TokenKind::Keyword
            } else if at_word_boundary(&chars, word_start)
                && word.starts_with(|c: char| c.is_uppercase())
            {
                TokenKind::Type
            } else {
                TokenKind::Plain
            };
            spans.push(make_span(word, kind, theme));
            continue;
        }

        // --- Everything else is plain ---
        plain_buf.push(chars[pos]);
        pos += 1;
    }

    flush_plain(&mut plain_buf, &mut spans, theme);
    spans
}

fn lex_rust(line: &str, theme: &Theme) -> Vec<Span<'static>> {
    lex_with_config(line, theme, RUST_KEYWORDS, "//")
}

fn lex_python(line: &str, theme: &Theme) -> Vec<Span<'static>> {
    lex_with_config(line, theme, PYTHON_KEYWORDS, "#")
}

fn lex_generic(line: &str, theme: &Theme) -> Vec<Span<'static>> {
    vec![make_span(line.to_owned(), TokenKind::Plain, theme)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn theme() -> Theme {
        Theme::crimson()
    }

    #[test]
    fn test_lex_rust_comment() {
        let t = theme();
        let spans = lex_line("// hello", "rust", &t);
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].style,
            Style::default().fg(t.syntax_comment)
        );
    }

    #[test]
    fn test_lex_rust_keyword_fn() {
        let t = theme();
        let spans = lex_line("fn main()", "rust", &t);
        // First span should be "fn" with keyword color.
        assert_eq!(spans[0].content, "fn");
        assert_eq!(
            spans[0].style,
            Style::default().fg(t.syntax_keyword)
        );
    }

    #[test]
    fn test_lex_rust_string() {
        let t = theme();
        let spans = lex_line(r#"let s = "hello";"#, "rust", &t);
        // Find the span whose content contains the string literal.
        let string_span = spans
            .iter()
            .find(|sp| sp.content.contains("hello"))
            .expect("no string span found");
        assert_eq!(
            string_span.style,
            Style::default().fg(t.syntax_string)
        );
    }

    #[test]
    fn test_lex_python_comment() {
        let t = theme();
        let spans = lex_line("# comment", "python", &t);
        assert_eq!(spans.len(), 1);
        assert_eq!(
            spans[0].style,
            Style::default().fg(t.syntax_comment)
        );
    }

    #[test]
    fn test_lex_python_keyword() {
        let t = theme();
        let spans = lex_line("def foo():", "python", &t);
        assert_eq!(spans[0].content, "def");
        assert_eq!(
            spans[0].style,
            Style::default().fg(t.syntax_keyword)
        );
    }

    #[test]
    fn test_lex_generic_is_plain() {
        let t = theme();
        let spans = lex_line("hello world", "text", &t);
        for span in &spans {
            assert_eq!(
                span.style,
                Style::default().fg(t.text_primary),
                "span {:?} should be plain",
                span.content
            );
        }
    }
}
