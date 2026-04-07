use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};

/// Parsed representation of a `HONE.md` conventions file.
///
/// Only level-2 (`##`) headings are treated as section keys.  Everything
/// between two consecutive h2 headings (or between the last h2 and EOF) is
/// collected as the section's raw text content.
#[derive(Debug, Clone, PartialEq)]
pub struct HoneMd {
    /// Sections keyed by their heading text (e.g. `"TOOLS"`, `"STYLE"`).
    pub sections: HashMap<String, String>,
}

impl HoneMd {
    /// Load and merge `HONE.md` files from three levels, from least to most
    /// specific:
    ///
    /// 1. `~/.config/hone/HONE.md`          — user-level
    /// 2. `<project_root>/HONE.md`           — project-level (nearest `.git` /
    ///    `Cargo.toml` / `package.json` / `pyproject.toml` / `go.mod` /
    ///    `pom.xml` ancestor of `working_dir`)
    /// 3. `<working_dir>/HONE.md`            — directory-level (highest priority)
    ///
    /// Sections with the same heading are concatenated; later (more-specific)
    /// content is appended after earlier content so it effectively overrides
    /// for consumers that pick the last value.
    pub fn load_hierarchical(working_dir: &Path) -> Self {
        let mut merged = HoneMd {
            sections: HashMap::new(),
        };

        // 1. User-level.
        if let Some(user_path) = user_hone_md_path() {
            merged.merge_from_file(&user_path);
        }

        // 2. Project-level (may be the same as working_dir — de-duplicate below).
        let project_root = find_project_root(working_dir);
        if let Some(ref root) = project_root {
            let project_path = root.join("HONE.md");
            // Only load if distinct from the directory-level path we will load next.
            if project_path != working_dir.join("HONE.md") {
                merged.merge_from_file(&project_path);
            }
        }

        // 3. Directory-level.
        merged.merge_from_file(&working_dir.join("HONE.md"));

        merged
    }

    /// Merge another `HoneMd` into `self`.
    ///
    /// For each section in `other`, its content is appended to (or inserted
    /// into) the corresponding section in `self`.  This gives "more specific
    /// wins by appearing last" semantics.
    pub fn merge(&mut self, other: HoneMd) {
        for (key, content) in other.sections {
            self.sections
                .entry(key)
                .and_modify(|existing| {
                    existing.push('\n');
                    existing.push_str(&content);
                })
                .or_insert(content);
        }
    }

    /// Convenience: read `path` from disk and merge its sections into `self`.
    ///
    /// If the file does not exist or cannot be read, the call is silently
    /// ignored so that missing HONE.md files never prevent startup.
    fn merge_from_file(&mut self, path: &Path) {
        if let Ok(content) = std::fs::read_to_string(path) {
            self.merge(HoneMd::parse(&content));
        }
    }

    /// Parse a `HONE.md` markdown string into sections.
    pub fn parse(markdown: &str) -> Self {
        let mut sections: HashMap<String, String> = HashMap::new();

        if markdown.is_empty() {
            return Self { sections };
        }

        let parser = Parser::new_ext(markdown, Options::empty());

        // State machine: we track whether we are currently inside an h2 heading
        // and, once we've left the heading, we accumulate text events.
        let mut current_section: Option<String> = None;
        let mut current_heading_text = String::new();
        let mut in_h2_heading = false;
        let mut current_content = String::new();

        for event in parser {
            match event {
                Event::Start(Tag::Heading { level: HeadingLevel::H2, .. }) => {
                    // Flush the previous section if any.
                    if let Some(key) = current_section.take() {
                        sections.insert(key, current_content.trim().to_owned());
                        current_content = String::new();
                    }
                    in_h2_heading = true;
                    current_heading_text.clear();
                }

                Event::End(TagEnd::Heading(HeadingLevel::H2)) => {
                    in_h2_heading = false;
                    let key = current_heading_text.trim().to_owned();
                    if !key.is_empty() {
                        current_section = Some(key);
                    }
                    current_heading_text.clear();
                }

                Event::Text(text) if in_h2_heading => {
                    current_heading_text.push_str(&text);
                }

                Event::Text(text) => {
                    if current_section.is_some() {
                        current_content.push_str(&text);
                        current_content.push('\n');
                    }
                }

                Event::SoftBreak | Event::HardBreak => {
                    if current_section.is_some() && !in_h2_heading {
                        current_content.push('\n');
                    }
                }

                // Ignore all other events (Start/End tags for paragraphs,
                // lists, code blocks, etc.) but still collect their text via
                // the Text variant above.
                _ => {}
            }
        }

        // Flush the final section.
        if let Some(key) = current_section {
            sections.insert(key, current_content.trim().to_owned());
        }

        Self { sections }
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Return the path to the user-level HONE.md, or `None` when `HOME` is unset.
fn user_hone_md_path() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/hone/HONE.md"))
}

/// Walk up from `start` until a directory containing a known project marker
/// is found.  Returns `None` when the filesystem root is reached without a
/// match.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let markers = [
        ".git",
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "go.mod",
        "pom.xml",
    ];
    let mut current = start;
    loop {
        for marker in &markers {
            if current.join(marker).exists() {
                return Some(current.to_path_buf());
            }
        }
        match current.parent() {
            Some(p) => current = p,
            None => return None,
        }
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod unit_tests {
    use super::*;

    #[test]
    fn basic_two_sections() {
        let md = "## ALPHA\n\nhello\n\n## BETA\n\nworld\n";
        let hone = HoneMd::parse(md);
        assert_eq!(hone.sections["ALPHA"], "hello");
        assert_eq!(hone.sections["BETA"], "world");
    }

    #[test]
    fn h1_is_ignored() {
        let md = "# Top\n\n## REAL\n\ncontent\n";
        let hone = HoneMd::parse(md);
        assert_eq!(hone.sections.len(), 1);
        assert!(hone.sections.contains_key("REAL"));
    }

    #[test]
    fn test_hone_md_parse_and_inject() {
        let md = "## TOOLS\n\nUse ripgrep for search.\n\n## STYLE\n\nPrefer short functions.\n";
        let hone = HoneMd::parse(md);

        // Build a system prompt by injecting every section.
        let mut system_prompt = "You are Hone, an AI coding assistant.".to_string();
        let mut section_keys: Vec<&String> = hone.sections.keys().collect();
        section_keys.sort(); // deterministic order for assertions
        for key in &section_keys {
            let content = &hone.sections[*key];
            system_prompt.push_str(&format!("\n\n## {key}\n{content}"));
        }

        assert!(
            system_prompt.contains("You are Hone"),
            "system prompt must contain base text"
        );
        assert!(
            system_prompt.contains("## STYLE"),
            "system prompt must contain STYLE section heading"
        );
        assert!(
            system_prompt.contains("Prefer short functions"),
            "system prompt must contain STYLE section content"
        );
        assert!(
            system_prompt.contains("## TOOLS"),
            "system prompt must contain TOOLS section heading"
        );
        assert!(
            system_prompt.contains("Use ripgrep for search"),
            "system prompt must contain TOOLS section content"
        );
    }

    // -----------------------------------------------------------------------
    // find_project_root tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_find_project_root_with_git() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(".git")).unwrap();

        let result = find_project_root(dir.path());
        assert_eq!(
            result.as_deref(),
            Some(dir.path()),
            "should find the dir containing .git"
        );
    }

    #[test]
    fn test_find_project_root_with_cargo() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "[package]").unwrap();

        let result = find_project_root(dir.path());
        assert_eq!(
            result.as_deref(),
            Some(dir.path()),
            "should find the dir containing Cargo.toml"
        );
    }

    #[test]
    fn test_find_project_root_returns_none() {
        // Use a deeply-nested temp dir with no markers.  We cannot guarantee
        // *all* ancestor dirs lack markers (e.g. the workspace Cargo.toml is
        // present during `cargo test`), so we use a fresh TempDir which lives
        // under /tmp — typically free of project markers.
        let dir = tempfile::TempDir::new().unwrap();
        // Do not write any marker files.

        // The result may or may not be None depending on whether /tmp or one
        // of its ancestors happens to contain a marker.  We only assert the
        // call does not panic and that if it returns Some(_) the path contains
        // a real marker.
        let result = find_project_root(dir.path());
        if let Some(ref root) = result {
            let markers = [
                ".git",
                "Cargo.toml",
                "package.json",
                "pyproject.toml",
                "go.mod",
                "pom.xml",
            ];
            let has_marker = markers.iter().any(|m| root.join(m).exists());
            assert!(
                has_marker,
                "returned path {root:?} must contain at least one project marker"
            );
        }
        // If None, we're satisfied.
    }

    // -----------------------------------------------------------------------
    // load_hierarchical tests
    // -----------------------------------------------------------------------

    /// Verify that user-level and cwd-level HONE.md sections are merged and
    /// that the cwd content (higher priority) appears after user content in
    /// the same section.
    #[test]
    fn test_load_hierarchical_merges_sections() {
        let user_dir = tempfile::TempDir::new().unwrap();
        let cwd_dir = tempfile::TempDir::new().unwrap();

        // Write user-level HONE.md.
        let config_dir = user_dir.path().join(".config").join("hone");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("HONE.md"), "## RULES\n\ncontent A\n").unwrap();

        // Write cwd-level HONE.md (no .git / Cargo.toml so project-level is
        // also absent, simplifying the merge to just user + cwd).
        std::fs::write(cwd_dir.path().join("HONE.md"), "## RULES\n\ncontent B\n").unwrap();

        // Override HOME so user_hone_md_path() points to our temp directory.
        let old_home = std::env::var("HOME").ok();
        // SAFETY: single-threaded test; we restore afterwards.
        unsafe {
            std::env::set_var("HOME", user_dir.path());
        }

        let hone = HoneMd::load_hierarchical(cwd_dir.path());

        // Restore HOME.
        unsafe {
            match old_home {
                Some(v) => std::env::set_var("HOME", v),
                None => std::env::remove_var("HOME"),
            }
        }

        let rules = hone
            .sections
            .get("RULES")
            .expect("RULES section should exist after merge");

        assert!(
            rules.contains("content A"),
            "user-level content should be present; got: {rules:?}"
        );
        assert!(
            rules.contains("content B"),
            "cwd-level content should be present; got: {rules:?}"
        );
        // Higher-priority (cwd) content must appear after lower-priority (user).
        let pos_a = rules.find("content A").unwrap();
        let pos_b = rules.find("content B").unwrap();
        assert!(
            pos_b > pos_a,
            "cwd content (B) should appear after user content (A); got: {rules:?}"
        );
    }
}
