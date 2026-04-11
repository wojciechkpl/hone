use ratatui::{
    style::Style,
    text::{Line, Span},
};

use crate::{animation::AnimationState, status::StatusInfo, theme::Theme};

// ---------------------------------------------------------------------------
// Bar rendering
// ---------------------------------------------------------------------------

/// Number of cells in the context/budget block bar.
const BAR_WIDTH: usize = 10;

/// Block-fill characters: ░ (empty) and ▓ (filled).
const BAR_FILLED: char = '▓';
const BAR_EMPTY: char = '░';
const BAR_FILLED_ASCII: char = '#';
const BAR_EMPTY_ASCII: char = '-';

/// Build a block bar as a vec of styled spans.
///
/// `fraction` — 0.0 to 1.0 fill ratio.
/// `width`    — number of cells.
/// `theme`    — for color selection; color thresholds mirror the context window rules:
///              green < 80 %, yellow 80–94 %, red ≥ 95 %.
/// When `theme.no_color` is true, uses ASCII `#` / `-` characters and no color.
pub fn bar_spans(fraction: f64, width: usize, theme: &Theme) -> Vec<Span<'static>> {
    let filled_cells = ((fraction * width as f64).round() as usize).min(width);

    let color = if fraction < 0.80 {
        theme.budget_low
    } else if fraction < 0.95 {
        theme.budget_mid
    } else {
        theme.budget_high
    };

    let style = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(color)
    };

    let (filled_ch, empty_ch) = if theme.no_color {
        (BAR_FILLED_ASCII, BAR_EMPTY_ASCII)
    } else {
        (BAR_FILLED, BAR_EMPTY)
    };

    let mut spans = Vec::with_capacity(width);
    for i in 0..width {
        let ch = if i < filled_cells { filled_ch } else { empty_ch };
        spans.push(Span::styled(ch.to_string(), style));
    }
    spans
}

// ---------------------------------------------------------------------------
// Tool classification
// ---------------------------------------------------------------------------

/// Classify a tool name into an icon character.
pub fn tool_icon(name: &str, no_color: bool) -> char {
    let n = name.to_ascii_lowercase();
    if no_color {
        // ASCII fallback
        if n.contains("read") || n.contains("grep") || n.contains("search")
            || n.contains("find") || n.contains("glob") || n.contains("symbol")
        {
            'R'
        } else if n.contains("write") || n.contains("edit") || n.contains("create")
            || n.contains("multi_edit")
        {
            'W'
        } else if n.contains("bash") || n.contains("shell") || n.contains("exec")
            || n.contains("run") || n.contains("git")
        {
            'X'
        } else {
            '?'
        }
    } else {
        if n.contains("read") || n.contains("grep") || n.contains("search")
            || n.contains("find") || n.contains("glob") || n.contains("symbol")
        {
            '✦'
        } else if n.contains("write") || n.contains("edit") || n.contains("create")
            || n.contains("multi_edit")
        {
            '✎'
        } else if n.contains("bash") || n.contains("shell") || n.contains("exec")
            || n.contains("run") || n.contains("git")
        {
            '❯'
        } else {
            '◆'
        }
    }
}

// ---------------------------------------------------------------------------
// Width breakpoints
// ---------------------------------------------------------------------------

/// Width tier drives which sections are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WidthTier {
    /// < 100 cols: model + context bar + cost only.
    Minimal,
    /// 100–140 cols: + session name + phase + tool.
    Standard,
    /// > 140 cols: full layout with budget bar + elapsed + turn count.
    Full,
}

impl WidthTier {
    pub fn from_width(cols: u16) -> Self {
        if cols < 100 {
            Self::Minimal
        } else if cols <= 140 {
            Self::Standard
        } else {
            Self::Full
        }
    }
}

// ---------------------------------------------------------------------------
// Section builders
// ---------------------------------------------------------------------------

fn sep(theme: &Theme) -> Span<'static> {
    Span::styled(" │ ", Style::default().fg(theme.text_muted))
}

fn muted(s: impl Into<String>, theme: &Theme) -> Span<'static> {
    Span::styled(s.into(), Style::default().fg(theme.text_muted))
}

fn secondary(s: impl Into<String>, theme: &Theme) -> Span<'static> {
    Span::styled(s.into(), Style::default().fg(theme.text_secondary))
}

fn accent(s: impl Into<String>, theme: &Theme) -> Span<'static> {
    Span::styled(s.into(), Style::default().fg(theme.accent_hot))
}

/// Left zone: `model │ session_name │ phase`
fn left_zone(status: &StatusInfo, theme: &Theme, tier: WidthTier) -> Vec<Span<'static>> {
    let mut spans = Vec::new();

    let model_text = if status.model.is_empty() {
        "no model".to_string()
    } else {
        status.model.clone()
    };
    spans.push(secondary(model_text, theme));

    if tier >= WidthTier::Standard {
        if let Some(ref name) = status.session_name {
            // Truncate session name to 14 chars in standard, 20 in full.
            let max_len = if tier == WidthTier::Full { 20 } else { 14 };
            let displayed = if name.len() > max_len {
                format!("{}…", &name[..max_len.saturating_sub(1)])
            } else {
                name.clone()
            };
            spans.push(sep(theme));
            spans.push(muted(displayed, theme));
        }

        let phase_label = if tier == WidthTier::Full {
            status.phase_long()
        } else {
            status.phase_short().to_string()
        };
        spans.push(sep(theme));
        spans.push(muted(phase_label, theme));
    }

    spans
}

/// Center zone: spinner + icon + tool name (accent colored), or empty.
fn center_zone(
    status: &StatusInfo,
    theme: &Theme,
    anim: &AnimationState,
    tier: WidthTier,
) -> Vec<Span<'static>> {
    if tier < WidthTier::Standard {
        return Vec::new();
    }

    let mut spans = Vec::new();

    if let Some(ref tool) = status.active_tool {
        let icon = tool_icon(tool, theme.no_color);
        let spinner = anim.spinner_char();
        let text = format!("{spinner} {icon} {tool}");
        spans.push(sep(theme));
        spans.push(accent(text, theme));
    }

    spans
}

/// Right zone: context bar + cost + budget bar (if set) + elapsed (if full).
fn right_zone(status: &StatusInfo, theme: &Theme, tier: WidthTier) -> Vec<Span<'static>> {
    let mut spans = Vec::new();

    // Context bar.
    let ctx_fraction = status.context_fraction();
    let ctx_pct = (ctx_fraction * 100.0).round() as u32;
    spans.push(sep(theme));
    spans.extend(bar_spans(ctx_fraction, BAR_WIDTH, theme));
    spans.push(muted(format!(" {ctx_pct}%"), theme));

    // Cost.
    spans.push(sep(theme));
    spans.push(secondary(status.format_cost(), theme));

    // Budget bar — shown in Standard+ when a budget is set.
    if tier >= WidthTier::Standard {
        if let Some(fraction) = status.budget_fraction() {
            spans.push(sep(theme));
            spans.extend(bar_spans(fraction, BAR_WIDTH, theme));
        }
    }

    // Full layout extras: elapsed + turn count.
    if tier == WidthTier::Full {
        let elapsed = status.format_elapsed();
        spans.push(sep(theme));
        spans.push(muted(elapsed, theme));

        if status.turn_count > 0 {
            spans.push(sep(theme));
            spans.push(muted(format!("turn {}", status.turn_count), theme));
        }
    }

    spans
}

// ---------------------------------------------------------------------------
// Provider health badge
// ---------------------------------------------------------------------------

fn health_badge(status: &StatusInfo, theme: &Theme) -> Span<'static> {
    use crate::status::ProviderStatus;

    let (ch, color) = match &status.provider_status {
        ProviderStatus::Connected => {
            let ch = if theme.no_color { '+' } else { '●' };
            (ch, theme.budget_low)
        }
        ProviderStatus::RateLimited => {
            let ch = if theme.no_color { '~' } else { '◌' };
            (ch, theme.budget_mid)
        }
        ProviderStatus::Error(msg) => {
            let ch = if theme.no_color { 'X' } else { '✕' };
            // Append a short error hint when space permits.
            let label = if msg.is_empty() {
                ch.to_string()
            } else {
                let truncated = if msg.len() > 20 { &msg[..20] } else { msg };
                format!("{ch} {truncated}")
            };
            return Span::styled(label, Style::default().fg(theme.budget_high));
        }
    };
    let style = if theme.no_color {
        Style::default()
    } else {
        Style::default().fg(color)
    };
    Span::styled(ch.to_string(), style)
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Render the status bar as a single `Line`.
///
/// `terminal_cols` drives the width-adaptive layout (see `WidthTier`).
pub fn render_status_bar(
    status: &StatusInfo,
    theme: &Theme,
    anim: &AnimationState,
    terminal_cols: u16,
) -> Line<'static> {
    let tier = WidthTier::from_width(terminal_cols);

    let mut spans: Vec<Span<'static>> = Vec::new();

    // Leading padding.
    spans.push(muted(" ", theme));

    // Left zone.
    spans.extend(left_zone(status, theme, tier));

    // Center zone (tool activity).
    spans.extend(center_zone(status, theme, anim, tier));

    // Right zone (resource gauges).
    spans.extend(right_zone(status, theme, tier));

    // Provider health badge — always shown.
    spans.push(sep(theme));
    spans.push(health_badge(status, theme));

    // Trailing padding.
    spans.push(muted(" ", theme));

    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{animation::AnimationState, status::{ProviderStatus, StatusInfo}, theme::Theme};

    fn text_of(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    // --- bar_spans ---

    #[test]
    fn bar_spans_all_empty_at_zero() {
        let theme = Theme::crimson();
        let spans = bar_spans(0.0, BAR_WIDTH, &theme);
        assert_eq!(spans.len(), BAR_WIDTH);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.chars().all(|c| c == BAR_EMPTY), "expected all empty cells");
    }

    #[test]
    fn bar_spans_all_filled_at_one() {
        let theme = Theme::crimson();
        let spans = bar_spans(1.0, BAR_WIDTH, &theme);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.chars().all(|c| c == BAR_FILLED), "expected all filled cells");
    }

    #[test]
    fn bar_spans_half_fill() {
        let theme = Theme::crimson();
        let spans = bar_spans(0.5, BAR_WIDTH, &theme);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        let filled = text.chars().filter(|&c| c == BAR_FILLED).count();
        let empty = text.chars().filter(|&c| c == BAR_EMPTY).count();
        // 0.5 × 10 = 5 filled
        assert_eq!(filled, 5);
        assert_eq!(empty, 5);
    }

    #[test]
    fn bar_spans_no_color_uses_ascii() {
        let theme = Theme::minimal();
        assert!(theme.no_color);
        let spans = bar_spans(0.5, BAR_WIDTH, &theme);
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(text.contains(BAR_FILLED_ASCII), "must use '#' in no_color mode");
        assert!(text.contains(BAR_EMPTY_ASCII), "must use '-' in no_color mode");
        assert!(!text.contains(BAR_FILLED), "must not use '▓' in no_color mode");
    }

    #[test]
    fn bar_spans_width_zero_is_empty() {
        let theme = Theme::crimson();
        let spans = bar_spans(0.5, 0, &theme);
        assert!(spans.is_empty());
    }

    // --- tool_icon ---

    #[test]
    fn tool_icon_read_tools() {
        assert_eq!(tool_icon("read_file", false), '✦');
        assert_eq!(tool_icon("grep", false), '✦');
        assert_eq!(tool_icon("glob", false), '✦');
        assert_eq!(tool_icon("symbol_search", false), '✦');
    }

    #[test]
    fn tool_icon_write_tools() {
        assert_eq!(tool_icon("write_file", false), '✎');
        assert_eq!(tool_icon("edit_file", false), '✎');
        assert_eq!(tool_icon("create_file", false), '✎');
        assert_eq!(tool_icon("multi_edit", false), '✎');
    }

    #[test]
    fn tool_icon_shell_tools() {
        assert_eq!(tool_icon("bash", false), '❯');
        assert_eq!(tool_icon("shell", false), '❯');
        assert_eq!(tool_icon("git_commit", false), '❯');
    }

    #[test]
    fn tool_icon_unknown() {
        assert_eq!(tool_icon("something_random", false), '◆');
    }

    #[test]
    fn tool_icon_ascii_fallback() {
        assert_eq!(tool_icon("read_file", true), 'R');
        assert_eq!(tool_icon("write_file", true), 'W');
        assert_eq!(tool_icon("bash", true), 'X');
        assert_eq!(tool_icon("unknown", true), '?');
    }

    // --- WidthTier ---

    #[test]
    fn width_tier_boundaries() {
        assert_eq!(WidthTier::from_width(80), WidthTier::Minimal);
        assert_eq!(WidthTier::from_width(99), WidthTier::Minimal);
        assert_eq!(WidthTier::from_width(100), WidthTier::Standard);
        assert_eq!(WidthTier::from_width(140), WidthTier::Standard);
        assert_eq!(WidthTier::from_width(141), WidthTier::Full);
        assert_eq!(WidthTier::from_width(200), WidthTier::Full);
    }

    // --- render_status_bar ---

    #[test]
    fn render_shows_model_name() {
        let mut status = StatusInfo::default();
        status.model = "claude-opus-4".to_string();
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim, 120);
        let text = text_of(&line);
        assert!(text.contains("claude-opus-4"), "model name must appear");
    }

    #[test]
    fn render_minimal_hides_session_and_phase() {
        let mut status = StatusInfo::default();
        status.session_name = Some("my-session".to_string());
        status.phase = "Editing".to_string();
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        // 80 cols → Minimal tier
        let line = render_status_bar(&status, &theme, &anim, 80);
        let text = text_of(&line);
        assert!(!text.contains("my-session"), "session name hidden in minimal");
        assert!(!text.contains("[W]"), "phase hidden in minimal");
    }

    #[test]
    fn render_standard_shows_session_and_phase() {
        let mut status = StatusInfo::default();
        status.session_name = Some("my-session".to_string());
        status.phase = "Editing".to_string();
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim, 120);
        let text = text_of(&line);
        assert!(text.contains("my-session"), "session name shown at 120 cols");
        assert!(text.contains("[W]"), "short phase shown at 120 cols");
    }

    #[test]
    fn render_full_shows_long_phase_and_elapsed() {
        let mut status = StatusInfo::default();
        status.phase = "Editing".to_string();
        status.elapsed_secs = 90;
        status.turn_count = 3;
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim, 160);
        let text = text_of(&line);
        assert!(text.contains("[Editing]"), "long phase shown at 160 cols");
        assert!(text.contains("01:30"), "elapsed shown at 160 cols");
        assert!(text.contains("turn 3"), "turn count shown at 160 cols");
    }

    #[test]
    fn render_shows_context_bar() {
        let status = StatusInfo {
            context_tokens: 62_000,
            context_limit: 100_000,
            ..Default::default()
        };
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim, 120);
        let text = text_of(&line);
        // 62% context → 6 filled cells
        let filled = text.chars().filter(|&c| c == BAR_FILLED).count();
        assert!(filled >= 6, "expected at least 6 filled cells for 62%");
        assert!(text.contains("62%"), "percentage label must appear");
    }

    #[test]
    fn render_no_color_uses_ascii_bars() {
        let status = StatusInfo {
            context_tokens: 50_000,
            context_limit: 100_000,
            ..Default::default()
        };
        let theme = Theme::minimal();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim, 120);
        let text = text_of(&line);
        assert!(text.contains('#'), "ASCII bar '#' must appear in no_color");
        assert!(!text.contains('▓'), "unicode '▓' must not appear in no_color");
    }

    #[test]
    fn render_active_tool_shows_in_standard() {
        let mut status = StatusInfo::default();
        status.active_tool = Some("read_file".to_string());
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim, 120);
        let text = text_of(&line);
        assert!(text.contains("read_file"), "tool name must appear");
        assert!(text.contains('✦'), "read icon must appear");
    }

    #[test]
    fn render_active_tool_hidden_in_minimal() {
        let mut status = StatusInfo::default();
        status.active_tool = Some("bash".to_string());
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        // 80 cols → Minimal
        let line = render_status_bar(&status, &theme, &anim, 80);
        let text = text_of(&line);
        assert!(!text.contains("bash"), "tool name hidden in minimal layout");
    }

    #[test]
    fn render_provider_connected_badge() {
        let status = StatusInfo::default(); // Connected by default
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim, 120);
        let text = text_of(&line);
        assert!(text.contains('●'), "connected badge must appear");
    }

    #[test]
    fn render_provider_error_badge() {
        let mut status = StatusInfo::default();
        status.provider_status = ProviderStatus::Error("timeout".to_string());
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim, 120);
        let text = text_of(&line);
        assert!(text.contains('✕'), "error badge must appear");
        assert!(text.contains("timeout"), "error message must appear");
    }

    #[test]
    fn render_budget_bar_shown_when_budget_set() {
        let status = StatusInfo {
            session_cost_usd: 0.5,
            budget_usd: Some(1.0),
            context_tokens: 10_000,
            context_limit: 100_000,
            ..Default::default()
        };
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        // Standard tier → budget bar appears
        let line = render_status_bar(&status, &theme, &anim, 120);
        let text = text_of(&line);
        // Two bar segments: context bar (10%) + budget bar (50%)
        let filled_count = text.chars().filter(|&c| c == BAR_FILLED).count();
        // Budget bar alone = 5 filled. Context bar = 1. Total ≥ 6.
        assert!(filled_count >= 6, "budget bar must add filled cells");
    }

    #[test]
    fn render_no_budget_bar_when_budget_none() {
        let status = StatusInfo {
            session_cost_usd: 0.5,
            budget_usd: None,
            context_tokens: 10_000,
            context_limit: 100_000,
            ..Default::default()
        };
        let theme = Theme::crimson();
        let anim = AnimationState::new();
        let line = render_status_bar(&status, &theme, &anim, 120);
        let text = text_of(&line);
        // Only context bar (10%) → 1 filled cell
        let filled_count = text.chars().filter(|&c| c == BAR_FILLED).count();
        assert_eq!(filled_count, 1, "only context bar present when no budget");
    }
}
