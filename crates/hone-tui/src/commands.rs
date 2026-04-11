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
    /// Enable vim-style keybindings (`:set vim` / `:set novim`).
    SetVim(bool),
    /// Start TDD watch mode with an optional test command.
    /// `:tdd` — use auto-detected command.
    /// `:tdd <cmd>` — use the given command.
    /// `:tdd off` / `:tdd stop` — stop the watcher.
    Tdd(TddSubcommand),
    /// Show a diff of all changes made in the current session.
    Review,
    /// A `/` prefixed word that was not recognised.
    Unknown(String),
}

/// Sub-command for `:tdd`.
#[derive(Debug, Clone, PartialEq)]
pub enum TddSubcommand {
    /// Start watching with an optional explicit test command.
    Start(Option<String>),
    /// Stop the watcher.
    Stop,
}

/// All known command names for autocomplete.
pub const COMMAND_NAMES: &[&str] = &[
    "model", "undo", "redo", "cost", "quit", "q", "help", "split", "panel", "theme",
    "theme-preview", "set", "tdd", "review",
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
            "set" => match arg {
                "vim" => Command::SetVim(true),
                "novim" | "novm" => Command::SetVim(false),
                other => Command::Unknown(format!("set {other}")),
            },
            "tdd" => match arg {
                "off" | "stop" => Command::Tdd(TddSubcommand::Stop),
                "" => Command::Tdd(TddSubcommand::Start(None)),
                cmd => Command::Tdd(TddSubcommand::Start(Some(cmd.to_string()))),
            },
            "review" => Command::Review,
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
