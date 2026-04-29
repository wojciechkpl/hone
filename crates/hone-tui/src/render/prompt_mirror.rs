//! Prompt Mirror overlay — shows the assembled context sent on the next turn.
//!
//! Opened with Ctrl+M; read-only, scrollable with j/k, g/G, Esc to close.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};

use crate::{app::App, message::DisplayRole};

// ---------------------------------------------------------------------------
// Segment type
// ---------------------------------------------------------------------------

/// A single logical section of the assembled prompt context.
pub(crate) struct MirrorSegment {
    label: String,
    content: String,
    tokens: usize,
}

impl MirrorSegment {
    fn new(label: impl Into<String>, content: impl Into<String>, tokens: usize) -> Self {
        Self {
            label: label.into(),
            content: content.into(),
            tokens,
        }
    }
}

// ---------------------------------------------------------------------------
// Build segments
// ---------------------------------------------------------------------------

/// Truncate `s` to at most 80 characters (display-width approximation).
fn preview80(s: &str) -> String {
    let mut chars = s.chars();
    let head: String = chars.by_ref().take(80).collect();
    if chars.next().is_some() {
        format!("{head}…")
    } else {
        head
    }
}

/// Build the ordered list of mirror segments from current app state.
pub(crate) fn build_segments(app: &App) -> Vec<MirrorSegment> {
    let mut segments: Vec<MirrorSegment> = Vec::new();

    // 1. System prompt placeholder
    segments.push(MirrorSegment::new(
        "[System Prompt]",
        "(not shown — set via -s / HONE_SYSTEM_PROMPT)",
        511,
    ));

    // 2. Skeleton injections from last completed turn
    let empty_skeleton = Vec::new();
    let skeletons = app
        .completed_turns
        .last()
        .map(|t| &t.skeleton_injections)
        .unwrap_or(&empty_skeleton);

    for entry in skeletons {
        let content = format!("{} ({})", entry.path, entry.strategy);
        let tokens = entry.tokens as usize;
        segments.push(MirrorSegment::new("[Context Injection]", content, tokens));
    }

    // 3. Memory recalls from last completed turn
    let empty_recalls = Vec::new();
    let recalls = app
        .completed_turns
        .last()
        .map(|t| &t.memory_recalls)
        .unwrap_or(&empty_recalls);

    for recall in recalls {
        let content = recall.snippet.clone();
        let tokens = content.len() / 4;
        segments.push(MirrorSegment::new("[Memory]", content, tokens));
    }

    // 4. Up to the 20 most recent conversation messages
    let messages = &app.messages;
    let start = messages.len().saturating_sub(20);
    for msg in &messages[start..] {
        let label = match &msg.role {
            DisplayRole::User => "[User]",
            DisplayRole::Assistant => "[hone]",
            DisplayRole::ToolCall { .. } => "[Tool Call]",
            DisplayRole::ToolResult => "[Tool Result]",
            DisplayRole::System => "[System]",
            DisplayRole::Error => "[System]",
        };
        let content = preview80(&msg.content);
        let tokens = msg.content.len() / 4;
        segments.push(MirrorSegment::new(label, content, tokens));
    }

    // 5. Pending input (if non-empty)
    if !app.input.buffer.is_empty() {
        let content = preview80(&app.input.buffer);
        let tokens = app.input.buffer.len() / 4;
        segments.push(MirrorSegment::new("[Pending Input]", content, tokens));
    }

    segments
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render the Prompt Mirror full-screen overlay onto `area`.
pub fn render_prompt_mirror(app: &App, frame: &mut Frame, area: Rect) {
    let theme = &app.theme;
    let nc = theme.no_color;

    // Clear the area behind the overlay.
    frame.render_widget(Clear, area);

    let border_style = Style::default().fg(theme.surface_border);
    let title_style = Style::default().fg(theme.accent_hot);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(border_style)
        .title(Span::styled(" Prompt Mirror ", title_style));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 {
        return;
    }

    let segments = build_segments(app);

    // Reserve one line at the bottom for the summary footer.
    let content_height = inner.height.saturating_sub(2) as usize;
    let footer_y = inner.y + inner.height.saturating_sub(1);

    // Build rendered lines
    let label_style = if nc {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.accent_hot)
    };
    let dim_style = Style::default().fg(theme.text_muted);
    let content_style = Style::default().fg(theme.text_primary);
    let pending_style = if nc {
        Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default()
            .fg(theme.accent_hot)
            .add_modifier(Modifier::BOLD)
    };

    let mut lines: Vec<Line<'static>> = Vec::with_capacity(segments.len());
    for seg in &segments {
        let is_pending = seg.label == "[Pending Input]";
        let lbl_style = if is_pending {
            pending_style
        } else {
            label_style
        };
        let line = Line::from(vec![
            Span::raw("  "),
            Span::styled(seg.label.clone(), lbl_style),
            Span::raw("  "),
            Span::styled(format!("(~{} tok)", seg.tokens), dim_style),
            Span::raw("  "),
            Span::styled(seg.content.clone(), content_style),
        ]);
        lines.push(line);
    }

    // Apply scroll
    let scroll = app.mirror_scroll.min(lines.len().saturating_sub(1));
    let visible: Vec<Line<'static>> = lines
        .into_iter()
        .skip(scroll)
        .take(content_height)
        .collect();

    let content_para = Paragraph::new(visible);
    let content_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: content_height as u16,
    };
    frame.render_widget(content_para, content_area);

    // Footer hint line
    let total_tokens: usize = segments.iter().map(|s| s.tokens).sum();
    let context_k = app.status.context_limit / 1000;
    let footer_text = format!(
        "  Total: ~{} segments  ~{} tok estimated  (context window: {}k)    \
         j/k scroll  g/G top/bottom  Esc close",
        segments.len(),
        total_tokens,
        context_k,
    );
    let footer_style = Style::default().fg(theme.text_muted);
    let footer_line = Line::from(Span::styled(footer_text, footer_style));
    let footer_area = Rect {
        x: inner.x,
        y: footer_y,
        width: inner.width,
        height: 1,
    };
    let footer_para = Paragraph::new(footer_line);
    frame.render_widget(footer_para, footer_area);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use crate::app::App;

    use super::build_segments;

    /// App with a non-empty input buffer produces a segment whose label
    /// contains "Pending".
    #[test]
    fn mirror_segments_include_pending_input() {
        let mut app = App::new();
        app.show_splash = false;
        app.input.buffer = "hello world".to_string();
        app.input.cursor_position = app.input.buffer.len();

        let segments = build_segments(&app);
        let last = segments.last().expect("should have at least one segment");
        assert!(
            last.label.contains("Pending"),
            "last segment label should contain 'Pending', got: {}",
            last.label
        );
    }

    /// App with no messages and no input produces at least the system prompt
    /// segment without panicking.
    #[test]
    fn mirror_segments_empty_when_no_messages() {
        let app = App::new();
        let segments = build_segments(&app);
        assert!(
            !segments.is_empty(),
            "segments should not be empty even with no messages"
        );
        assert!(
            segments[0].label.contains("System Prompt"),
            "first segment should be the system prompt placeholder"
        );
    }

    /// Pressing 'g' while mirror_scroll > 0 resets scroll to 0.
    #[test]
    fn mirror_key_g_resets_scroll() {
        let mut app = App::new();
        app.show_splash = false;
        app.mirror_open = true;
        app.mirror_scroll = 5;

        let key = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::NONE);
        app.handle_key_mirror(key);

        assert_eq!(
            app.mirror_scroll, 0,
            "mirror_scroll should be reset to 0 after 'g'"
        );
    }
}
