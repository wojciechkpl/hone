/// Lightweight session summary used by the session switcher overlay.
/// Populated by main.rs from the session store at startup.
#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub name: String,
    pub updated_at: i64, // Unix seconds
    pub message_count: usize,
}

/// Format a Unix timestamp as a relative time string.
///
/// - < 60s  : "just now"
/// - < 3600s: "N minutes ago"
/// - < 86400s: "N hours ago"
/// - else   : "N days ago"
pub fn format_relative_time(updated_at: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let diff = now.saturating_sub(updated_at);

    if diff < 60 {
        "just now".to_string()
    } else if diff < 3600 {
        let mins = diff / 60;
        format!("{mins} minutes ago")
    } else if diff < 86400 {
        let hours = diff / 3600;
        format!("{hours} hours ago")
    } else {
        let days = diff / 86400;
        format!("{days} days ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn now_secs() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    }

    #[test]
    fn sessions_relative_time_just_now() {
        let ts = now_secs() - 30;
        assert_eq!(format_relative_time(ts), "just now");
    }

    #[test]
    fn sessions_relative_time_minutes_ago() {
        let ts = now_secs() - 5 * 60; // 5 minutes
        assert_eq!(format_relative_time(ts), "5 minutes ago");
    }

    #[test]
    fn sessions_relative_time_hours_ago() {
        let ts = now_secs() - 7200; // 2 hours
        assert_eq!(format_relative_time(ts), "2 hours ago");
    }

    #[test]
    fn sessions_relative_time_days_ago() {
        let ts = now_secs() - 3 * 86400; // 3 days
        assert_eq!(format_relative_time(ts), "3 days ago");
    }
}
