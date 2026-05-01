use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::agent_def::{AgentDef, AgentDefError};

// Built-in agent definitions embedded at compile time.
const BUILTIN_AGENTS: &[&str] = &[
    include_str!("../agents/api-designer.md"),
    include_str!("../agents/code-reviewer.md"),
    include_str!("../agents/data-engineer.md"),
    include_str!("../agents/debugger.md"),
    include_str!("../agents/documentation-agent.md"),
    include_str!("../agents/ml-engineer.md"),
    include_str!("../agents/performance-optimizer.md"),
    include_str!("../agents/python-expert.md"),
    include_str!("../agents/researcher.md"),
    include_str!("../agents/rust-expert.md"),
    include_str!("../agents/security-auditor.md"),
    include_str!("../agents/tdd-workflow.md"),
];

pub struct AgentRegistry {
    agents: HashMap<String, AgentDef>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    pub fn register(&mut self, def: AgentDef) {
        self.agents.insert(def.name.clone(), def);
    }

    pub fn get(&self, name: &str) -> Option<&AgentDef> {
        self.agents.get(name)
    }

    pub fn list(&self) -> Vec<&AgentDef> {
        let mut list: Vec<&AgentDef> = self.agents.values().collect();
        list.sort_by_key(|d| &d.name);
        list
    }

    /// Find the first agent whose description contains `query` (case-insensitive).
    pub fn find_by_description(&self, query: &str) -> Option<&AgentDef> {
        let q = query.to_lowercase();
        // Sort by name for deterministic results.
        let mut candidates: Vec<&AgentDef> = self
            .agents
            .values()
            .filter(|d| d.description.to_lowercase().contains(&q))
            .collect();
        candidates.sort_by_key(|d| &d.name);
        candidates.into_iter().next()
    }

    /// Register all built-in agent definitions embedded at compile time.
    pub fn load_builtins(&mut self) {
        for src in BUILTIN_AGENTS {
            match AgentDef::from_markdown(src) {
                Ok(def) => self.register(def),
                Err(e) => {
                    // Built-in agents are compiled-in; a parse failure is a
                    // programmer error, not a runtime condition.
                    tracing::error!("failed to parse built-in agent: {e}");
                }
            }
        }
    }

    /// Load agent definitions from `.md` files in `dir` and register them.
    pub fn load_from_dir(&mut self, dir: &Path) -> Result<(), AgentDefError> {
        for def in AgentDef::load_dir(dir)? {
            self.register(def);
        }
        Ok(())
    }

    /// Build a registry with three overlays applied in increasing precedence:
    ///   1. compiled-in built-in agents
    ///   2. user-level agents from `~/.config/hone/agents/*.md`
    ///   3. project-level agents from `<project_root>/.hone/agents/*.md`
    ///      where project_root is the nearest ancestor of `working_dir`
    ///      containing `.git` / `Cargo.toml` / `package.json` /
    ///      `pyproject.toml` / `go.mod` / `pom.xml`.
    ///
    /// Same-name agents at a higher overlay replace lower ones.
    /// Missing directories are silently skipped.  Malformed files emit a
    /// `tracing::warn!` but do not abort loading of other files.
    pub fn load_hierarchical(working_dir: &Path) -> Self {
        let mut registry = Self::new();

        // Layer 1: built-ins.
        registry.load_builtins();

        // Layer 2: user-level agents from ~/.config/hone/agents/.
        if let Some(user_agents_dir) = user_agents_dir() {
            for def in AgentDef::load_dir_lenient(&user_agents_dir) {
                registry.register(def);
            }
        }

        // Layer 3: project-level agents from <project_root>/.hone/agents/.
        if let Some(project_root) = hone_config::find_project_root(working_dir) {
            let project_agents_dir = project_root.join(".hone/agents");
            for def in AgentDef::load_dir_lenient(&project_agents_dir) {
                registry.register(def);
            }
        }

        registry
    }
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Return `~/.config/hone/agents/` as a `PathBuf`, or `None` when `HOME` is
/// unset.  The directory may not exist; callers must handle that case.
fn user_agents_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config/hone/agents"))
}
