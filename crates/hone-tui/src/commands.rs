/// Slash-commands the user can type in the input box.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Switch to a different model — argument is the model identifier.
    Model(String),
    Undo,
    Redo,
    Cost,
    Quit,
    Help,
    /// Toggle split-pane layout.
    Split,
    /// Toggle bottom tool output panel.
    Panel,
    /// Switch theme — argument is the theme name.
    ThemeSwitch(String),
    /// Render a demo of all theme colors.
    ThemePreview,
    /// A `/` prefixed word that was not recognised.
    Unknown(String),
}

/// All known command names for autocomplete.
pub const COMMAND_NAMES: &[&str] = &[
    "model", "undo", "redo", "cost", "quit", "q", "help", "split", "panel", "theme",
    "theme-preview",
];

impl Command {
    /// Parse a raw input string into a `Command`.
    ///
    /// Returns `Some(Command)` when the input starts with `/`, otherwise `None`.
    pub fn parse(input: &str) -> Option<Self> {
        let rest = input.strip_prefix('/')?;
        let (keyword, arg) = match rest.split_once(' ') {
            Some((kw, a)) => (kw, a.trim()),
            None => (rest, ""),
        };

        let cmd = match keyword {
            "model" => Command::Model(arg.to_string()),
            "undo" => Command::Undo,
            "redo" => Command::Redo,
            "cost" => Command::Cost,
            "quit" | "q" => Command::Quit,
            "help" => Command::Help,
            "split" => Command::Split,
            "panel" => Command::Panel,
            "theme" => Command::ThemeSwitch(arg.to_string()),
            "theme-preview" => Command::ThemePreview,
            other => Command::Unknown(other.to_string()),
        };
        Some(cmd)
    }

    /// Return command names matching a prefix (for autocomplete).
    pub fn completions(prefix: &str) -> Vec<&'static str> {
        COMMAND_NAMES
            .iter()
            .filter(|name| name.starts_with(prefix))
            .copied()
            .collect()
    }
}
