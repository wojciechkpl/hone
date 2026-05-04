use std::path::{Path, PathBuf};

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
pub enum PermissionError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml: {0}")]
    Toml(String),
    #[error("invalid glob '{pattern}': {reason}")]
    InvalidGlob { pattern: String, reason: String },
}

// ── Decision ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    /// Auto-approve without prompting.
    Allow,
    /// Block entirely; do not even prompt.
    Deny,
    /// Prompt the user (default for unmatched commands).
    Ask,
}

// ── Rule ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    /// Pattern for matching commands. Supports glob (`*`, `?`) or literal
    /// prefix match with word-boundary enforcement.
    pub r#match: String,
    pub decision: Decision,
}

impl Rule {
    /// Return true if `command` matches this rule's pattern.
    ///
    /// Pattern semantics:
    ///   - If the pattern contains `*` or `?`: full glob match via `globset`.
    ///   - Otherwise: literal prefix match with word-boundary enforcement —
    ///     the command must equal the pattern exactly, or start with the
    ///     pattern followed by whitespace.
    pub fn matches(&self, command: &str) -> bool {
        let pattern = &self.r#match;
        if pattern.contains('*') || pattern.contains('?') {
            match globset::Glob::new(pattern) {
                Ok(glob) => glob.compile_matcher().is_match(command),
                Err(_) => false,
            }
        } else {
            command == pattern || command.starts_with(&format!("{} ", pattern))
        }
    }
}

// ── PermissionPolicy ──────────────────────────────────────────────────────────

/// A list of ordered rules that govern whether a shell command should be
/// auto-allowed, denied, or prompted.
///
/// Rule evaluation: the **first** matching rule wins. If no rule matches,
/// the default decision is [`Decision::Ask`].
///
/// # Hierarchical loading precedence
///
/// When loaded via [`PermissionPolicy::load_hierarchical`], user-level rules
/// (`~/.config/hone/permissions.toml`) are placed **before** project-level
/// rules (`.hone/permissions.toml`). Because first-match-wins, a user-level
/// `Allow` for `"cargo test"` takes precedence over a project-level `Deny`
/// for `"cargo *"`. Conversely, a project-level `Deny` for a pattern that
/// the user file does not mention will fire before reaching the default `Ask`.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct PermissionPolicy {
    #[serde(default)]
    pub rules: Vec<Rule>,
}

impl PermissionPolicy {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    /// Evaluate `command` against the rule list.
    /// Returns the decision of the first matching rule, or `Ask` if none match.
    pub fn decide(&self, command: &str) -> Decision {
        self.rules
            .iter()
            .find(|r| r.matches(command))
            .map(|r| r.decision)
            .unwrap_or(Decision::Ask)
    }

    /// Append an `Allow` rule for `command` (exact literal prefix) if one
    /// does not already exist. Intended as a convenience method for the TUI
    /// "always allow" prompt path.
    pub fn approve_always(&mut self, command: &str) {
        let already_exists = self.rules.iter().any(|r| r.r#match == command);
        if !already_exists {
            self.rules.push(Rule { r#match: command.to_string(), decision: Decision::Allow });
        }
    }

    /// Load a `PermissionPolicy` from a TOML file at `path`.
    /// If the file does not exist, returns an empty default policy.
    pub fn load_from_toml(path: &Path) -> Result<Self, PermissionError> {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                toml::from_str(&content).map_err(|e| PermissionError::Toml(e.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(PermissionError::Io(e)),
        }
    }

    /// Persist the policy to `path`, creating parent directories as needed.
    pub fn save_to(&self, path: &Path) -> Result<(), PermissionError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string(self).map_err(|e| PermissionError::Toml(e.to_string()))?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Load the policy by merging the user-level file with the project-level
    /// file. User rules precede project rules (first-match-wins favours user).
    ///
    /// Paths used:
    ///   - user:    `~/.config/hone/permissions.toml`
    ///   - project: `<working_dir>/.hone/permissions.toml`
    pub fn load_hierarchical(working_dir: &Path) -> Self {
        let user_path = dirs_next::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("hone")
            .join("permissions.toml");
        let project_path = working_dir.join(".hone").join("permissions.toml");
        Self::load_hierarchical_with_paths(&user_path, &project_path)
    }

    /// Test seam: load and merge two explicit paths.
    pub fn load_hierarchical_with_paths(user: &Path, project: &Path) -> Self {
        let user_policy = Self::load_from_toml(user).unwrap_or_default();
        let project_policy = Self::load_from_toml(project).unwrap_or_default();
        let mut rules = user_policy.rules;
        rules.extend(project_policy.rules);
        Self { rules }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::TempDir;

    // ── helpers ───────────────────────────────────────────────────────────────

    fn allow_rule(pattern: &str) -> Rule {
        Rule { r#match: pattern.to_owned(), decision: Decision::Allow }
    }

    fn deny_rule(pattern: &str) -> Rule {
        Rule { r#match: pattern.to_owned(), decision: Decision::Deny }
    }

    fn ask_rule(pattern: &str) -> Rule {
        Rule { r#match: pattern.to_owned(), decision: Decision::Ask }
    }

    fn write_toml(dir: &TempDir, filename: &str, content: &str) -> PathBuf {
        let path = dir.path().join(filename);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    // ── Decision serde ────────────────────────────────────────────────────────

    #[test]
    fn decision_serde_roundtrip() {
        let cases = [
            (Decision::Allow, "\"allow\""),
            (Decision::Deny, "\"deny\""),
            (Decision::Ask, "\"ask\""),
        ];
        for (decision, expected_json) in cases {
            let serialized = serde_json::to_string(&decision).unwrap();
            assert_eq!(serialized, expected_json, "serialize {decision:?}");
            let deserialized: Decision = serde_json::from_str(expected_json).unwrap();
            assert_eq!(deserialized, decision, "deserialize {expected_json}");
        }
    }

    // ── Rule matching ─────────────────────────────────────────────────────────

    #[test]
    fn rule_literal_prefix_matches_at_word_boundary() {
        let rule = allow_rule("cargo test");

        // exact match
        assert!(rule.matches("cargo test"), "exact match");
        // prefix + space + more args
        assert!(rule.matches("cargo test --release"), "prefix with flag");
        // must NOT match when next char is not whitespace
        assert!(!rule.matches("cargo testfoo"), "no word boundary");
    }

    #[test]
    fn rule_literal_prefix_does_not_match_unrelated_prefix() {
        let rule = allow_rule("git");

        assert!(rule.matches("git"), "exact match");
        assert!(rule.matches("git status"), "git + space");
        // "github-cli" starts with "git" but next char is 'h', not whitespace
        assert!(!rule.matches("github-cli foo"), "no word boundary on github-cli");
    }

    #[test]
    fn rule_glob_with_star_matches() {
        let rule = allow_rule("cargo *");

        assert!(rule.matches("cargo test"), "cargo test via glob");
        assert!(rule.matches("cargo build --release"), "cargo build via glob");
        assert!(!rule.matches("make test"), "make test does not match cargo *");
    }

    #[test]
    fn rule_glob_with_question_matches_single_char() {
        let rule = allow_rule("git ?");

        assert!(rule.matches("git a"), "single char matches ?");
        assert!(!rule.matches("git ab"), "two chars do not match single ?");
        assert!(!rule.matches("git "), "trailing space only — ? needs a char");
    }

    #[test]
    fn rule_glob_does_not_treat_literal_as_glob() {
        // A pattern without wildcards must be literal-prefix matched, NOT glob-matched.
        // "cargo" (no wildcards) should prefix-match "cargo build" but must not
        // accidentally be treated as a zero-width glob.
        let rule = allow_rule("cargo");
        assert!(rule.matches("cargo build"), "literal prefix cargo matches cargo build");
        assert!(!rule.matches("notcargo"), "literal does not match notcargo");
    }

    #[test]
    fn rule_case_sensitive() {
        let rule = allow_rule("cargo test");
        assert!(!rule.matches("CARGO test"), "upper-case command fails");
        assert!(!rule.matches("Cargo test"), "mixed-case command fails");
    }

    // ── PermissionPolicy::decide ──────────────────────────────────────────────

    #[test]
    fn decide_returns_ask_for_empty_policy() {
        let policy = PermissionPolicy::new();
        assert_eq!(policy.decide("cargo test"), Decision::Ask);
    }

    #[test]
    fn decide_returns_first_matching_rule() {
        let mut policy = PermissionPolicy::new();
        policy.add(allow_rule("cargo test"));
        policy.add(deny_rule("cargo test")); // second rule — should never fire

        assert_eq!(policy.decide("cargo test"), Decision::Allow);
    }

    #[test]
    fn decide_returns_first_match_even_if_later_rule_more_specific() {
        let mut policy = PermissionPolicy::new();
        // broad deny first, then specific allow — broad wins because it's first
        policy.add(deny_rule("cargo *"));
        policy.add(allow_rule("cargo test"));

        assert_eq!(policy.decide("cargo test"), Decision::Deny);
    }

    #[test]
    fn decide_returns_ask_when_no_rule_matches() {
        let mut policy = PermissionPolicy::new();
        policy.add(deny_rule("rm -rf *"));

        assert_eq!(policy.decide("cargo test"), Decision::Ask);
    }

    #[test]
    fn decide_handles_empty_command_string() {
        let mut policy = PermissionPolicy::new();
        policy.add(allow_rule("cargo test"));

        assert_eq!(policy.decide(""), Decision::Ask);
    }

    // ── approve_always ────────────────────────────────────────────────────────

    #[test]
    fn approve_always_appends_an_allow_rule_with_exact_prefix() {
        let mut policy = PermissionPolicy::new();
        policy.approve_always("cargo test");

        assert_eq!(policy.rules.len(), 1);
        assert_eq!(policy.rules[0].r#match, "cargo test");
        assert_eq!(policy.rules[0].decision, Decision::Allow);

        // The rule must fire on a command with trailing arguments.
        assert_eq!(policy.decide("cargo test --release"), Decision::Allow);
    }

    #[test]
    fn approve_always_does_not_duplicate_existing_rule() {
        let mut policy = PermissionPolicy::new();
        policy.approve_always("cargo test");
        policy.approve_always("cargo test");

        assert_eq!(policy.rules.len(), 1, "duplicate approve_always must be idempotent");
    }

    // ── Persistence ───────────────────────────────────────────────────────────

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("permissions.toml");

        let mut policy = PermissionPolicy::new();
        policy.add(allow_rule("cargo test"));
        policy.add(deny_rule("rm -rf *"));

        policy.save_to(&path).unwrap();
        let loaded = PermissionPolicy::load_from_toml(&path).unwrap();

        assert_eq!(loaded.rules.len(), 2);
        assert_eq!(loaded.rules[0].r#match, "cargo test");
        assert_eq!(loaded.rules[0].decision, Decision::Allow);
        assert_eq!(loaded.rules[1].r#match, "rm -rf *");
        assert_eq!(loaded.rules[1].decision, Decision::Deny);
    }

    #[test]
    fn load_missing_file_returns_default_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.toml");

        let policy = PermissionPolicy::load_from_toml(&path).unwrap();
        assert!(policy.rules.is_empty());
    }

    #[test]
    fn load_invalid_toml_returns_err() {
        let dir = TempDir::new().unwrap();
        let path = write_toml(&dir, "bad.toml", "[[rules]\nmatch = !!!");

        let result = PermissionPolicy::load_from_toml(&path);
        assert!(result.is_err(), "invalid TOML must return Err");
        assert!(
            matches!(result.unwrap_err(), PermissionError::Toml(_)),
            "must be a Toml variant"
        );
    }

    #[test]
    fn save_creates_parent_dirs_if_missing() {
        let dir = TempDir::new().unwrap();
        // nested path that does not yet exist
        let path = dir.path().join("nested").join("deeply").join("permissions.toml");

        let policy = PermissionPolicy::new();
        policy.save_to(&path).unwrap();

        assert!(path.exists(), "save_to must create parent directories");
    }

    // ── Hierarchical loading ──────────────────────────────────────────────────

    #[test]
    fn load_hierarchical_with_paths_combines_user_and_project_rules() {
        let dir = TempDir::new().unwrap();
        let user_path = write_toml(
            &dir,
            "user_perms.toml",
            r#"[[rules]]
match = "cargo test"
decision = "allow"
"#,
        );
        let project_path = write_toml(
            &dir,
            "project_perms.toml",
            r#"[[rules]]
match = "make deploy"
decision = "deny"
"#,
        );

        let policy = PermissionPolicy::load_hierarchical_with_paths(&user_path, &project_path);

        assert_eq!(policy.rules.len(), 2);
        // user rule is first
        assert_eq!(policy.rules[0].r#match, "cargo test");
        assert_eq!(policy.rules[0].decision, Decision::Allow);
        // project rule appended after
        assert_eq!(policy.rules[1].r#match, "make deploy");
        assert_eq!(policy.rules[1].decision, Decision::Deny);
    }

    #[test]
    fn load_hierarchical_with_paths_user_rule_wins_when_user_listed_first() {
        // User allows "cargo test" exactly; project denies "cargo *" broadly.
        // Because user rules come first, "cargo test" should resolve to Allow.
        let dir = TempDir::new().unwrap();
        let user_path = write_toml(
            &dir,
            "user_perms.toml",
            r#"[[rules]]
match = "cargo test"
decision = "allow"
"#,
        );
        let project_path = write_toml(
            &dir,
            "project_perms.toml",
            r#"[[rules]]
match = "cargo *"
decision = "deny"
"#,
        );

        let policy = PermissionPolicy::load_hierarchical_with_paths(&user_path, &project_path);

        assert_eq!(
            policy.decide("cargo test"),
            Decision::Allow,
            "user Allow must win over project Deny because user rules precede project rules"
        );
    }

    #[test]
    fn load_hierarchical_with_paths_project_only() {
        let dir = TempDir::new().unwrap();
        let user_path = dir.path().join("nonexistent_user.toml");
        let project_path = write_toml(
            &dir,
            "project_perms.toml",
            r#"[[rules]]
match = "make test"
decision = "allow"
"#,
        );

        let policy = PermissionPolicy::load_hierarchical_with_paths(&user_path, &project_path);

        assert_eq!(policy.rules.len(), 1);
        assert_eq!(policy.rules[0].r#match, "make test");
    }

    #[test]
    fn load_hierarchical_with_paths_missing_files_returns_default() {
        let dir = TempDir::new().unwrap();
        let user_path = dir.path().join("no_user.toml");
        let project_path = dir.path().join("no_project.toml");

        let policy = PermissionPolicy::load_hierarchical_with_paths(&user_path, &project_path);

        assert!(policy.rules.is_empty());
    }
}
