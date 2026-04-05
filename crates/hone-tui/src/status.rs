/// Information displayed in the status bar.
#[derive(Debug, Clone)]
pub struct StatusInfo {
    pub model: String,
    pub mode: String,
    pub session_cost_usd: f64,
    pub token_count: u32,
}

impl Default for StatusInfo {
    fn default() -> Self {
        Self {
            model: String::new(),
            mode: "chat".to_string(),
            session_cost_usd: 0.0,
            token_count: 0,
        }
    }
}
