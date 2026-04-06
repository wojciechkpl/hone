use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

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

        let mut def: AgentDef = serde_yaml::from_str(yaml)
            .map_err(|e| AgentDefError::Frontmatter(e.to_string()))?;

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
