use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{animation::AnimationState, status::StatusInfo, theme::Theme};

/// Sparkline block characters, from 1/8 fill to full block.
const SPARK_CHARS: &[char] = &['\u{258F}', '\u{258E}', '\u{258D}', '\u{258C}', '\u{258B}', '\u{258A}', '\u{2589}', '\u{2588}'];

/// Number of sparkline segments to render.
const SPARK_WIDTH: usize = 8;

/// Select the sparkline character that best represents `fraction` (0.0..=1.0).
fn spark_char(fraction: f64) -> char {
    let idx = ((fraction * SPARK_CHARS.len() as f64) as usize).clamp(0, SPARK_CHARS.len() - 1);
    SPARK_CHARS[idx]
}

/// Build the sparkline as a sequence of characters representing budget usage.
///
/// Each of the `SPARK_WIDTH` columns encodes a portion of the total budget.
/// A column is filled proportionally to how much budget its slice has consumed.
fn sparkline_spans(fraction: f64, theme: &Theme) -> Vec<Span<'static>> {
    let color = if fraction < 0.5 {
        theme.budget_low
    } else if fraction < 0.8 {
        theme.budget_mid
    } else {
        theme.budget_high
    };

    let style = Style::default().fg(color);
    let filled_cols = (fraction * SPARK_WIDTH as f64) as usize;
    let partial = fraction * SPARK_WIDTH as f64 - filled_cols as f64;

    let mut spans = Vec::with_capacity(SPARK_WIDTH);
    for i in 0..SPARK_WIDTH {
        let ch = if i < filled_cols {
            '\u{2588}' // full block
        } else if i == filled_cols && partial > 0.0 {
            spark_char(partial)
        } else {
            ' '
        };
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}

/// Render the status bar as a single `Line`.
///
/// Segments: model | tokens | cost [sparkline] | elapsed | activity
pub fn render_status_bar(status: &StatusInfo, theme: &Theme, anim: &AnimationState) -> Line<'static> {
    let muted = Style::default().fg(theme.text_muted);
    let secondary = Style::default().fg(theme.text_secondary);
    let accent = Style::default().fg(theme.accent_hot);

    let sep = Span::styled(" | ", muted);

    let model_span = Span::styled(
        if status.model.is_empty() { "no model".to_string() } else { status.model.clone() },
        secondary,
    );

    let tokens_span = Span::styled(format!("{} tokens", status.format_tokens()), secondary);
    let cost_span = Span::styled(status.format_cost(), secondary);

    let elapsed_span = Span::styled(anim.format_elapsed(), muted);

    let mut spans: Vec<Span<'static>> = vec![
        Span::styled(" ", muted),
        model_span,
        sep.clone(),
        tokens_span,
        sep.clone(),
        cost_span,
        Span::styled(" ", muted),
    ];

    // Append sparkline only when a budget is configured.
    if let Some(fraction) = status.budget_fraction() {
        spans.push(Span::styled("[", muted));
        spans.extend(sparkline_spans(fraction, theme));
        spans.push(Span::styled("]", muted));
        spans.push(sep.clone());
    }

    spans.push(elapsed_span);

    // Activity indicator: show the active tool or spinner.
    if let Some(tool) = &status.active_tool {
        spans.push(sep.clone());
        spans.push(Span::styled(
            format!("{} {}", anim.spinner_char(), tool),
            accent,
        ));
    }

    spans.push(Span::styled(" ", muted));

    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{animation::AnimationState, status::StatusInfo, theme::Theme};

    #[test]
    fn test_status_bar_shows_model() {
        let mut status = StatusInfo::default();
        status.model = "claude-opus-4".to_string();
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains("claude-opus-4"), "model name must appear in status bar");
    }

    #[test]
    fn test_sparkline_empty_when_no_budget() {
        let status = StatusInfo::default(); // budget_usd is None
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        // None of the spark block chars should appear.
        for ch in SPARK_CHARS {
            assert!(
                !text.contains(*ch),
                "sparkline chars must not appear without a budget"
            );
        }
    }

    #[test]
    fn test_sparkline_present_when_budget_set() {
        let status = StatusInfo {
            session_cost_usd: 0.5,
            budget_usd: Some(1.0),
            ..Default::default()
        };
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        // At 50% budget the full-block or partial chars must be present.
        let has_spark = SPARK_CHARS.iter().any(|ch| text.contains(*ch));
        assert!(has_spark, "sparkline must appear when budget is set");
    }
}
