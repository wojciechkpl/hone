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
    /// Set a per-run budget cap in USD (must be positive).
    Budget(f64),
    /// Toggle split-pane layout.
    Split,
    /// Toggle bottom tool output panel.
    Panel,
    /// Pre-approve a named tool for the next run without pausing.
    Permit(String),
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
    /// In-TUI configuration.
    /// `/configure provider` — change provider
    /// `/configure model <id>` — change model
    /// `/configure` — show current configuration
    Configure(String),
    /// Preview an orchestrated plan without spawning workers.
    ///
    /// `/plan <prompt>` — decompose `<prompt>` and show the plan overlay.
    /// `/plan` alone — open the overlay with an empty prompt.
    Plan(String),
    /// A `/` prefixed word that was not recognised.
    Unknown(String),
    /// A user-defined slash command loaded from a markdown file.
    User {
        name: String,
        args: String,
    },
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
    "model",
    "undo",
    "redo",
    "cost",
    "quit",
    "q",
    "help",
    "budget",
    "split",
    "panel",
    "permit",
    "plan",
    "theme",
    "theme-preview",
    "set",
    "tdd",
    "review",
    "clarify",
    "configure",
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
            "budget" => match arg.parse::<f64>() {
                Ok(usd) if usd > 0.0 && usd.is_finite() => Command::Budget(usd),
                _ => Command::Unknown(format!("budget {arg}")),
            },
            "permit" => {
                if arg.is_empty() {
                    Command::Unknown("permit".to_string())
                } else {
                    Command::Permit(arg.to_string())
                }
            }
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
            "plan" => Command::Plan(arg.to_string()),
            "review" => Command::Review,
            "clarify" => Command::Clarify(arg.to_string()),
            "configure" => Command::Configure(arg.to_string()),
            other => Command::Unknown(other.to_string()),
        };
        Some(cmd)
    }

    /// Like [`Command::parse`] but also checks a [`UserCommandRegistry`] for
    /// unknown slash commands. An `Unknown(n)` that matches a registered user
    /// command is converted to `Command::User { name, args }`.
    pub fn parse_with_user_commands(
        input: &str,
        registry: &crate::user_commands::UserCommandRegistry,
    ) -> Option<Self> {
        let cmd = Self::parse(input)?;
        if let Command::Unknown(ref raw) = cmd {
            // `raw` is just the keyword (the existing `parse` strips the `/`
            // prefix and splits on the first space to extract `keyword`). The
            // args portion never reaches Unknown. Re-parse from the original
            // input to recover the full arg string.
            let rest = input.strip_prefix('/').unwrap_or(input);
            let (keyword, arg) = match rest.split_once(' ') {
                Some((kw, a)) => (kw, a.trim()),
                None => (rest, ""),
            };
            let _ = raw;
            if registry.get(keyword).is_some() {
                return Some(Command::User {
                    name: keyword.to_string(),
                    args: arg.to_string(),
                });
            }
        }
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

    /// Return (name, description) pairs for commands matching a prefix.
    ///
    /// Descriptions are sourced from `PALETTE_COMMANDS` where available; commands
    /// that exist only in `COMMAND_NAMES` (e.g. `set`, `theme-preview`) fall back
    /// to a short inline description.
    pub fn completions_with_descriptions(prefix: &str) -> Vec<(&'static str, &'static str)> {
        use crate::render::command_palette::PALETTE_COMMANDS;

        // Supplemental descriptions for commands not in PALETTE_COMMANDS.
        const EXTRA: &[(&str, &str)] = &[
            ("q", "Exit hone"),
            ("budget", "Set per-run USD budget cap (e.g. /budget 0.50)"),
            (
                "permit",
                "Pre-approve a tool for the next run (e.g. /permit shell)",
            ),
            ("set", "Set option (e.g. vim/novim)"),
            ("theme-preview", "Preview all theme colors"),
            ("panel", "Toggle tool output panel"),
            (
                "plan",
                "Preview an orchestrated plan without spawning workers",
            ),
            ("split", "Toggle split-pane layout"),
            ("undo", "Undo last action"),
            ("redo", "Redo last action"),
        ];

        COMMAND_NAMES
            .iter()
            .filter(|name| name.starts_with(prefix))
            .map(|name| {
                // Look up in PALETTE_COMMANDS first.
                let desc = PALETTE_COMMANDS
                    .iter()
                    .find(|e| e.name == *name)
                    .map(|e| e.description)
                    .or_else(|| EXTRA.iter().find(|(n, _)| *n == *name).map(|(_, d)| *d))
                    .unwrap_or("");
                (*name, desc)
            })
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

    // --- Command::parse for /configure -------------------------------------

    #[test]
    fn parse_configure_no_arg() {
        let cmd = Command::parse("/configure").unwrap();
        assert_eq!(cmd, Command::Configure(String::new()));
    }

    #[test]
    fn parse_configure_model() {
        let cmd = Command::parse("/configure model anthropic:claude-3-5-sonnet").unwrap();
        assert_eq!(
            cmd,
            Command::Configure("model anthropic:claude-3-5-sonnet".to_string())
        );
    }

    #[test]
    fn parse_configure_budget() {
        let cmd = Command::parse("/configure budget 5.00").unwrap();
        assert_eq!(cmd, Command::Configure("budget 5.00".to_string()));
    }

    #[test]
    fn parse_configure_budget_off() {
        let cmd = Command::parse("/configure budget off").unwrap();
        assert_eq!(cmd, Command::Configure("budget off".to_string()));
    }

    #[test]
    fn parse_configure_vim_on() {
        let cmd = Command::parse("/configure vim on").unwrap();
        assert_eq!(cmd, Command::Configure("vim on".to_string()));
    }

    #[test]
    fn parse_configure_vim_off() {
        let cmd = Command::parse("/configure vim off").unwrap();
        assert_eq!(cmd, Command::Configure("vim off".to_string()));
    }

    #[test]
    fn parse_configure_provider() {
        let cmd = Command::parse("/configure provider deepseek").unwrap();
        assert_eq!(cmd, Command::Configure("provider deepseek".to_string()));
    }

    #[test]
    fn parse_configure_help() {
        let cmd = Command::parse("/configure help").unwrap();
        assert_eq!(cmd, Command::Configure("help".to_string()));
    }

    #[test]
    fn parse_configure_unknown_subcommand() {
        let cmd = Command::parse("/configure badsubcmd").unwrap();
        assert_eq!(cmd, Command::Configure("badsubcmd".to_string()));
    }

    #[test]
    fn configure_in_command_names() {
        assert!(COMMAND_NAMES.contains(&"configure"));
    }

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

    // --- parse_with_user_commands -------------------------------------------

    fn make_registry_with(
        name: &str,
        description: &str,
        body: &str,
    ) -> crate::user_commands::UserCommandRegistry {
        let mut r = crate::user_commands::UserCommandRegistry::new();
        r.commands.push(crate::user_commands::UserCommand {
            name: name.to_string(),
            description: description.to_string(),
            args: None,
            model: None,
            agent: None,
            tools: vec![],
            body: body.to_string(),
        });
        r
    }

    #[test]
    fn parse_with_user_commands_routes_registered_name() {
        let registry = make_registry_with("review-pr", "Review PR", "review {{args}}");
        let cmd = Command::parse_with_user_commands("/review-pr 42", &registry).unwrap();
        assert_eq!(
            cmd,
            Command::User {
                name: "review-pr".into(),
                args: "42".into()
            }
        );
    }

    #[test]
    fn parse_with_user_commands_falls_through_for_unknown() {
        let registry = crate::user_commands::UserCommandRegistry::new();
        let cmd = Command::parse_with_user_commands("/not-a-thing", &registry).unwrap();
        assert_eq!(cmd, Command::Unknown("not-a-thing".into()));
    }

    #[test]
    fn parse_with_user_commands_captures_args() {
        let registry = make_registry_with("deploy", "Deploy", "deploy {{args}}");
        let cmd = Command::parse_with_user_commands("/deploy production", &registry).unwrap();
        assert_eq!(
            cmd,
            Command::User {
                name: "deploy".into(),
                args: "production".into()
            }
        );
    }

    // --- completions_with_descriptions coverage ----------------------------

    /// Every entry in `COMMAND_NAMES` must have a non-empty description in
    /// either `PALETTE_COMMANDS` or the `EXTRA` fallback table inside
    /// `completions_with_descriptions`.  This guards against silent dead-ends
    /// when a new command is added to `COMMAND_NAMES` but its description is
    /// forgotten.
    #[test]
    fn every_command_has_a_description() {
        let entries = Command::completions_with_descriptions("");
        for (name, desc) in &entries {
            assert!(
                !desc.is_empty(),
                "command {name} has empty description — add it to PALETTE_COMMANDS \
                 or the EXTRA table in completions_with_descriptions"
            );
        }
        assert_eq!(
            entries.len(),
            COMMAND_NAMES.len(),
            "completions_with_descriptions must return one entry per COMMAND_NAMES"
        );
    }
}
