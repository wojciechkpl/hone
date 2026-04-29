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
    /// When `true`, suppress all spinner animation.  Spinners show a static
    /// glyph (`·`) instead of cycling.  Set via `HONE_REDUCED_MOTION=1`.
    pub reduced_motion: bool,
    /// When `true`, replace all Unicode box-drawing characters with their
    /// plain ASCII equivalents.  Set via `HONE_ASCII_ONLY=1`.
    pub ascii_only: bool,
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
            reduced_motion: false,
            ascii_only: false,
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
            reduced_motion: false,
            ascii_only: false,
        }
    }

    /// Detect the appropriate theme from environment variables.
    ///
    /// Priority:
    /// 1. `NO_COLOR` or `HONE_NO_COLOR` → full minimal/ASCII mode
    /// 2. `HONE_THEME` → named theme base
    /// 3. Default: crimson
    ///
    /// After the base theme is selected, overlay accessibility flags:
    /// - `HONE_REDUCED_MOTION=1` → set `reduced_motion = true`
    /// - `HONE_ASCII_ONLY=1` → set `ascii_only = true`
    pub fn detect() -> Self {
        let no_color = std::env::var("NO_COLOR").is_ok() || std::env::var("HONE_NO_COLOR").is_ok();

        let mut theme = if no_color {
            Self::minimal()
        } else {
            match std::env::var("HONE_THEME").as_deref() {
                Ok("neon") => Self::neon(),
                Ok("chalk") => Self::chalk(),
                Ok("minimal") => Self::minimal(),
                _ => Self::crimson(),
            }
        };

        // Accessibility overlays — readable from env even in color builds.
        if env_flag("HONE_REDUCED_MOTION") {
            theme.reduced_motion = true;
        }
        if env_flag("HONE_ASCII_ONLY") {
            theme.ascii_only = true;
        }

        theme
    }

    /// Apply accessibility flags supplied at runtime (e.g. from CLI args).
    pub fn with_reduced_motion(mut self, enabled: bool) -> Self {
        self.reduced_motion = enabled;
        self
    }

    /// Apply ascii-only flag supplied at runtime.
    pub fn with_ascii_only(mut self, enabled: bool) -> Self {
        self.ascii_only = enabled;
        self
    }

    /// Apply overrides from a flat TOML file (`key = "#rrggbb"` lines).
    ///
    /// Silently does nothing if the file doesn't exist, can't be read, or
    /// contains unparseable lines — only valid hex color overrides are applied.
    pub fn apply_toml_overrides(&mut self, path: &std::path::Path) {
        let Ok(content) = std::fs::read_to_string(path) else {
            return;
        };
        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let Some((key, val)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let val = val.trim().trim_matches('"');
            let Some(color) = parse_hex_color(val) else {
                continue;
            };
            match key {
                "accent" | "accent_hot" => self.accent_hot = color,
                "accent_mid" => self.accent_mid = color,
                "accent_deep" => self.accent_deep = color,
                "text_primary" => self.text_primary = color,
                "text_secondary" => self.text_secondary = color,
                "text_muted" => self.text_muted = color,
                "role_user" => self.role_user = color,
                "role_assistant" => self.role_assistant = color,
                "role_tool_call" | "role_tool" => self.role_tool = color,
                "role_tool_result" => self.role_tool_result = color,
                "role_error" => self.role_error = color,
                "surface" | "surface_base" => self.surface_base = color,
                "surface_raised" => self.surface_raised = color,
                "surface_overlay" => self.surface_overlay = color,
                "surface_focus" => self.surface_focus = color,
                "surface_border" => self.surface_border = color,
                "budget_low" => self.budget_low = color,
                "budget_mid" => self.budget_mid = color,
                "budget_high" => self.budget_high = color,
                _ => {}
            }
        }
    }

    /// Detect the base theme then apply `~/.config/hone/theme.toml` overrides.
    pub fn detect_with_overrides() -> Self {
        let mut theme = Self::detect();
        let config_path = std::env::var("HOME")
            .map(|h| std::path::PathBuf::from(h).join(".config/hone/theme.toml"))
            .unwrap_or_default();
        theme.apply_toml_overrides(&config_path);
        theme
    }

    /// Return the spinner glyph for the current tick.
    ///
    /// When `reduced_motion` is true, always returns a static `·` regardless
    /// of the tick counter.
    pub fn spinner_glyph(&self, tick: u64) -> char {
        if self.reduced_motion || self.no_color {
            '·'
        } else {
            const FRAMES: [char; 8] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧'];
            FRAMES[(tick as usize) % FRAMES.len()]
        }
    }

    /// Return the appropriate box-drawing character set.
    ///
    /// When `ascii_only` or `no_color` is true, returns plain ASCII characters
    /// (`+`, `-`, `|`) instead of Unicode box-drawing (`┌`, `─`, `│`).
    pub fn box_chars(&self) -> BoxChars {
        if self.ascii_only || self.no_color {
            BoxChars::ASCII
        } else {
            BoxChars::UNICODE
        }
    }
}

/// A set of box-drawing characters (unicode or ASCII).
#[derive(Debug, Clone, Copy)]
pub struct BoxChars {
    pub top_left: char,
    pub top_right: char,
    pub bottom_left: char,
    pub bottom_right: char,
    pub horizontal: char,
    pub vertical: char,
    pub tee_right: char,
    pub tee_left: char,
}

impl BoxChars {
    pub const UNICODE: Self = Self {
        top_left: '┌',
        top_right: '┐',
        bottom_left: '└',
        bottom_right: '┘',
        horizontal: '─',
        vertical: '│',
        tee_right: '├',
        tee_left: '┤',
    };

    pub const ASCII: Self = Self {
        top_left: '+',
        top_right: '+',
        bottom_left: '+',
        bottom_right: '+',
        horizontal: '-',
        vertical: '|',
        tee_right: '+',
        tee_left: '+',
    };
}

/// Parse a CSS-style hex color string (`#rrggbb`) into a [`Color::Rgb`].
fn parse_hex_color(s: &str) -> Option<ratatui::style::Color> {
    let s = s.trim().strip_prefix('#')?;
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(ratatui::style::Color::Rgb(r, g, b))
}

/// Return `true` when an environment variable is set to a non-empty value.
fn env_flag(name: &str) -> bool {
    std::env::var(name).map(|v| !v.is_empty()).unwrap_or(false)
}

impl Default for Theme {
    fn default() -> Self {
        Self::crimson()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduced_motion_false_by_default() {
        let t = Theme::crimson();
        assert!(!t.reduced_motion);
    }

    #[test]
    fn ascii_only_false_by_default() {
        let t = Theme::crimson();
        assert!(!t.ascii_only);
    }

    #[test]
    fn spinner_glyph_cycles_when_motion_enabled() {
        let t = Theme::crimson();
        // Different ticks should produce different glyphs.
        let g0 = t.spinner_glyph(0);
        let g1 = t.spinner_glyph(1);
        assert_ne!(g0, g1, "spinner should cycle");
    }

    #[test]
    fn spinner_glyph_static_when_reduced_motion() {
        let t = Theme::crimson().with_reduced_motion(true);
        let g0 = t.spinner_glyph(0);
        let g1 = t.spinner_glyph(99);
        assert_eq!(g0, g1, "reduced_motion should yield same glyph every tick");
        assert_eq!(g0, '·');
    }

    #[test]
    fn spinner_glyph_static_in_no_color_mode() {
        let t = Theme::minimal(); // no_color = true
        let g = t.spinner_glyph(3);
        assert_eq!(g, '·');
    }

    #[test]
    fn box_chars_unicode_by_default() {
        let t = Theme::crimson();
        let bc = t.box_chars();
        assert_eq!(bc.top_left, '┌');
        assert_eq!(bc.horizontal, '─');
    }

    #[test]
    fn box_chars_ascii_when_ascii_only() {
        let t = Theme::crimson().with_ascii_only(true);
        let bc = t.box_chars();
        assert_eq!(bc.top_left, '+');
        assert_eq!(bc.horizontal, '-');
    }

    #[test]
    fn box_chars_ascii_in_no_color_mode() {
        let t = Theme::minimal(); // no_color = true
        let bc = t.box_chars();
        assert_eq!(bc.top_left, '+');
    }

    #[test]
    fn with_reduced_motion_builder() {
        let t = Theme::crimson().with_reduced_motion(true);
        assert!(t.reduced_motion);
        // Base color flags unchanged.
        assert!(!t.no_color);
    }

    #[test]
    fn with_ascii_only_builder() {
        let t = Theme::crimson().with_ascii_only(true);
        assert!(t.ascii_only);
        assert!(!t.no_color);
    }

    #[test]
    fn detect_reads_hone_no_color() {
        // SAFETY: single-threaded test; no other thread reads this var.
        unsafe { std::env::set_var("HONE_NO_COLOR", "1") };
        let t = Theme::detect();
        unsafe { std::env::remove_var("HONE_NO_COLOR") };
        assert!(t.no_color);
    }

    #[test]
    fn detect_reads_hone_reduced_motion() {
        unsafe { std::env::set_var("HONE_REDUCED_MOTION", "1") };
        let t = Theme::detect();
        unsafe { std::env::remove_var("HONE_REDUCED_MOTION") };
        assert!(t.reduced_motion);
    }

    #[test]
    fn detect_reads_hone_ascii_only() {
        unsafe { std::env::set_var("HONE_ASCII_ONLY", "1") };
        let t = Theme::detect();
        unsafe { std::env::remove_var("HONE_ASCII_ONLY") };
        assert!(t.ascii_only);
    }

    #[test]
    fn theme_apply_hex_color_override() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("theme.toml");
        let mut f = std::fs::File::create(&path).expect("create");
        write!(f, "accent = \"#ff0000\"\n").expect("write");
        drop(f);

        let mut t = Theme::crimson();
        t.apply_toml_overrides(&path);
        assert_eq!(t.accent_hot, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn theme_invalid_color_ignored() {
        use std::io::Write;
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("theme.toml");
        let mut f = std::fs::File::create(&path).expect("create");
        write!(f, "accent = \"not-a-color\"\n").expect("write");
        drop(f);

        let base_accent = Theme::crimson().accent_hot;
        let mut t = Theme::crimson();
        t.apply_toml_overrides(&path);
        assert_eq!(t.accent_hot, base_accent);
    }

    #[test]
    fn theme_missing_file_is_noop() {
        let path = std::path::Path::new("/tmp/hone_theme_does_not_exist_xyz.toml");
        let base_accent = Theme::crimson().accent_hot;
        let mut t = Theme::crimson();
        t.apply_toml_overrides(path);
        assert_eq!(t.accent_hot, base_accent);
    }

    #[test]
    fn parse_hex_color_valid() {
        assert_eq!(parse_hex_color("#ff6b6b"), Some(Color::Rgb(255, 107, 107)));
        assert_eq!(parse_hex_color("#000000"), Some(Color::Rgb(0, 0, 0)));
        assert_eq!(parse_hex_color("#ffffff"), Some(Color::Rgb(255, 255, 255)));
    }

    #[test]
    fn parse_hex_color_invalid() {
        assert_eq!(parse_hex_color("ff6b6b"), None, "missing # prefix");
        assert_eq!(parse_hex_color("#ff6b6"), None, "too short");
        assert_eq!(parse_hex_color("#ff6b6b00"), None, "too long");
        assert_eq!(parse_hex_color("#zzzzzz"), None, "invalid hex digits");
        assert_eq!(parse_hex_color(""), None, "empty string");
    }
}
