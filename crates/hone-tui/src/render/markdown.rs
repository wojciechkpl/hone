use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

use crate::theme::Theme;

/// Convert a markdown string to a list of styled `Line`s for rendering in the TUI.
///
/// Handles (in order of precedence):
/// 1. Fenced code blocks (``` ... ```)
/// 2. ATX headers (# / ## / ###)
/// 3. Horizontal rules (--- or ***)
/// 4. Bullet list items (- or * prefix)
/// 5. Inline spans: `code`, **bold**, __bold__, *italic*, _italic_
/// 6. Plain text
///
/// All returned `Line`s are `Line<'static>` — all string data is owned.
pub fn markdown_to_lines(text: &str, theme: &Theme) -> Vec<Line<'static>> {
    let mut output: Vec<Line<'static>> = Vec::new();
    let mut in_code_block = false;
    let mut lines_iter = text.lines().peekable();

    while let Some(line) = lines_iter.next() {
        // ── Fenced code block ────────────────────────────────────────────────
        if line.trim_start().starts_with("```") {
            if in_code_block {
                // Closing fence
                in_code_block = false;
                output.push(Line::default());
            } else {
                // Opening fence
                in_code_block = true;
                output.push(Line::default());
            }
            continue;
        }

        if in_code_block {
            let style = if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.text_secondary)
            };
            output.push(Line::from(Span::styled(line.to_string(), style)));
            continue;
        }

        // ── ATX headers ──────────────────────────────────────────────────────
        if let Some(header_text) = parse_header(line) {
            let level = header_level(line);
            let style = if theme.no_color {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            output.push(Line::from(Span::styled(header_text.to_string(), style)));

            // Separator after h1 and h2
            if level <= 2 {
                let sep = "\u{2500}".repeat(60);
                let sep_style = if theme.no_color {
                    Style::default()
                } else {
                    Style::default().fg(theme.text_secondary)
                };
                output.push(Line::from(Span::styled(sep, sep_style)));
            }
            continue;
        }

        // ── Horizontal rule ──────────────────────────────────────────────────
        let trimmed = line.trim();
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            let sep = "\u{2500}".repeat(60);
            let sep_style = if theme.no_color {
                Style::default()
            } else {
                Style::default().fg(theme.text_secondary)
            };
            output.push(Line::from(Span::styled(sep, sep_style)));
            continue;
        }

        // ── Bullet list items ────────────────────────────────────────────────
        if let Some(rest) = strip_bullet_prefix(line) {
            let prefix = if theme.no_color || theme.ascii_only {
                "  - ".to_string()
            } else {
                "  \u{2022} ".to_string()
            };
            let mut spans = vec![Span::raw(prefix)];
            spans.extend(parse_inline_spans(rest, theme));
            output.push(Line::from(spans));
            continue;
        }

        // ── Plain / inline-formatted line ────────────────────────────────────
        let spans = parse_inline_spans(line, theme);
        output.push(Line::from(spans));
    }

    output
}

/// Return the header level (1–6) for an ATX header line, or 0 if not a header.
fn header_level(line: &str) -> usize {
    let mut level = 0;
    for ch in line.chars() {
        if ch == '#' {
            level += 1;
        } else {
            break;
        }
    }
    if level == 0 || level > 6 {
        return 0;
    }
    // The character after the `#` run must be a space (or end of string).
    let after = &line[level..];
    if after.is_empty() || after.starts_with(' ') {
        level
    } else {
        0
    }
}

/// If `line` is an ATX header, return the header text (stripped of `#` prefix and leading space).
fn parse_header(line: &str) -> Option<&str> {
    let level = header_level(line);
    if level == 0 {
        return None;
    }
    let rest = line[level..].trim_start_matches(' ');
    Some(rest)
}

/// Strip a bullet prefix (`- ` or `* `) from the beginning of a line.
/// Returns `Some(rest)` if matched, `None` otherwise.
fn strip_bullet_prefix(line: &str) -> Option<&str> {
    // Allow optional leading whitespace (up to 3 spaces per CommonMark).
    let trimmed = line.trim_start_matches(' ');
    let indent_len = line.len() - trimmed.len();
    if indent_len > 3 {
        return None;
    }
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return Some(rest);
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return Some(rest);
    }
    None
}

/// Parse inline markdown spans from a single line of text.
///
/// Handles (first-match-wins, left to right):
/// - `` `code` `` — accent color + bold
/// - `**bold**` / `__bold__` — bold modifier
/// - `*italic*` / `_italic_` — italic modifier (single delimiter only)
///
/// Returns `Vec<Span<'static>>` with all content owned.
fn parse_inline_spans(line: &str, theme: &Theme) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = line;

    while !remaining.is_empty() {
        // ── Inline code: `...` ───────────────────────────────────────────────
        if let Some(pos) = remaining.find('`') {
            let after_open = &remaining[pos + 1..];
            if let Some(close) = after_open.find('`') {
                // Push text before the backtick
                if pos > 0 {
                    let before = &remaining[..pos];
                    spans.push(plain_span(before, theme));
                }
                let code_text = &after_open[..close];
                let code_style = if theme.no_color {
                    Style::default()
                } else {
                    Style::default()
                        .fg(theme.accent_hot)
                        .add_modifier(Modifier::BOLD)
                };
                spans.push(Span::styled(code_text.to_string(), code_style));
                remaining = &after_open[close + 1..];
                continue;
            }
        }

        // ── Bold: **...** or __...__ ─────────────────────────────────────────
        if let Some((before, inner, after)) = find_delimited(remaining, "**") {
            if !before.is_empty() {
                spans.push(plain_span(before, theme));
            }
            let bold_style = if theme.no_color {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(inner.to_string(), bold_style));
            remaining = after;
            continue;
        }
        if let Some((before, inner, after)) = find_delimited(remaining, "__") {
            if !before.is_empty() {
                spans.push(plain_span(before, theme));
            }
            let bold_style = if theme.no_color {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::BOLD)
            };
            spans.push(Span::styled(inner.to_string(), bold_style));
            remaining = after;
            continue;
        }

        // ── Italic: *...* or _..._ (single delimiter) ────────────────────────
        // Only match single `*`/`_` — must not be followed by another `*`/`_`
        // (those would be bold openers, already handled above).
        if let Some((before, inner, after)) = find_single_delimited(remaining, '*') {
            if !before.is_empty() {
                spans.push(plain_span(before, theme));
            }
            let italic_style = if theme.no_color {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::ITALIC)
            };
            spans.push(Span::styled(inner.to_string(), italic_style));
            remaining = after;
            continue;
        }
        if let Some((before, inner, after)) = find_single_delimited(remaining, '_') {
            if !before.is_empty() {
                spans.push(plain_span(before, theme));
            }
            let italic_style = if theme.no_color {
                Style::default()
            } else {
                Style::default().add_modifier(Modifier::ITALIC)
            };
            spans.push(Span::styled(inner.to_string(), italic_style));
            remaining = after;
            continue;
        }

        // ── No more markers found ─────────────────────────────────────────────
        spans.push(plain_span(remaining, theme));
        break;
    }

    spans
}

/// Build a plain-text span with the theme's primary text color.
fn plain_span(text: &str, theme: &Theme) -> Span<'static> {
    let style = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(theme.text_primary)
    };
    Span::styled(text.to_string(), style)
}

/// Find the first occurrence of `delimiter` in `s`, return `(before, inner, after)`.
/// Skips matches where `inner` is empty.
fn find_delimited<'a>(s: &'a str, delimiter: &str) -> Option<(&'a str, &'a str, &'a str)> {
    let dlen = delimiter.len();
    // Find the opening delimiter
    let open_pos = s.find(delimiter)?;
    let after_open = &s[open_pos + dlen..];
    // Find the closing delimiter (must be after the opening one)
    let close_pos = after_open.find(delimiter)?;
    if close_pos == 0 {
        // Empty inner — skip
        return None;
    }
    let before = &s[..open_pos];
    let inner = &after_open[..close_pos];
    let after = &after_open[close_pos + dlen..];
    Some((before, inner, after))
}

/// Find a single-character delimiter (not doubled) surrounding some text.
///
/// `delimiter` must not appear doubled at the opening position (to avoid
/// consuming bold markers as italic).
fn find_single_delimited(s: &str, delim: char) -> Option<(&str, &str, &str)> {
    let mut chars = s.char_indices();
    while let Some((i, ch)) = chars.next() {
        if ch != delim {
            continue;
        }
        // Ensure this is not a double delimiter (e.g. ** or __)
        let next_char = s[i + delim.len_utf8()..].chars().next();
        if next_char == Some(delim) {
            // Skip both characters (advance past the doubled delimiter)
            chars.next();
            continue;
        }
        // We have a single opening delimiter at position `i`.
        let after_open = &s[i + delim.len_utf8()..];
        if after_open.is_empty() {
            break;
        }
        // Find a closing single delimiter
        let close_pos = find_single_close(after_open, delim)?;
        let before = &s[..i];
        let inner = &after_open[..close_pos];
        let after = &after_open[close_pos + delim.len_utf8()..];
        if inner.is_empty() {
            // empty span — not valid italic
            break;
        }
        return Some((before, inner, after));
    }
    None
}

/// Find the position of a single closing `delim` character in `s`.
/// A "single" close is one NOT followed immediately by another `delim`.
fn find_single_close(s: &str, delim: char) -> Option<usize> {
    let dlen = delim.len_utf8();
    let mut i = 0;
    while i < s.len() {
        let ch = s[i..].chars().next()?;
        if ch == delim {
            let next = s[i + dlen..].chars().next();
            if next != Some(delim) {
                return Some(i);
            }
            // Double delimiter — skip both
            i += dlen * 2;
            continue;
        }
        i += ch.len_utf8();
    }
    None
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;

    fn color_theme() -> Theme {
        Theme::crimson()
    }

    fn no_color_theme() -> Theme {
        Theme::minimal()
    }

    // Helper: collect all span content text from a set of lines.
    fn all_text(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .flat_map(|l| l.spans.iter())
            .map(|s| s.content.as_ref())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn plain_text_unchanged() {
        let theme = color_theme();
        let lines = markdown_to_lines("hello world", &theme);
        assert_eq!(lines.len(), 1);
        assert_eq!(all_text(&lines), "hello world");
        // Style should have the primary text color, no modifiers.
        let span = &lines[0].spans[0];
        assert_eq!(span.style.add_modifier, Modifier::empty());
    }

    #[test]
    fn bold_span_is_bold_modifier() {
        let theme = color_theme();
        let lines = markdown_to_lines("hello **world** end", &theme);
        assert_eq!(lines.len(), 1);
        // Find the bold span
        let bold_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "world")
            .expect("should have a 'world' span");
        assert!(
            bold_span.style.add_modifier.contains(Modifier::BOLD),
            "bold span must have BOLD modifier"
        );
    }

    #[test]
    fn inline_code_gets_accent_color() {
        let theme = color_theme();
        let lines = markdown_to_lines("run `cargo test` now", &theme);
        assert_eq!(lines.len(), 1);
        let code_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "cargo test")
            .expect("should have a 'cargo test' span");
        assert!(
            code_span.style.add_modifier.contains(Modifier::BOLD),
            "inline code span must have BOLD modifier"
        );
        assert_eq!(
            code_span.style.fg,
            Some(theme.accent_hot),
            "inline code span must use accent_hot color"
        );
    }

    #[test]
    fn header_h1_gets_bold_and_separator() {
        let theme = color_theme();
        let lines = markdown_to_lines("# My Title", &theme);
        // Expect: header line + separator line
        assert!(lines.len() >= 2, "h1 should produce at least 2 lines");
        // First line should be bold
        let header_span = &lines[0].spans[0];
        assert!(
            header_span.style.add_modifier.contains(Modifier::BOLD),
            "h1 text must be BOLD"
        );
        assert_eq!(header_span.content.as_ref(), "My Title");
        // Second line should be the separator (─ chars)
        let sep_text = all_text(&lines[1..2]);
        assert!(
            sep_text.contains('\u{2500}'),
            "separator must use ─ character"
        );
    }

    #[test]
    fn header_h2_gets_bold_and_separator() {
        let theme = color_theme();
        let lines = markdown_to_lines("## Section", &theme);
        assert!(lines.len() >= 2);
        let header_span = &lines[0].spans[0];
        assert!(header_span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(header_span.content.as_ref(), "Section");
        let sep_text = all_text(&lines[1..2]);
        assert!(sep_text.contains('\u{2500}'));
    }

    #[test]
    fn header_h3_no_separator() {
        let theme = color_theme();
        let lines = markdown_to_lines("### Sub", &theme);
        // h3 produces only 1 line (no separator)
        assert_eq!(lines.len(), 1);
        let header_span = &lines[0].spans[0];
        assert!(header_span.style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(header_span.content.as_ref(), "Sub");
    }

    #[test]
    fn fenced_code_block_returns_inner_lines() {
        let theme = color_theme();
        let input = "```\nlet x = 1;\nlet y = 2;\n```";
        let lines = markdown_to_lines(input, &theme);
        // Should have: blank, "let x = 1;", "let y = 2;", blank
        let texts: Vec<&str> = lines
            .iter()
            .map(|l| {
                if l.spans.is_empty() {
                    ""
                } else {
                    l.spans[0].content.as_ref()
                }
            })
            .collect();
        assert!(
            texts.contains(&"let x = 1;"),
            "inner line 1 must appear; got: {:?}",
            texts
        );
        assert!(
            texts.contains(&"let y = 2;"),
            "inner line 2 must appear; got: {:?}",
            texts
        );
        // Inner lines should use the secondary (dim) color
        let code_line = lines
            .iter()
            .find(|l| l.spans.first().map(|s| s.content.as_ref()) == Some("let x = 1;"))
            .unwrap();
        assert_eq!(
            code_line.spans[0].style.fg,
            Some(theme.text_secondary),
            "code block lines must use text_secondary color"
        );
    }

    #[test]
    fn bullet_list_item_has_bullet_prefix() {
        let theme = color_theme();
        let lines = markdown_to_lines("- item one", &theme);
        assert_eq!(lines.len(), 1);
        // First span should be the bullet prefix
        let prefix = lines[0].spans[0].content.as_ref();
        assert_eq!(prefix, "  \u{2022} ", "bullet prefix must be '  • '");
    }

    #[test]
    fn bullet_list_item_ascii_mode() {
        let theme = no_color_theme();
        let lines = markdown_to_lines("- item one", &theme);
        assert_eq!(lines.len(), 1);
        let prefix = lines[0].spans[0].content.as_ref();
        assert_eq!(prefix, "  - ", "ascii bullet prefix must be '  - '");
    }

    #[test]
    fn italic_span_is_italic_modifier() {
        let theme = color_theme();
        let lines = markdown_to_lines("this is *emphasized* text", &theme);
        assert_eq!(lines.len(), 1);
        let italic_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "emphasized")
            .expect("should have an 'emphasized' span");
        assert!(
            italic_span.style.add_modifier.contains(Modifier::ITALIC),
            "italic span must have ITALIC modifier"
        );
    }

    #[test]
    fn no_color_mode_no_styling() {
        let theme = no_color_theme();
        let lines = markdown_to_lines("**bold** and `code` and *italic*", &theme);
        assert_eq!(lines.len(), 1);
        for span in &lines[0].spans {
            assert_eq!(
                span.style,
                Style::default(),
                "no_color mode must produce plain Style::default() for all spans"
            );
        }
    }

    #[test]
    fn horizontal_rule_produces_separator_line() {
        let theme = color_theme();
        let lines = markdown_to_lines("---", &theme);
        assert_eq!(lines.len(), 1);
        let text = all_text(&lines);
        assert!(
            text.contains('\u{2500}'),
            "horizontal rule must use ─ character"
        );
    }

    #[test]
    fn double_underscore_bold() {
        let theme = color_theme();
        let lines = markdown_to_lines("__bold text__", &theme);
        assert_eq!(lines.len(), 1);
        let bold_span = lines[0]
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "bold text")
            .expect("should find 'bold text' span");
        assert!(bold_span.style.add_modifier.contains(Modifier::BOLD));
    }
}
