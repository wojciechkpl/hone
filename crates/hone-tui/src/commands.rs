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
    /// A `/` prefixed word that was not recognised.
    Unknown(String),
}

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
            other => Command::Unknown(other.to_string()),
        };
        Some(cmd)
    }
}
