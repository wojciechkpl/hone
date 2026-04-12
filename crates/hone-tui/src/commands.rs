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
    /// Force a clarification step before the next agent turn.
    ///
    /// `/clarify <prompt>` — ask clarifying questions about `<prompt>`.
    /// `/clarify` alone — ask the user to provide a prompt, then clarify it.
    Clarify(String),
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
    "theme-preview", "set", "tdd", "review", "clarify",
];

/// Agent persona names available for `@agent-name` inline dispatch.
pub const AGENT_NAMES: &[&str] = &[
    "api-designer",
    "code-reviewer",
    "data-engineer",
    "debugger",
    "documentation-agent",
    "ml-engineer",
    "performance-optimizer",
    "python-expert",
    "researcher",
    "rust-expert",
    "security-auditor",
    "tdd-workflow",
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
            "clarify" => Command::Clarify(arg.to_string()),
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

    /// Return agent names matching a prefix (for `@`-autocomplete).
    pub fn agent_completions(prefix: &str) -> Vec<&'static str> {
        AGENT_NAMES
            .iter()
            .filter(|name| name.starts_with(prefix))
            .copied()
            .collect()
    }
}

/// Parse an `@agent-name` prefix from user input.
///
/// Returns `Some((agent_name, remaining_prompt))` when the input starts with
/// `@` followed by a recognised agent name.  Returns `None` otherwise.
///
/// Examples:
/// - `"@debugger why does test_login fail?"` → `Some(("debugger", "why does test_login fail?"))`
/// - `"@rust-expert"` → `Some(("rust-expert", ""))`
/// - `"hello world"` → `None`
pub fn parse_inline_agent(input: &str) -> Option<(&str, &str)> {
    let trimmed = input.trim();
    let rest = trimmed.strip_prefix('@')?;
    let (name, prompt) = match rest.find(char::is_whitespace) {
        Some(pos) => (&rest[..pos], rest[pos..].trim()),
        None => (rest, ""),
    };
    if name.is_empty() {
        return None;
    }
    Some((name, prompt))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- Command::parse for /clarify ----------------------------------------

    #[test]
    fn parse_clarify_no_arg() {
        let cmd = Command::parse("/clarify").unwrap();
        assert_eq!(cmd, Command::Clarify(String::new()));
    }

    #[test]
    fn parse_clarify_with_prompt() {
        let cmd = Command::parse("/clarify fix the auth module").unwrap();
        assert_eq!(cmd, Command::Clarify("fix the auth module".to_string()));
    }

    #[test]
    fn clarify_in_command_names() {
        assert!(COMMAND_NAMES.contains(&"clarify"));
    }

    // --- parse_inline_agent -------------------------------------------------

    #[test]
    fn parse_agent_with_prompt() {
        let result = parse_inline_agent("@debugger why does test_login fail?");
        assert_eq!(result, Some(("debugger", "why does test_login fail?")));
    }

    #[test]
    fn parse_agent_name_only() {
        let result = parse_inline_agent("@rust-expert");
        assert_eq!(result, Some(("rust-expert", "")));
    }

    #[test]
    fn parse_agent_with_leading_whitespace() {
        let result = parse_inline_agent("  @security-auditor review src/auth.rs");
        assert_eq!(result, Some(("security-auditor", "review src/auth.rs")));
    }

    #[test]
    fn parse_agent_no_at_sign() {
        assert!(parse_inline_agent("hello world").is_none());
    }

    #[test]
    fn parse_agent_bare_at_sign() {
        assert!(parse_inline_agent("@").is_none());
    }

    #[test]
    fn parse_agent_at_then_space() {
        // "@ something" — empty name → None
        assert!(parse_inline_agent("@ something").is_none());
    }

    // --- Command::agent_completions -----------------------------------------

    #[test]
    fn agent_completions_prefix_debug() {
        let matches = Command::agent_completions("debug");
        assert!(matches.contains(&"debugger"));
    }

    #[test]
    fn agent_completions_empty_prefix_returns_all() {
        let matches = Command::agent_completions("");
        assert_eq!(matches.len(), AGENT_NAMES.len());
    }

    #[test]
    fn agent_completions_no_match_returns_empty() {
        let matches = Command::agent_completions("zzz-nonexistent");
        assert!(matches.is_empty());
    }

    // --- AGENT_NAMES list completeness --------------------------------------

    #[test]
    fn agent_names_not_empty() {
        assert!(!AGENT_NAMES.is_empty());
    }

    #[test]
    fn rust_expert_in_agent_names() {
        assert!(AGENT_NAMES.contains(&"rust-expert"));
    }
}
