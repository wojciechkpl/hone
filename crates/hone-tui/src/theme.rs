use ratatui::style::Color;

/// Color palette used throughout the TUI.
#[derive(Debug, Clone)]
pub struct Theme {
    pub user_color: Color,
    pub assistant_color: Color,
    pub system_color: Color,
    pub error_color: Color,
    pub tool_color: Color,
    pub status_bg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            user_color: Color::Cyan,
            assistant_color: Color::Green,
            system_color: Color::Yellow,
            error_color: Color::Red,
            tool_color: Color::Magenta,
            status_bg: Color::DarkGray,
        }
    }
}
