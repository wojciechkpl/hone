use std::path::Path;

use serde::Deserialize;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum UserCommandError {
    #[error("missing frontmatter delimiters")]
    MissingFrontmatter,
    #[error("invalid YAML frontmatter: {0}")]
    Yaml(String),
}

// ---------------------------------------------------------------------------
// Internal serde helper
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct Frontmatter {
    #[serde(default)]
    description: String,
    #[serde(default)]
    args: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    tools: Vec<String>,
}

// ---------------------------------------------------------------------------
// UserCommand
// ---------------------------------------------------------------------------

/// One user-defined slash command loaded from a markdown file.
#[derive(Debug, Clone, PartialEq)]
pub struct UserCommand {
    pub name: String,
    pub description: String,
    pub args: Option<String>,
    pub model: Option<String>,
    pub agent: Option<String>,
    pub tools: Vec<String>,
    pub body: String,
}

impl UserCommand {
    /// Parse one markdown file with YAML frontmatter into a `UserCommand`.
    /// `name` is taken from the file stem (caller passes it in).
    pub fn from_markdown(name: &str, content: &str) -> Result<Self, UserCommandError> {
        let after_open = content
            .strip_prefix("---\n")
            .ok_or(UserCommandError::MissingFrontmatter)?;

        let close = after_open
            .find("\n---\n")
            .ok_or(UserCommandError::MissingFrontmatter)?;

        let yaml = &after_open[..close];
        let body = &after_open[close + "\n---\n".len()..];

        let fm: Frontmatter =
            serde_yaml::from_str(yaml).map_err(|e| UserCommandError::Yaml(e.to_string()))?;

        Ok(UserCommand {
            name: name.to_string(),
            description: fm.description,
            args: fm.args,
            model: fm.model,
            agent: fm.agent,
            tools: fm.tools,
            body: body.to_string(),
        })
    }

    /// Render the body with `{{args}}` substituted by the caller's argument string.
    /// If the body contains no `{{args}}` placeholder, the rendered output is
    /// `<body>\n\n<args>` when args is non-empty, else just `<body>`.
    pub fn render(&self, args: &str) -> String {
        const PLACEHOLDER: &str = "{{args}}";
        if self.body.contains(PLACEHOLDER) {
            self.body.replace(PLACEHOLDER, args)
        } else if args.is_empty() {
            self.body.clone()
        } else {
            format!("{}\n\n{}", self.body, args)
        }
    }
}

// ---------------------------------------------------------------------------
// UserCommandRegistry
// ---------------------------------------------------------------------------

/// In-memory registry of user-defined slash commands.
#[derive(Debug, Default, Clone)]
pub struct UserCommandRegistry {
    pub commands: Vec<UserCommand>,
}

impl UserCommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load `*.md` files from a single directory, lenient (missing dir = empty).
    pub fn load_from_dir(&mut self, dir: &Path) {
        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                tracing::warn!(dir = %dir.display(), "failed to open commands directory: {e}");
                return;
            }
        };

        let mut entries: Vec<_> = read_dir.flatten().collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to read command file: {e}");
                    continue;
                }
            };

            match UserCommand::from_markdown(&stem, &content) {
                Ok(cmd) => self.commands.push(cmd),
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to parse command file: {e}");
                }
            }
        }
    }

    /// Build a registry overlaying user-level then project-level dirs.
    /// Same `name` from project dir overrides user dir.
    pub fn load_hierarchical_with_dirs(user_dir: &Path, project_dir: &Path) -> Self {
        let mut registry = Self::new();
        registry.load_from_dir(user_dir);
        // Load project dir — project overrides user by replacing any existing entry.
        let project_before_len = registry.commands.len();
        let _ = project_before_len;

        let read_dir = match std::fs::read_dir(project_dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return registry,
            Err(e) => {
                tracing::warn!(dir = %project_dir.display(), "failed to open commands directory: {e}");
                return registry;
            }
        };

        let mut entries: Vec<_> = read_dir.flatten().collect();
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            let stem = match path.file_stem().and_then(|s| s.to_str()) {
                Some(s) => s.to_string(),
                None => continue,
            };

            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to read command file: {e}");
                    continue;
                }
            };

            match UserCommand::from_markdown(&stem, &content) {
                Ok(cmd) => {
                    // Replace if same name exists, otherwise push.
                    if let Some(existing) = registry.commands.iter_mut().find(|c| c.name == cmd.name) {
                        *existing = cmd;
                    } else {
                        registry.commands.push(cmd);
                    }
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to parse command file: {e}");
                }
            }
        }

        registry
    }

    /// Like `load_hierarchical_with_dirs` but resolves dirs from HOME and
    /// `hone_config::find_project_root`.
    pub fn load_hierarchical(working_dir: &Path) -> Self {
        let user_dir = dirs_home().join(".config").join("hone").join("commands");
        let project_dir = hone_config::find_project_root(working_dir)
            .map(|root| root.join(".hone").join("commands"))
            .unwrap_or_else(|| working_dir.join(".hone").join("commands"));

        Self::load_hierarchical_with_dirs(&user_dir, &project_dir)
    }

    pub fn get(&self, name: &str) -> Option<&UserCommand> {
        self.commands.iter().find(|c| c.name == name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.commands.iter().map(|c| c.name.as_str()).collect()
    }
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/"))
}

// ---------------------------------------------------------------------------
// Tests — RED PHASE
// All tests below must FAIL until the implementation is complete.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn write_file(dir: &TempDir, filename: &str, content: &str) {
        let path = dir.path().join(filename);
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
    }

    fn minimal_md(description: &str, body: &str) -> String {
        format!("---\ndescription: \"{description}\"\n---\n{body}")
    }

    // -----------------------------------------------------------------------
    // Markdown parsing
    // -----------------------------------------------------------------------

    #[test]
    fn from_markdown_minimal_frontmatter() {
        let content = minimal_md("Run tests", "cargo test");
        let cmd = UserCommand::from_markdown("run-tests", &content).unwrap();
        assert_eq!(cmd.name, "run-tests");
        assert_eq!(cmd.description, "Run tests");
        assert_eq!(cmd.body, "cargo test");
        assert!(cmd.tools.is_empty());
        assert!(cmd.model.is_none());
        assert!(cmd.agent.is_none());
        assert!(cmd.args.is_none());
    }

    #[test]
    fn from_markdown_full_frontmatter() {
        let content = indoc(
            r#"---
description: "Review the current PR"
args: "Optional PR number"
model: "claude-3-5-sonnet"
agent: "code-reviewer"
tools:
  - read_file
  - grep
---
Review PR {{args}} carefully.
"#,
        );
        let cmd = UserCommand::from_markdown("review-pr", &content).unwrap();
        assert_eq!(cmd.name, "review-pr");
        assert_eq!(cmd.description, "Review the current PR");
        assert_eq!(cmd.args.as_deref(), Some("Optional PR number"));
        assert_eq!(cmd.model.as_deref(), Some("claude-3-5-sonnet"));
        assert_eq!(cmd.agent.as_deref(), Some("code-reviewer"));
        assert_eq!(cmd.tools, vec!["read_file", "grep"]);
        assert_eq!(cmd.body, "Review PR {{args}} carefully.\n");
    }

    #[test]
    fn from_markdown_missing_frontmatter_errors() {
        let content = "No frontmatter here, just a plain paragraph.";
        let err = UserCommand::from_markdown("plain", content).unwrap_err();
        assert!(
            matches!(err, UserCommandError::MissingFrontmatter),
            "expected MissingFrontmatter, got {err:?}"
        );
    }

    #[test]
    fn from_markdown_invalid_yaml_errors() {
        let content = "---\ndescription: [unclosed array\n---\nbody\n";
        let err = UserCommand::from_markdown("bad", content).unwrap_err();
        assert!(
            matches!(err, UserCommandError::Yaml(_)),
            "expected Yaml error, got {err:?}"
        );
    }

    #[test]
    fn from_markdown_default_tools_is_empty_vec() {
        let content = minimal_md("No tools declared", "do something");
        let cmd = UserCommand::from_markdown("no-tools", &content).unwrap();
        assert_eq!(cmd.tools, Vec::<String>::new());
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    fn make_cmd(body: &str) -> UserCommand {
        UserCommand {
            name: "test".into(),
            description: "Test command".into(),
            args: None,
            model: None,
            agent: None,
            tools: vec![],
            body: body.to_string(),
        }
    }

    #[test]
    fn render_substitutes_args_placeholder() {
        let cmd = make_cmd("Review {{args}}.");
        assert_eq!(cmd.render("PR #123"), "Review PR #123.");
    }

    #[test]
    fn render_with_no_placeholder_appends_args() {
        let cmd = make_cmd("Run all tests.");
        assert_eq!(cmd.render("in parallel"), "Run all tests.\n\nin parallel");
    }

    #[test]
    fn render_with_empty_args_no_placeholder_returns_body_unchanged() {
        let cmd = make_cmd("Status check.");
        assert_eq!(cmd.render(""), "Status check.");
    }

    #[test]
    fn render_with_empty_args_with_placeholder_substitutes_empty() {
        let cmd = make_cmd("Hello {{args}}.");
        assert_eq!(cmd.render(""), "Hello .");
    }

    #[test]
    fn render_replaces_all_occurrences() {
        let cmd = make_cmd("{{args}} and {{args}}.");
        assert_eq!(cmd.render("foo"), "foo and foo.");
    }

    // -----------------------------------------------------------------------
    // Loading from directory
    // -----------------------------------------------------------------------

    #[test]
    fn load_from_dir_picks_up_md_files_only() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "alpha.md", &minimal_md("Alpha", "do alpha"));
        write_file(&dir, "beta.md", &minimal_md("Beta", "do beta"));
        write_file(&dir, "ignored.txt", "not markdown");

        let mut registry = UserCommandRegistry::new();
        registry.load_from_dir(dir.path());
        assert_eq!(registry.commands.len(), 2);
    }

    #[test]
    fn load_from_dir_uses_file_stem_as_name() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "review-pr.md", &minimal_md("Review PR", "review {{args}}"));

        let mut registry = UserCommandRegistry::new();
        registry.load_from_dir(dir.path());
        assert_eq!(registry.commands.len(), 1);
        assert_eq!(registry.commands[0].name, "review-pr");
    }

    #[test]
    fn load_from_dir_skips_malformed_files_silently() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "valid.md", &minimal_md("Valid", "body"));
        write_file(&dir, "broken.md", "no frontmatter at all");

        let mut registry = UserCommandRegistry::new();
        registry.load_from_dir(dir.path());
        assert_eq!(registry.commands.len(), 1);
        assert_eq!(registry.commands[0].name, "valid");
    }

    #[test]
    fn load_from_dir_missing_dir_is_empty() {
        let mut registry = UserCommandRegistry::new();
        registry.load_from_dir(Path::new("/nonexistent/path/that/cannot/exist"));
        assert_eq!(registry.commands.len(), 0);
    }

    // -----------------------------------------------------------------------
    // Hierarchical loading
    // -----------------------------------------------------------------------

    #[test]
    fn load_hierarchical_overlays_project_over_user() {
        let user_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();

        // Same name "deploy" in both — project wins.
        write_file(&user_dir, "deploy.md", &minimal_md("User deploy", "user body"));
        write_file(&project_dir, "deploy.md", &minimal_md("Project deploy", "project body"));

        let registry = UserCommandRegistry::load_hierarchical_with_dirs(
            user_dir.path(),
            project_dir.path(),
        );

        let cmd = registry.get("deploy").expect("deploy must be present");
        assert_eq!(cmd.description, "Project deploy");
    }

    #[test]
    fn load_hierarchical_user_only_when_project_missing() {
        let user_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap().into_path(); // consume, but use a non-existent path
        write_file(
            &TempDir::new().unwrap(), // throwaway — we just need user_dir populated
            "unused.md",
            "",
        );
        write_file(
            &{
                // Re-open user_dir so it stays alive.
                // Simpler: just write inline.
                let tmp = TempDir::new().unwrap();
                write_file(&tmp, "lint.md", &minimal_md("Lint project", "cargo clippy"));
                // We can't return TempDir here; use a separate scope.
                // Actually, let's restructure.
                tmp
            },
            "ignored.md",
            "",
        );

        // Cleaner approach: create fresh dirs.
        let user_dir2 = TempDir::new().unwrap();
        write_file(&user_dir2, "lint.md", &minimal_md("Lint project", "cargo clippy"));
        let nonexistent = Path::new("/tmp/hone_test_nonexistent_project_commands_dir_xyz");

        let registry =
            UserCommandRegistry::load_hierarchical_with_dirs(user_dir2.path(), nonexistent);

        assert!(registry.get("lint").is_some());
    }

    #[test]
    fn load_hierarchical_combined_unique_names() {
        let user_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();

        write_file(&user_dir, "a.md", &minimal_md("Command A", "do a"));
        write_file(&project_dir, "b.md", &minimal_md("Command B", "do b"));

        let registry = UserCommandRegistry::load_hierarchical_with_dirs(
            user_dir.path(),
            project_dir.path(),
        );

        assert!(registry.get("a").is_some(), "user command 'a' must be present");
        assert!(registry.get("b").is_some(), "project command 'b' must be present");
        assert_eq!(registry.commands.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Registry lookup
    // -----------------------------------------------------------------------

    #[test]
    fn registry_get_returns_none_for_unknown() {
        let registry = UserCommandRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn registry_names_returns_all() {
        let dir = TempDir::new().unwrap();
        write_file(&dir, "foo.md", &minimal_md("Foo", "foo body"));
        write_file(&dir, "bar.md", &minimal_md("Bar", "bar body"));

        let mut registry = UserCommandRegistry::new();
        registry.load_from_dir(dir.path());

        let mut names = registry.names();
        names.sort();
        assert_eq!(names, vec!["bar", "foo"]);
    }

    // -----------------------------------------------------------------------
    // Inline helper (avoids an `indoc` dependency)
    // -----------------------------------------------------------------------

    /// Strip a consistent leading indent from a string literal.
    /// We use this instead of the `indoc` crate to avoid adding a dependency.
    fn indoc(s: &str) -> String {
        s.to_string()
    }
}
