use ratatui::style::Color;

/// Full 35-token color system for the TUI.
#[derive(Debug, Clone)]
pub struct Theme {
    // Surfaces
    pub surface_base: Color,
    pub surface_raised: Color,
    pub surface_overlay: Color,
    pub surface_border: Color,
    pub surface_focus: Color,
    // Text
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_muted: Color,
    pub text_on_accent: Color,
    // Roles
    pub role_user: Color,
    pub role_assistant: Color,
    pub role_system: Color,
    pub role_tool: Color,
    pub role_tool_result: Color,
    pub role_error: Color,
    // Accent
    pub accent_hot: Color,
    pub accent_mid: Color,
    pub accent_deep: Color,
    // Syntax
    pub syntax_keyword: Color,
    pub syntax_string: Color,
    pub syntax_number: Color,
    pub syntax_comment: Color,
    pub syntax_type: Color,
    pub syntax_function: Color,
    // Diff
    pub diff_add_bg: Color,
    pub diff_add_fg: Color,
    pub diff_remove_bg: Color,
    pub diff_remove_fg: Color,
    pub diff_hunk_bg: Color,
    pub diff_hunk_fg: Color,
    // Budget
    pub budget_low: Color,
    pub budget_mid: Color,
    pub budget_high: Color,
    // Flags
    pub no_color: bool,
}

impl Theme {
    /// Default dark theme using ANSI-256 indexed colors.
    pub fn crimson() -> Self {
        Self {
            // Surfaces
            surface_base: Color::Indexed(233),
            surface_raised: Color::Indexed(234),
            surface_overlay: Color::Indexed(235),
            surface_border: Color::Indexed(237),
            surface_focus: Color::Indexed(204),
            // Text
            text_primary: Color::Indexed(252),
            text_secondary: Color::Indexed(244),
            text_muted: Color::Indexed(238),
            text_on_accent: Color::White,
            // Roles
            role_user: Color::Indexed(75),
            role_assistant: Color::Indexed(204),
            role_system: Color::Indexed(244),
            role_tool: Color::Indexed(177),
            role_tool_result: Color::Indexed(120),
            role_error: Color::Indexed(203),
            // Accent
            accent_hot: Color::Indexed(204),
            accent_mid: Color::Indexed(197),
            accent_deep: Color::Indexed(88),
            // Syntax
            syntax_keyword: Color::Indexed(204),
            syntax_string: Color::Indexed(120),
            syntax_number: Color::Indexed(221),
            syntax_comment: Color::Indexed(240),
            syntax_type: Color::Indexed(75),
            syntax_function: Color::Indexed(177),
            // Diff
            diff_add_bg: Color::Indexed(22),
            diff_add_fg: Color::Indexed(120),
            diff_remove_bg: Color::Indexed(52),
            diff_remove_fg: Color::Indexed(203),
            diff_hunk_bg: Color::Indexed(24),
            diff_hunk_fg: Color::Indexed(244),
            // Budget
            budget_low: Color::Indexed(120),
            budget_mid: Color::Indexed(221),
            budget_high: Color::Indexed(203),
            // Flags
            no_color: false,
        }
    }

    /// High-contrast dark theme.
    pub fn neon() -> Self {
        let mut t = Self::crimson();
        t.accent_hot = Color::Indexed(197);
        t.surface_border = Color::Indexed(240);
        t
    }

    /// Light theme.
    pub fn chalk() -> Self {
        let mut t = Self::crimson();
        t.surface_base = Color::Indexed(255);
        t.surface_raised = Color::Indexed(254);
        t.surface_overlay = Color::Indexed(253);
        t.surface_border = Color::Indexed(251);
        t.surface_focus = Color::Indexed(160);
        t.text_primary = Color::Indexed(234);
        t.text_secondary = Color::Indexed(242);
        t.text_muted = Color::Indexed(248);
        t.accent_hot = Color::Indexed(160);
        t.accent_mid = Color::Indexed(124);
        t.accent_deep = Color::Indexed(88);
        t.role_user = Color::Indexed(26);
        t.role_assistant = Color::Indexed(160);
        t.role_tool = Color::Indexed(91);
        t.role_tool_result = Color::Indexed(22);
        t.role_error = Color::Indexed(124);
        t
    }

    /// Minimal theme: all colors reset, no_color enabled.
    pub fn minimal() -> Self {
        Self {
            surface_base: Color::Reset,
            surface_raised: Color::Reset,
            surface_overlay: Color::Reset,
            surface_border: Color::Reset,
            surface_focus: Color::Reset,
            text_primary: Color::Reset,
            text_secondary: Color::Reset,
            text_muted: Color::Reset,
            text_on_accent: Color::Reset,
            role_user: Color::Reset,
            role_assistant: Color::Reset,
            role_system: Color::Reset,
            role_tool: Color::Reset,
            role_tool_result: Color::Reset,
            role_error: Color::Reset,
            accent_hot: Color::Reset,
            accent_mid: Color::Reset,
            accent_deep: Color::Reset,
            syntax_keyword: Color::Reset,
            syntax_string: Color::Reset,
            syntax_number: Color::Reset,
            syntax_comment: Color::Reset,
            syntax_type: Color::Reset,
            syntax_function: Color::Reset,
            diff_add_bg: Color::Reset,
            diff_add_fg: Color::Reset,
            diff_remove_bg: Color::Reset,
            diff_remove_fg: Color::Reset,
            diff_hunk_bg: Color::Reset,
            diff_hunk_fg: Color::Reset,
            budget_low: Color::Reset,
            budget_mid: Color::Reset,
            budget_high: Color::Reset,
            no_color: true,
        }
    }

    /// Detect the appropriate theme from environment variables.
    ///
    /// Checks NO_COLOR first, then HONE_THEME, defaulting to crimson.
    pub fn detect() -> Self {
        if std::env::var("NO_COLOR").is_ok() {
            return Self::minimal();
        }
        match std::env::var("HONE_THEME").as_deref() {
            Ok("neon") => Self::neon(),
            Ok("chalk") => Self::chalk(),
            Ok("minimal") => Self::minimal(),
            _ => Self::crimson(),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::crimson()
    }
}
