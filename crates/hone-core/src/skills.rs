use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub triggers: Vec<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    pub model: Option<String>,
    #[serde(default)]
    pub always: bool,
    #[serde(skip)]
    pub body: String,
}

impl Skill {
    /// Parse one SKILL.md file. `dir_name` is the parent directory name used as
    /// a fallback when frontmatter `name:` is omitted.
    pub fn from_markdown(dir_name: &str, content: &str) -> Result<Self, SkillError> {
        let after_open = content
            .strip_prefix("---\n")
            .ok_or(SkillError::MissingFrontmatter)?;

        let close = after_open
            .find("\n---\n")
            .ok_or(SkillError::MissingFrontmatter)?;

        let yaml = &after_open[..close];
        let body = after_open[close + "\n---\n".len()..].to_string();

        // Deserialize into a helper that has an optional name field.
        #[derive(serde::Deserialize)]
        struct SkillFrontmatter {
            name: Option<String>,
            description: String,
            #[serde(default)]
            triggers: Vec<String>,
            #[serde(default)]
            tools: Vec<String>,
            model: Option<String>,
            #[serde(default)]
            always: bool,
        }

        let fm: SkillFrontmatter =
            serde_yaml::from_str(yaml).map_err(|e| SkillError::Yaml(e.to_string()))?;

        // Validate all trigger regexes at parse time.
        for trigger in &fm.triggers {
            regex::Regex::new(trigger).map_err(|e| SkillError::InvalidRegex {
                pattern: trigger.clone(),
                reason: e.to_string(),
            })?;
        }

        Ok(Skill {
            name: fm.name.unwrap_or_else(|| dir_name.to_string()),
            description: fm.description,
            triggers: fm.triggers,
            tools: fm.tools,
            model: fm.model,
            always: fm.always,
            body,
        })
    }

    /// Returns true when this skill should be injected for the given prompt.
    /// True when `always` is true OR any trigger regex matches.
    pub fn matches(&self, prompt: &str) -> bool {
        if self.always {
            return true;
        }
        self.triggers
            .iter()
            .any(|t| regex::Regex::new(t).map(|r| r.is_match(prompt)).unwrap_or(false))
    }
}

#[derive(Debug, Default, Clone)]
pub struct SkillRegistry {
    pub skills: Vec<Skill>,
}

impl SkillRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, skill: Skill) {
        self.skills.push(skill);
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Walk subdirectories of `dir`, looking for `<sub>/SKILL.md`. Lenient:
    /// missing dir = no-op; malformed file = warn + skip.
    pub fn load_from_dir(&mut self, dir: &Path) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let skill_file = path.join("SKILL.md");
            if !skill_file.exists() {
                continue;
            }
            let content = match std::fs::read_to_string(&skill_file) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let dir_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");
            match Skill::from_markdown(dir_name, &content) {
                Ok(skill) => self.skills.push(skill),
                Err(_) => {} // lenient — skip malformed
            }
        }
    }

    /// Overlay user dir then project dir; project wins on duplicate `name`.
    pub fn load_hierarchical_with_dirs(user_dir: &Path, project_dir: &Path) -> Self {
        let mut registry = Self::new();
        registry.load_from_dir(user_dir);

        // Load project skills, replacing any user skill with the same name.
        let mut project = Self::new();
        project.load_from_dir(project_dir);
        for skill in project.skills {
            registry.skills.retain(|s| s.name != skill.name);
            registry.skills.push(skill);
        }

        registry
    }

    /// `~/.config/hone/skills/` (user) overlaid with `<project_root>/.hone/skills/` (project).
    pub fn load_hierarchical(working_dir: &Path) -> Self {
        let user_dir = std::env::var_os("HOME")
            .map(PathBuf::from)
            .map(|h| h.join(".config").join("hone").join("skills"))
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));

        let project_dir =
            hone_config::find_project_root(working_dir)
                .map(|r| r.join(".hone").join("skills"))
                .unwrap_or_else(|| PathBuf::from("/nonexistent"));

        Self::load_hierarchical_with_dirs(&user_dir, &project_dir)
    }

    /// All skills whose `matches(prompt)` returns true. Order: registration order.
    pub fn matching(&self, prompt: &str) -> Vec<&Skill> {
        self.skills.iter().filter(|s| s.matches(prompt)).collect()
    }

    /// Render the matched skills as a markdown block suitable for appending to
    /// the system prompt. Empty string when no matches. Format:
    /// "\n\n## Skills\n\n### <name>\n<body>\n\n### <name2>\n<body2>\n"
    pub fn render_for_prompt(&self, prompt: &str) -> String {
        let matched = self.matching(prompt);
        if matched.is_empty() {
            return String::new();
        }

        let mut out = String::from("\n\n## Skills\n\n");
        for skill in matched {
            out.push_str(&format!("### {}\n{}\n\n", skill.name, skill.body));
        }
        out
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("missing frontmatter delimiters")]
    MissingFrontmatter,
    #[error("invalid YAML frontmatter: {0}")]
    Yaml(String),
    #[error("invalid trigger regex '{pattern}': {reason}")]
    InvalidRegex { pattern: String, reason: String },
}

// ---------------------------------------------------------------------------
// Private helper — kept here so tests can reference the path logic without
// requiring HOME to be set to a real directory.
// ---------------------------------------------------------------------------

#[allow(dead_code)]
fn user_skills_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/hone/skills"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::fs;
    use tempfile::TempDir;

    use super::*;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn minimal_skill_md(description: &str) -> String {
        format!(
            "---\ndescription: {description}\n---\n\nSkill body content here.\n"
        )
    }

    fn full_skill_md(name: &str, description: &str, triggers: &[&str], always: bool) -> String {
        let triggers_yaml = if triggers.is_empty() {
            String::new()
        } else {
            let items: String = triggers
                .iter()
                .map(|t| format!("  - \"{t}\"\n"))
                .collect();
            format!("triggers:\n{items}")
        };
        let always_line = format!("always: {always}\n");
        format!(
            "---\nname: {name}\ndescription: {description}\n{triggers_yaml}{always_line}---\n\nPlaybook body here.\n"
        )
    }

    fn write_skill(dir: &TempDir, subdir: &str, content: &str) {
        let skill_dir = dir.path().join(subdir);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), content).unwrap();
    }

    // -----------------------------------------------------------------------
    // Parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn from_markdown_minimal_uses_dir_name_as_fallback() {
        let content = minimal_skill_md("A minimal skill");
        let skill = Skill::from_markdown("my-skill", &content).expect("should parse");
        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.description, "A minimal skill");
    }

    #[test]
    fn from_markdown_explicit_name_overrides_dir_name() {
        let content =
            "---\nname: explicit-name\ndescription: overrides dir\n---\n\nBody.\n".to_string();
        let skill = Skill::from_markdown("dir-name", &content).expect("should parse");
        assert_eq!(skill.name, "explicit-name");
    }

    #[test]
    fn from_markdown_with_triggers_parses_list() {
        let content = full_skill_md("pr-review", "Review PRs", &["review.*PR", "review pull request"], false);
        let skill = Skill::from_markdown("pr-review", &content).expect("should parse");
        assert_eq!(skill.triggers.len(), 2);
        assert!(skill.triggers.contains(&"review.*PR".to_string()));
        assert!(skill.triggers.contains(&"review pull request".to_string()));
    }

    #[test]
    fn from_markdown_default_always_is_false() {
        let content = minimal_skill_md("Test skill");
        let skill = Skill::from_markdown("test", &content).expect("should parse");
        assert!(!skill.always);
    }

    #[test]
    fn from_markdown_invalid_yaml_errors() {
        let content = "---\n: bad: yaml: [\n---\n\nBody.\n";
        let result = Skill::from_markdown("test", content);
        assert!(result.is_err(), "expected error for invalid YAML");
        assert!(
            matches!(result.unwrap_err(), SkillError::Yaml(_)),
            "expected SkillError::Yaml"
        );
    }

    #[test]
    fn from_markdown_missing_frontmatter_errors() {
        let content = "No frontmatter at all, just plain markdown.\n";
        let result = Skill::from_markdown("test", content);
        assert!(result.is_err(), "expected error for missing frontmatter");
        assert!(
            matches!(result.unwrap_err(), SkillError::MissingFrontmatter),
            "expected SkillError::MissingFrontmatter"
        );
    }

    // -----------------------------------------------------------------------
    // Matching tests
    // -----------------------------------------------------------------------

    #[test]
    fn matches_returns_true_when_trigger_regex_matches() {
        let content = full_skill_md("pr-review", "Review PRs", &["review.*PR"], false);
        let skill = Skill::from_markdown("pr-review", &content).expect("should parse");
        assert!(skill.matches("please review my PR"));
    }

    #[test]
    fn matches_returns_false_when_no_trigger_matches() {
        let content = full_skill_md("pr-review", "Review PRs", &["review.*PR"], false);
        let skill = Skill::from_markdown("pr-review", &content).expect("should parse");
        assert!(!skill.matches("deploy the application to production"));
    }

    #[test]
    fn matches_returns_true_when_always_true_regardless_of_triggers() {
        // always: true, no triggers — must still match any prompt
        let content = "---\nname: always-on\ndescription: Always injected\nalways: true\n---\n\nBody.\n";
        let skill = Skill::from_markdown("always-on", content).expect("should parse");
        assert!(skill.matches("any completely unrelated prompt text"));
    }

    #[test]
    fn matches_invalid_regex_skipped_silently_returns_false_unless_always() {
        // A skill with a bad trigger regex and always: false must not panic,
        // and must return false when no valid trigger matches.
        let content =
            "---\nname: bad-regex\ndescription: Has invalid regex\ntriggers:\n  - \"[invalid\"\nalways: false\n---\n\nBody.\n";
        // Either parse succeeds and matches() returns false, or parse fails with
        // SkillError::InvalidRegex — both are acceptable; the test just asserts no panic.
        match Skill::from_markdown("bad-regex", content) {
            Ok(skill) => {
                // Must not panic
                let result = skill.matches("any prompt");
                assert!(!result, "bad regex should not match");
            }
            Err(SkillError::InvalidRegex { pattern: _, reason: _ }) => {
                // Also acceptable — error surfaced at parse time
            }
            Err(e) => {
                panic!("unexpected error variant: {e}");
            }
        }
    }

    // -----------------------------------------------------------------------
    // Loading tests
    // -----------------------------------------------------------------------

    #[test]
    fn load_from_dir_picks_up_skill_md_in_subdirs() {
        let dir = TempDir::new().unwrap();
        write_skill(&dir, "pr-review", &full_skill_md("pr-review", "Review PRs", &[], false));
        write_skill(&dir, "security", &full_skill_md("security", "Security audit", &[], false));

        let mut registry = SkillRegistry::new();
        registry.load_from_dir(dir.path());

        assert_eq!(registry.len(), 2);
    }

    #[test]
    fn load_from_dir_skips_subdirs_without_skill_md() {
        let dir = TempDir::new().unwrap();
        // Subdir with no SKILL.md inside
        fs::create_dir_all(dir.path().join("no-skill-here")).unwrap();
        // One valid skill
        write_skill(&dir, "valid-skill", &full_skill_md("valid-skill", "Valid", &[], false));

        let mut registry = SkillRegistry::new();
        registry.load_from_dir(dir.path());

        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn load_from_dir_skips_malformed_skill_md_silently() {
        let dir = TempDir::new().unwrap();
        // Malformed — no frontmatter
        let skill_dir = dir.path().join("broken");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "not valid frontmatter\n").unwrap();
        // One good skill alongside
        write_skill(&dir, "good-skill", &full_skill_md("good-skill", "Good", &[], false));

        let mut registry = SkillRegistry::new();
        registry.load_from_dir(dir.path()); // must not panic

        assert_eq!(registry.len(), 1, "only the good skill should be loaded");
    }

    #[test]
    fn load_from_dir_missing_dir_returns_empty() {
        let mut registry = SkillRegistry::new();
        registry.load_from_dir(Path::new("/nonexistent/path/to/skills"));
        assert!(registry.is_empty());
    }

    // -----------------------------------------------------------------------
    // Hierarchical loading tests
    // -----------------------------------------------------------------------

    #[test]
    fn load_hierarchical_overlays_project_over_user() {
        let user_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();

        // Same skill name in both; project description differs
        write_skill(
            &user_dir,
            "shared-skill",
            "---\nname: shared-skill\ndescription: User version\n---\n\nUser body.\n",
        );
        write_skill(
            &project_dir,
            "shared-skill",
            "---\nname: shared-skill\ndescription: Project version\n---\n\nProject body.\n",
        );

        let registry = SkillRegistry::load_hierarchical_with_dirs(
            user_dir.path(),
            project_dir.path(),
        );

        // Exactly one skill — project wins
        assert_eq!(registry.len(), 1, "project version should override user version");
        assert_eq!(registry.skills[0].description, "Project version");
    }

    #[test]
    fn load_hierarchical_combined_unique_names() {
        let user_dir = TempDir::new().unwrap();
        let project_dir = TempDir::new().unwrap();

        write_skill(
            &user_dir,
            "skill-a",
            "---\nname: skill-a\ndescription: User skill A\n---\n\nA body.\n",
        );
        write_skill(
            &project_dir,
            "skill-b",
            "---\nname: skill-b\ndescription: Project skill B\n---\n\nB body.\n",
        );

        let registry = SkillRegistry::load_hierarchical_with_dirs(
            user_dir.path(),
            project_dir.path(),
        );

        assert_eq!(registry.len(), 2, "both unique skills should be present");
        let names: Vec<&str> = registry.skills.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"skill-a"));
        assert!(names.contains(&"skill-b"));
    }

    // -----------------------------------------------------------------------
    // Rendering tests
    // -----------------------------------------------------------------------

    #[test]
    fn matching_returns_skills_in_registration_order() {
        let mut registry = SkillRegistry::new();

        let skill_a = Skill {
            name: "skill-a".to_string(),
            description: "First".to_string(),
            triggers: vec![],
            tools: vec![],
            model: None,
            always: true,
            body: "Body A".to_string(),
        };
        let skill_b = Skill {
            name: "skill-b".to_string(),
            description: "Second".to_string(),
            triggers: vec![],
            tools: vec![],
            model: None,
            always: true,
            body: "Body B".to_string(),
        };

        registry.register(skill_a);
        registry.register(skill_b);

        let matched = registry.matching("any prompt");
        assert_eq!(matched.len(), 2);
        assert_eq!(matched[0].name, "skill-a");
        assert_eq!(matched[1].name, "skill-b");
    }

    #[test]
    fn matching_filters_by_trigger() {
        let mut registry = SkillRegistry::new();

        let skill_matching = Skill {
            name: "pr-review".to_string(),
            description: "Reviews PRs".to_string(),
            triggers: vec!["review.*PR".to_string()],
            tools: vec![],
            model: None,
            always: false,
            body: "PR review playbook.".to_string(),
        };
        let skill_not_matching = Skill {
            name: "deploy".to_string(),
            description: "Deploy skill".to_string(),
            triggers: vec!["deploy to production".to_string()],
            tools: vec![],
            model: None,
            always: false,
            body: "Deploy playbook.".to_string(),
        };

        registry.register(skill_matching);
        registry.register(skill_not_matching);

        let matched = registry.matching("please review my PR for merge");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].name, "pr-review");
    }

    #[test]
    fn render_for_prompt_empty_when_nothing_matches() {
        let mut registry = SkillRegistry::new();
        let skill = Skill {
            name: "deploy".to_string(),
            description: "Deploy".to_string(),
            triggers: vec!["deploy to production".to_string()],
            tools: vec![],
            model: None,
            always: false,
            body: "Deploy steps.".to_string(),
        };
        registry.register(skill);

        let rendered = registry.render_for_prompt("write some unit tests");
        assert_eq!(rendered, "", "no matches → empty string");
    }

    #[test]
    fn render_for_prompt_includes_matched_skill_body() {
        let mut registry = SkillRegistry::new();
        let skill = Skill {
            name: "pr-review".to_string(),
            description: "Review PRs".to_string(),
            triggers: vec![],
            tools: vec![],
            model: None,
            always: true,
            body: "Walk the diff hunk-by-hunk.".to_string(),
        };
        registry.register(skill);

        let rendered = registry.render_for_prompt("any prompt");
        assert!(rendered.contains("## Skills"), "must contain '## Skills' header");
        assert!(
            rendered.contains("Walk the diff hunk-by-hunk."),
            "must contain skill body"
        );
    }

    #[test]
    fn render_for_prompt_includes_multiple_matched_skills() {
        let mut registry = SkillRegistry::new();
        for (name, body) in [("skill-a", "Body for A."), ("skill-b", "Body for B.")] {
            registry.register(Skill {
                name: name.to_string(),
                description: "desc".to_string(),
                triggers: vec![],
                tools: vec![],
                model: None,
                always: true,
                body: body.to_string(),
            });
        }

        let rendered = registry.render_for_prompt("anything");
        assert!(rendered.contains("### skill-a"), "must contain skill-a header");
        assert!(rendered.contains("### skill-b"), "must contain skill-b header");
        assert!(rendered.contains("Body for A."));
        assert!(rendered.contains("Body for B."));
    }
}
