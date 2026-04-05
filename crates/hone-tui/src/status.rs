/// Information displayed in the status bar.
#[derive(Debug, Clone)]
pub struct StatusInfo {
    pub model: String,
    pub mode: String,
    pub session_cost_usd: f64,
    pub token_count: u32,
    pub budget_usd: Option<f64>,
    pub active_tool: Option<String>,
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
        }
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
}
