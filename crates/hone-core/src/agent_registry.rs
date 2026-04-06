use std::collections::HashMap;
use std::path::Path;

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
}

impl Default for AgentRegistry {
    fn default() -> Self {
        Self::new()
    }
}
