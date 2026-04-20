/// Health of the model provider connection.
#[derive(Debug, Clone, PartialEq)]
pub enum ProviderStatus {
    Connected,
    RateLimited,
    Error(String),
}

impl Default for ProviderStatus {
    fn default() -> Self {
        Self::Connected
    }
}

impl ProviderStatus {
    /// Single-character health badge (unicode).
    pub fn badge(&self) -> char {
        match self {
            Self::Connected => '●',
            Self::RateLimited => '◌',
            Self::Error(_) => '✕',
        }
    }

    /// ASCII fallback for NO_COLOR terminals.
    pub fn badge_ascii(&self) -> char {
        match self {
            Self::Connected => '+',
            Self::RateLimited => '~',
            Self::Error(_) => 'X',
        }
    }
}

/// Information displayed in the status bar.
#[derive(Debug, Clone)]
pub struct StatusInfo {
    pub model: String,
    pub mode: String,
    pub session_cost_usd: f64,
    pub token_count: u32,
    pub budget_usd: Option<f64>,
    pub active_tool: Option<String>,
    /// Estimated tokens currently in the context window.
    pub context_tokens: u32,
    /// Maximum context window size for the active model.
    pub context_limit: u32,
    /// Current session name (e.g. "my-fix-session").
    pub session_name: Option<String>,
    /// Task phase label (e.g. "Exploration", "Editing", "Verification").
    pub phase: String,
    /// Number of turns completed this session.
    pub turn_count: u32,
    /// Seconds elapsed since session start (updated externally).
    pub elapsed_secs: u64,
    /// Seconds elapsed since the current turn started (0 when idle).
    pub turn_elapsed_secs: u64,
    /// Health of the model provider.
    pub provider_status: ProviderStatus,
    /// Incremented each render tick (~10fps). Used to rotate status-bar hints
    /// every 40 ticks (≈4 seconds).
    pub hint_tick: usize,
}

impl Default for StatusInfo {
    fn default() -> Self {
        Self {
            model: String::new(),
            mode: "chat".to_string(),
            session_cost_usd: 0.0,
            token_count: 0,
            budget_usd: None,
            active_tool: None,
            context_tokens: 0,
            context_limit: 128_000,
            session_name: None,
            phase: "Exploration".to_string(),
            turn_count: 0,
            elapsed_secs: 0,
            turn_elapsed_secs: 0,
            provider_status: ProviderStatus::Connected,
            hint_tick: 0,
        }
    }
}

impl StatusInfo {
    /// Format the context window usage as "42k / 200k".
    pub fn format_context(&self) -> String {
        fn fmt_k(n: u32) -> String {
            if n >= 1_000_000 {
                format!("{:.0}M", n as f64 / 1_000_000.0)
            } else if n >= 1_000 {
                format!("{:.0}k", n as f64 / 1_000.0)
            } else {
                format!("{n}")
            }
        }
        format!("{} / {}", fmt_k(self.context_tokens), fmt_k(self.context_limit))
    }
}

impl StatusInfo {
    /// Budget consumption as a fraction (0.0 to 1.0). Returns None if no budget.
    pub fn budget_fraction(&self) -> Option<f64> {
        self.budget_usd.map(|b| {
            if b <= 0.0 {
                1.0
            } else {
                (self.session_cost_usd / b).clamp(0.0, 1.0)
            }
        })
    }

    /// Context window usage as a fraction (0.0 to 1.0).
    pub fn context_fraction(&self) -> f64 {
        if self.context_limit == 0 {
            return 0.0;
        }
        (self.context_tokens as f64 / self.context_limit as f64).clamp(0.0, 1.0)
    }

    /// Format token count as human-readable (e.g., "12.4k").
    pub fn format_tokens(&self) -> String {
        if self.token_count >= 1_000_000 {
            format!("{:.1}M", self.token_count as f64 / 1_000_000.0)
        } else if self.token_count >= 1_000 {
            format!("{:.1}k", self.token_count as f64 / 1_000.0)
        } else {
            format!("{}", self.token_count)
        }
    }

    /// Format cost as compact USD string.
    pub fn format_cost(&self) -> String {
        if self.session_cost_usd >= 1.0 {
            format!("${:.2}", self.session_cost_usd)
        } else {
            format!("${:.4}", self.session_cost_usd)
        }
    }

    /// Phase label in short form for narrow layouts: [E], [W], [V], [O].
    pub fn phase_short(&self) -> &str {
        match self.phase.as_str() {
            "Exploration" => "[E]",
            "Editing" => "[W]",
            "Verification" => "[V]",
            "Orchestrating" => "[O]",
            _ => "[?]",
        }
    }

    /// Phase label in long form for wide layouts: [Exploration], etc.
    pub fn phase_long(&self) -> String {
        format!("[{}]", self.phase)
    }

    /// Detect and update `phase` from a tool name heuristic.
    ///
    /// Call this on `ToolCallStart` events so the phase tracks the activity
    /// automatically without requiring the caller to classify tools.
    pub fn update_phase_from_tool(&mut self, tool_name: &str) {
        let name = tool_name.to_ascii_lowercase();
        if name.contains("write") || name.contains("edit") || name.contains("create") {
            self.phase = "Editing".to_string();
        } else if name.contains("bash")
            || name.contains("shell")
            || name.contains("exec")
            || name.contains("run")
            || name.contains("test")
            || name.contains("cargo")
            || name.contains("pytest")
        {
            self.phase = "Verification".to_string();
        } else if name.contains("read")
            || name.contains("grep")
            || name.contains("search")
            || name.contains("find")
            || name.contains("glob")
        {
            // Only transition to Exploration if we haven't started Editing yet.
            if self.phase == "Exploration" {
                self.phase = "Exploration".to_string();
            }
        }
    }

    /// Format elapsed seconds as `H:MM:SS` (hours optional).
    pub fn format_elapsed(&self) -> String {
        let s = self.elapsed_secs;
        let h = s / 3600;
        let m = (s % 3600) / 60;
        let sec = s % 60;
        if h > 0 {
            format!("{h}:{m:02}:{sec:02}")
        } else {
            format!("{m:02}:{sec:02}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_fraction_none() {
        let s = StatusInfo::default();
        assert!(s.budget_fraction().is_none());
    }

    #[test]
    fn test_budget_fraction_half() {
        let s = StatusInfo {
            session_cost_usd: 0.5,
            budget_usd: Some(1.0),
            ..Default::default()
        };
        assert!((s.budget_fraction().unwrap() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_format_tokens() {
        assert_eq!(
            StatusInfo { token_count: 500, ..Default::default() }.format_tokens(),
            "500"
        );
        assert_eq!(
            StatusInfo { token_count: 12400, ..Default::default() }.format_tokens(),
            "12.4k"
        );
        assert_eq!(
            StatusInfo { token_count: 2_500_000, ..Default::default() }.format_tokens(),
            "2.5M"
        );
    }

    #[test]
    fn test_format_cost() {
        assert_eq!(
            StatusInfo { session_cost_usd: 0.0234, ..Default::default() }.format_cost(),
            "$0.0234"
        );
        assert_eq!(
            StatusInfo { session_cost_usd: 5.67, ..Default::default() }.format_cost(),
            "$5.67"
        );
    }

    #[test]
    fn test_context_fraction() {
        let s = StatusInfo {
            context_tokens: 62_000,
            context_limit: 100_000,
            ..Default::default()
        };
        assert!((s.context_fraction() - 0.62).abs() < 1e-6);
    }

    #[test]
    fn test_context_fraction_zero_limit() {
        let s = StatusInfo { context_limit: 0, ..Default::default() };
        assert_eq!(s.context_fraction(), 0.0);
    }

    #[test]
    fn test_phase_short() {
        let mut s = StatusInfo::default();
        s.phase = "Exploration".to_string();
        assert_eq!(s.phase_short(), "[E]");
        s.phase = "Editing".to_string();
        assert_eq!(s.phase_short(), "[W]");
        s.phase = "Verification".to_string();
        assert_eq!(s.phase_short(), "[V]");
        s.phase = "Orchestrating".to_string();
        assert_eq!(s.phase_short(), "[O]");
    }

    #[test]
    fn test_update_phase_from_tool_write() {
        let mut s = StatusInfo::default();
        s.update_phase_from_tool("write_file");
        assert_eq!(s.phase, "Editing");
    }

    #[test]
    fn test_update_phase_from_tool_shell() {
        let mut s = StatusInfo::default();
        s.update_phase_from_tool("bash");
        assert_eq!(s.phase, "Verification");
    }

    #[test]
    fn test_format_elapsed() {
        let mut s = StatusInfo::default();
        s.elapsed_secs = 4212;
        assert_eq!(s.format_elapsed(), "1:10:12");

        s.elapsed_secs = 252;
        assert_eq!(s.format_elapsed(), "04:12");
    }

    #[test]
    fn test_provider_status_badge() {
        assert_eq!(ProviderStatus::Connected.badge(), '●');
        assert_eq!(ProviderStatus::RateLimited.badge(), '◌');
        assert_eq!(ProviderStatus::Error("oops".into()).badge(), '✕');
    }
}
