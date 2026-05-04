use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Per-agent sandbox configuration parsed from frontmatter.
///
/// When present in an agent's frontmatter `sandbox:` block, this profile
/// overrides the global [`hone_sandbox::SandboxPolicy`] for that agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SandboxProfile {
    pub mode: hone_sandbox::SandboxMode,
    #[serde(default)]
    pub allow_read: Vec<std::path::PathBuf>,
    #[serde(default)]
    pub allow_write: Vec<std::path::PathBuf>,
}

impl SandboxProfile {
    /// Build a [`hone_sandbox::SandboxPolicy`] from this profile.
    pub fn to_policy(&self) -> hone_sandbox::SandboxPolicy {
        let mut builder = hone_sandbox::SandboxPolicyBuilder::new(self.mode);
        for p in &self.allow_read {
            builder = builder.allow_read(p.clone());
        }
        for p in &self.allow_write {
            builder = builder.allow_write(p.clone());
        }
        builder.build().expect("SandboxProfile::to_policy build failed")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub memory: String,
    #[serde(default)]
    pub sub_agents: Vec<String>,
    /// The full system prompt (markdown body after frontmatter)
    #[serde(skip)]
    pub system_prompt: String,
    /// Optional per-agent sandbox profile; overrides the global policy when set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxProfile>,
}

fn default_model() -> String {
    "sonnet".to_string()
}

impl AgentDef {
    /// Parse a Markdown file with YAML frontmatter into an `AgentDef`.
    ///
    /// The content must start with `---\n`, followed by YAML frontmatter,
    /// then a closing `---\n`. Everything after the closing delimiter becomes
    /// the `system_prompt`.
    pub fn from_markdown(content: &str) -> Result<Self, AgentDefError> {
        let after_open = content
            .strip_prefix("---\n")
            .ok_or(AgentDefError::MissingFrontmatter)?;

        let close = after_open
            .find("\n---\n")
            .ok_or(AgentDefError::MissingFrontmatter)?;

        let yaml = &after_open[..close];
        let body = &after_open[close + "\n---\n".len()..];

        let mut def: AgentDef =
            serde_yaml::from_str(yaml).map_err(|e| AgentDefError::Frontmatter(e.to_string()))?;

        def.system_prompt = body.to_string();
        Ok(def)
    }

    /// Load all `.md` agent definitions from a directory.
    ///
    /// Files that fail to parse are returned as errors; the first error
    /// encountered is returned. Non-`.md` files are silently skipped.
    pub fn load_dir(dir: &Path) -> Result<Vec<Self>, AgentDefError> {
        let mut defs = Vec::new();

        let read_dir = std::fs::read_dir(dir)?;
        let mut entries: Vec<_> = read_dir.collect::<Result<_, _>>()?;
        entries.sort_by_key(|e| e.file_name());

        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = std::fs::read_to_string(&path)?;
            let def = Self::from_markdown(&content)?;
            defs.push(def);
        }

        Ok(defs)
    }

    /// Load all `.md` agent definitions from a directory, tolerantly.
    ///
    /// * If `dir` does not exist the function returns an empty `Vec` silently.
    /// * Other I/O errors on the directory itself (e.g. permission denied)
    ///   emit a `tracing::warn!` and return an empty `Vec` so the caller
    ///   can surface a misconfiguration without aborting the whole load.
    /// * Non-`.md` files are silently skipped.
    /// * Files that fail to read or parse emit a `tracing::warn!` and are
    ///   skipped; loading continues for remaining files.
    pub fn load_dir_lenient(dir: &Path) -> Vec<Self> {
        let read_dir = match std::fs::read_dir(dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(e) => {
                tracing::warn!(
                    dir = %dir.display(),
                    "failed to open agents directory: {e}",
                );
                return Vec::new();
            }
        };

        let mut entries: Vec<_> = read_dir.flatten().collect();
        entries.sort_by_key(|e| e.file_name());

        let mut defs = Vec::new();
        for entry in entries {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to read agent file: {e}");
                    continue;
                }
            };
            match Self::from_markdown(&content) {
                Ok(def) => defs.push(def),
                Err(e) => {
                    tracing::warn!(path = %path.display(), "failed to parse agent file: {e}");
                }
            }
        }
        defs
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentDefError {
    #[error("invalid frontmatter: {0}")]
    Frontmatter(String),
    #[error("missing frontmatter delimiters")]
    MissingFrontmatter,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

// Silence the unused-import warning when serde_yaml::Error is only used via
// the #[from] conversion in AgentDefError.
const _: () = {
    let _ = std::mem::size_of::<HashMap<(), ()>>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use hone_sandbox::SandboxMode;

    // -----------------------------------------------------------------------
    // Frontmatter parsing tests
    // -----------------------------------------------------------------------

    #[test]
    fn from_markdown_without_sandbox_block_returns_none() {
        let md = "---\nname: simple-agent\ndescription: A simple agent\n---\nBody text.\n";
        let def = AgentDef::from_markdown(md).expect("should parse");
        // Contract: missing sandbox block → field is None
        assert!(def.sandbox.is_none(), "expected sandbox to be None but got {:?}", def.sandbox);
    }

    #[test]
    fn from_markdown_with_manual_sandbox_parses_mode() {
        // 'manual' maps to SandboxMode::Manual
        let md = "---\nname: manual-agent\ndescription: Agent with manual sandbox\nsandbox:\n  mode: manual\n---\nBody.\n";
        let def = AgentDef::from_markdown(md).expect("should parse");
        let profile = def.sandbox.expect("sandbox should be Some");
        // Contract: mode field correctly parsed
        assert_eq!(profile.mode, SandboxMode::Manual);
    }

    #[test]
    fn from_markdown_with_full_sandbox_block_parses_lists() {
        let md = concat!(
            "---\n",
            "name: restricted-agent\n",
            "description: Fully restricted agent\n",
            "sandbox:\n",
            "  mode: manual\n",
            "  allow_read:\n",
            "    - /a\n",
            "    - /b\n",
            "  allow_write:\n",
            "    - /c\n",
            "---\n",
            "Body.\n"
        );
        let def = AgentDef::from_markdown(md).expect("should parse");
        let profile = def.sandbox.expect("sandbox should be Some");
        assert_eq!(profile.mode, SandboxMode::Manual);
        // Contract: allow_read and allow_write lists populated correctly
        assert_eq!(
            profile.allow_read,
            vec![
                std::path::PathBuf::from("/a"),
                std::path::PathBuf::from("/b"),
            ]
        );
        assert_eq!(
            profile.allow_write,
            vec![std::path::PathBuf::from("/c")]
        );
    }

    #[test]
    fn from_markdown_with_sandbox_missing_mode_field_errors() {
        // sandbox block without 'mode' key must produce a parse error
        let md = concat!(
            "---\n",
            "name: broken-agent\n",
            "description: Agent with incomplete sandbox\n",
            "sandbox:\n",
            "  allow_read:\n",
            "    - /workspace\n",
            "---\n",
            "Body.\n"
        );
        let result = AgentDef::from_markdown(md);
        // Contract: missing required 'mode' field → Err
        assert!(
            result.is_err(),
            "expected Err for missing mode field, got Ok({:?})",
            result.ok()
        );
    }

    // -----------------------------------------------------------------------
    // SandboxProfile::to_policy tests
    // -----------------------------------------------------------------------

    #[test]
    fn to_policy_round_trips_mode_and_paths() {
        let profile = SandboxProfile {
            mode: SandboxMode::Manual,
            allow_read: vec![
                std::path::PathBuf::from("/workspace"),
                std::path::PathBuf::from("/etc/hosts"),
            ],
            allow_write: vec![std::path::PathBuf::from("/tmp/output")],
        };
        let policy = profile.to_policy();
        // Contract: mode, allow_read, and allow_write survive the round-trip
        assert_eq!(policy.mode, SandboxMode::Manual);
        assert_eq!(
            policy.allow_read,
            vec![
                std::path::PathBuf::from("/workspace"),
                std::path::PathBuf::from("/etc/hosts"),
            ]
        );
        assert_eq!(
            policy.allow_write,
            vec![std::path::PathBuf::from("/tmp/output")]
        );
    }

    #[test]
    fn to_policy_with_empty_lists_yields_empty_policy_lists() {
        let profile = SandboxProfile {
            mode: SandboxMode::Supervised,
            allow_read: vec![],
            allow_write: vec![],
        };
        let policy = profile.to_policy();
        // Contract: empty lists → empty lists in the produced policy
        assert_eq!(policy.mode, SandboxMode::Supervised);
        assert!(policy.allow_read.is_empty());
        assert!(policy.allow_write.is_empty());
    }

    // -----------------------------------------------------------------------
    // Backward-compat tests
    // -----------------------------------------------------------------------

    #[test]
    fn agent_def_default_has_no_sandbox() {
        // The minimal valid frontmatter parse must yield sandbox == None.
        // AgentDef has no Default impl; verify via from_markdown with no sandbox block.
        let md = "---\nname: default-test\ndescription: Minimal agent\n---\n";
        let def = AgentDef::from_markdown(md).expect("should parse");
        assert!(
            def.sandbox.is_none(),
            "serde default must leave sandbox as None, got {:?}",
            def.sandbox
        );
    }

    #[test]
    fn existing_built_in_agents_still_load() {
        // All built-in agents must still parse correctly after the new field is added.
        // The serde #[serde(default)] ensures backward compat without explicit assertion.
        let mut registry = crate::AgentRegistry::new();
        registry.load_builtins();
        let agents = registry.list();
        // Contract: all 12 built-in agents load without panicking
        assert!(
            agents.len() >= 12,
            "expected at least 12 built-in agents, got {}",
            agents.len()
        );
        // All built-ins have no sandbox block → sandbox field must be None for each
        for agent in &agents {
            assert!(
                agent.sandbox.is_none(),
                "built-in agent '{}' unexpectedly has a sandbox profile",
                agent.name
            );
        }
    }
}
