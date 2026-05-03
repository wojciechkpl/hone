use std::path::Path;

#[derive(serde::Serialize, serde::Deserialize)]
struct Root {
    entries: Vec<MarketplaceEntry>,
}

/// One curated MCP server entry.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MarketplaceEntry {
    /// Short stable id (kebab-case, e.g. "filesystem", "github", "puppeteer").
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// One-sentence description shown in pickers.
    pub description: String,
    /// Command to spawn the server (e.g. "npx").
    pub command: String,
    /// Arguments to the command (e.g. ["-y", "@modelcontextprotocol/server-filesystem", "/some/path"]).
    pub args: Vec<String>,
    /// URL to the server's source / docs.
    pub source_url: String,
    /// Hint shown in the picker explaining how to install / configure.
    pub install_hint: String,
    /// Tags for filtering (e.g. ["filesystem", "official", "anthropic"]).
    #[serde(default)]
    pub tags: Vec<String>,
    /// Whether this entry is officially endorsed by the MCP project.
    #[serde(default)]
    pub official: bool,
}

/// In-memory marketplace registry.
#[derive(Debug, Default, Clone)]
pub struct Marketplace {
    pub entries: Vec<MarketplaceEntry>,
}

#[derive(Debug, thiserror::Error)]
pub enum MarketplaceError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml: {0}")]
    Toml(String),
    #[error("duplicate id: {0}")]
    DuplicateId(String),
}

impl Marketplace {
    /// Load the built-in curated list bundled with Hone.
    pub fn builtin() -> Self {
        let root: Root =
            toml::from_str(BUILTIN_TOML).expect("builtin TOML must parse");
        Self { entries: root.entries }
    }

    /// Load from a TOML file (returns Ok(default) when missing).
    pub fn load_from_toml(path: &Path) -> Result<Self, MarketplaceError> {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(e) => return Err(MarketplaceError::Io(e)),
        };
        let root: Root = toml::from_str(&text)
            .map_err(|e| MarketplaceError::Toml(e.to_string()))?;
        // Validate uniqueness of ids.
        let mut seen = std::collections::HashSet::new();
        for entry in &root.entries {
            if !seen.insert(entry.id.clone()) {
                return Err(MarketplaceError::DuplicateId(entry.id.clone()));
            }
        }
        Ok(Self { entries: root.entries })
    }

    /// Save to a TOML file (creates parent dirs as needed).
    pub fn save_to(&self, path: &Path) -> Result<(), MarketplaceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let root = Root { entries: self.entries.clone() };
        let text = toml::to_string(&root)
            .map_err(|e| MarketplaceError::Toml(e.to_string()))?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Look up by id.
    pub fn get(&self, id: &str) -> Option<&MarketplaceEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// All entries with the given tag.
    pub fn by_tag(&self, tag: &str) -> Vec<&MarketplaceEntry> {
        self.entries
            .iter()
            .filter(|e| e.tags.iter().any(|t| t == tag))
            .collect()
    }

    /// Filter to officially-endorsed entries.
    pub fn official_only(&self) -> Vec<&MarketplaceEntry> {
        self.entries.iter().filter(|e| e.official).collect()
    }

    /// Total count.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

// ---------------------------------------------------------------------------
// Helper: a raw TOML string the implementer can use as the builtin source.
// The implementer should parse this in builtin() using the `toml` crate.
// ---------------------------------------------------------------------------
#[allow(dead_code)]
const BUILTIN_TOML: &str = r#"
[[entries]]
id = "filesystem"
name = "Filesystem"
description = "Read and write files on the local filesystem."
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
source_url = "https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem"
install_hint = "Requires Node.js >=18 and npm. Replace /tmp with the directory you want the model to access."
tags = ["filesystem", "official", "anthropic"]
official = true

[[entries]]
id = "github"
name = "GitHub"
description = "Search and read GitHub repositories, issues, and pull requests."
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
source_url = "https://github.com/modelcontextprotocol/servers/tree/main/src/github"
install_hint = "Requires Node.js >=18, npm, and a GITHUB_PERSONAL_ACCESS_TOKEN env var."
tags = ["github", "official", "anthropic", "vcs"]
official = true

[[entries]]
id = "git"
name = "Git"
description = "Run git commands against a local repository."
command = "npx"
args = ["-y", "@modelcontextprotocol/server-git"]
source_url = "https://github.com/modelcontextprotocol/servers/tree/main/src/git"
install_hint = "Requires Node.js >=18 and npm. Run from inside the repo you want the model to access."
tags = ["git", "official", "anthropic", "vcs"]
official = true

[[entries]]
id = "slack"
name = "Slack"
description = "Post messages and read channel history from Slack workspaces."
command = "npx"
args = ["-y", "@modelcontextprotocol/server-slack"]
source_url = "https://github.com/modelcontextprotocol/servers/tree/main/src/slack"
install_hint = "Requires Node.js >=18, npm, and SLACK_BOT_TOKEN + SLACK_TEAM_ID env vars."
tags = ["slack", "official", "anthropic", "communication"]
official = true

[[entries]]
id = "puppeteer"
name = "Puppeteer"
description = "Control a headless Chromium browser for web scraping and automation."
command = "npx"
args = ["-y", "@modelcontextprotocol/server-puppeteer"]
source_url = "https://github.com/modelcontextprotocol/servers/tree/main/src/puppeteer"
install_hint = "Requires Node.js >=18 and npm. Chromium is downloaded automatically on first run."
tags = ["browser", "official", "anthropic", "automation"]
official = true

[[entries]]
id = "sqlite"
name = "SQLite (community)"
description = "Query and inspect a local SQLite database."
command = "npx"
args = ["-y", "mcp-server-sqlite", "--db-path", "/tmp/data.db"]
source_url = "https://github.com/joshuarileydev/sqlite-mcp-server"
install_hint = "Requires Node.js >=18 and npm. Replace /tmp/data.db with your database path."
tags = ["database", "sqlite", "community"]
official = false

[[entries]]
id = "brave-search"
name = "Brave Search (community)"
description = "Perform web searches using the Brave Search API."
command = "npx"
args = ["-y", "mcp-server-brave-search"]
source_url = "https://github.com/modelcontextprotocol/servers/tree/main/src/brave-search"
install_hint = "Requires Node.js >=18, npm, and a BRAVE_API_KEY env var."
tags = ["search", "web", "community"]
official = false
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -----------------------------------------------------------------------
    // MarketplaceEntry serde
    // -----------------------------------------------------------------------

    #[test]
    fn entry_serde_roundtrip() {
        let entry = MarketplaceEntry {
            id: "test-server".to_string(),
            name: "Test Server".to_string(),
            description: "A test MCP server.".to_string(),
            command: "npx".to_string(),
            args: vec!["-y".to_string(), "@test/server".to_string()],
            source_url: "https://example.com/test".to_string(),
            install_hint: "Requires Node.js.".to_string(),
            tags: vec!["test".to_string(), "official".to_string()],
            official: true,
        };

        let json = serde_json::to_string(&entry).expect("serialize");
        let decoded: MarketplaceEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(entry, decoded);
    }

    #[test]
    fn entry_default_tags_is_empty() {
        let json = r#"{
            "id": "no-tags",
            "name": "No Tags",
            "description": "desc",
            "command": "npx",
            "args": [],
            "source_url": "https://example.com",
            "install_hint": "hint"
        }"#;
        let entry: MarketplaceEntry = serde_json::from_str(json).expect("deserialize");
        assert!(entry.tags.is_empty(), "tags should default to empty vec");
    }

    #[test]
    fn entry_default_official_is_false() {
        let json = r#"{
            "id": "no-official",
            "name": "No Official",
            "description": "desc",
            "command": "npx",
            "args": [],
            "source_url": "https://example.com",
            "install_hint": "hint"
        }"#;
        let entry: MarketplaceEntry = serde_json::from_str(json).expect("deserialize");
        assert!(!entry.official, "official should default to false");
    }

    // -----------------------------------------------------------------------
    // Marketplace::builtin()
    // -----------------------------------------------------------------------

    #[test]
    fn builtin_includes_at_least_five_entries() {
        let m = Marketplace::builtin();
        assert!(
            m.len() >= 5,
            "expected at least 5 builtin entries, got {}",
            m.len()
        );
    }

    #[test]
    fn builtin_includes_filesystem_entry() {
        let m = Marketplace::builtin();
        assert!(
            m.get("filesystem").is_some(),
            "builtin marketplace must include 'filesystem'"
        );
    }

    #[test]
    fn builtin_includes_github_entry() {
        let m = Marketplace::builtin();
        assert!(
            m.get("github").is_some(),
            "builtin marketplace must include 'github'"
        );
    }

    #[test]
    fn builtin_entries_have_unique_ids() {
        let m = Marketplace::builtin();
        let mut seen = std::collections::HashSet::new();
        for entry in &m.entries {
            assert!(
                seen.insert(entry.id.clone()),
                "duplicate id found in builtin: {}",
                entry.id
            );
        }
    }

    #[test]
    fn builtin_official_entries_outnumber_community_entries() {
        let m = Marketplace::builtin();
        let official = m.official_only().len();
        let community = m.entries.iter().filter(|e| !e.official).count();
        assert!(
            official > community,
            "expected more official ({official}) than community ({community}) entries in builtin"
        );
    }

    #[test]
    fn builtin_filesystem_entry_has_npx_command() {
        let m = Marketplace::builtin();
        let fs = m.get("filesystem").expect("filesystem entry must exist");
        assert_eq!(
            fs.command, "npx",
            "filesystem entry must use npx as command"
        );
    }

    // -----------------------------------------------------------------------
    // Tag and filter helpers
    // -----------------------------------------------------------------------

    #[test]
    fn by_tag_returns_only_matching_entries() {
        let m = Marketplace::builtin();
        // "official" tag is present on all official entries per BUILTIN_TOML.
        let results = m.by_tag("official");
        assert!(
            !results.is_empty(),
            "expected at least one entry tagged 'official'"
        );
        for entry in &results {
            assert!(
                entry.tags.contains(&"official".to_string()),
                "by_tag returned entry without the requested tag: {}",
                entry.id
            );
        }
    }

    #[test]
    fn by_tag_unknown_returns_empty() {
        let m = Marketplace::builtin();
        let results = m.by_tag("this-tag-does-not-exist-xyzzy");
        assert!(results.is_empty(), "expected empty vec for unknown tag");
    }

    #[test]
    fn official_only_returns_only_official_entries() {
        let m = Marketplace::builtin();
        let results = m.official_only();
        assert!(
            !results.is_empty(),
            "expected at least one official entry in builtin"
        );
        for entry in &results {
            assert!(
                entry.official,
                "official_only returned non-official entry: {}",
                entry.id
            );
        }
    }

    // -----------------------------------------------------------------------
    // Persistence
    // -----------------------------------------------------------------------

    #[test]
    fn save_and_load_round_trip() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("marketplace.toml");

        let original = Marketplace::builtin();
        original.save_to(&path).expect("save");

        let loaded = Marketplace::load_from_toml(&path).expect("load");
        assert_eq!(
            original.len(),
            loaded.len(),
            "entry count must match after round-trip"
        );

        // Entries are ordered; compare the first one for deep equality.
        assert_eq!(
            original.entries.first(),
            loaded.entries.first(),
            "first entry must match after round-trip"
        );
    }

    #[test]
    fn load_missing_file_returns_default_empty() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("nonexistent.toml");

        let m = Marketplace::load_from_toml(&path).expect("missing file must return Ok");
        assert!(
            m.is_empty(),
            "loading a missing file must return an empty marketplace"
        );
    }

    #[test]
    fn load_invalid_toml_returns_err() {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, b"[[entries]\nNOT VALID TOML }{").expect("write bad toml");

        let result = Marketplace::load_from_toml(&path);
        assert!(
            result.is_err(),
            "loading invalid TOML must return Err, got Ok"
        );
        // Verify it is a Toml variant, not an Io error.
        assert!(
            matches!(result.unwrap_err(), MarketplaceError::Toml(_)),
            "error must be MarketplaceError::Toml"
        );
    }

    #[test]
    fn save_creates_parent_dirs_if_missing() {
        let dir = TempDir::new().expect("tempdir");
        // Three levels of non-existent intermediate directories.
        let path = dir.path().join("a").join("b").join("c").join("market.toml");

        let m = Marketplace::builtin();
        m.save_to(&path).expect("save must create intermediate dirs");

        assert!(path.exists(), "file must exist after save");
    }

    // -----------------------------------------------------------------------
    // Lookup helpers
    // -----------------------------------------------------------------------

    #[test]
    fn get_returns_none_for_unknown_id() {
        let m = Marketplace::builtin();
        assert!(
            m.get("this-server-does-not-exist-xyzzy").is_none(),
            "get with unknown id must return None"
        );
    }

    #[test]
    fn len_and_is_empty_consistent() {
        let empty = Marketplace::default();
        assert!(empty.is_empty());
        assert_eq!(empty.len(), 0);

        let full = Marketplace::builtin();
        assert!(!full.is_empty());
        assert_eq!(full.len(), full.entries.len());
    }
}
