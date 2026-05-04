use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hone_tools::ToolRegistry;

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
// Allowlist enforcement — public API
// ---------------------------------------------------------------------------

/// A structured error produced when an agent attempts to call a tool that is
/// not in its allowlist.
#[derive(Debug, Clone)]
pub struct AllowlistRejection {
    pub tool_name: String,
    pub agent_name: String,
    pub allowed: Vec<String>,
}

impl AllowlistRejection {
    /// Render as a JSON tool-result the model will see so it can self-correct.
    ///
    /// Shape:
    /// ```json
    /// {
    ///   "error": "tool_not_allowed",
    ///   "tool": "<tool_name>",
    ///   "agent": "<agent_name>",
    ///   "allowed_tools": ["<tool1>", ...]
    /// }
    /// ```
    pub fn to_tool_error_json(&self) -> serde_json::Value {
        serde_json::json!({
            "error": "tool_not_allowed",
            "tool": self.tool_name,
            "agent": self.agent_name,
            "allowed_tools": self.allowed,
        })
    }
}

/// Build a [`ToolRegistry`] filtered to the tools an agent is allowed to use.
///
/// Behaviour:
/// * If `allowed.is_empty()`, return `base` unchanged (no allowlist == all tools).
/// * Otherwise, return a new registry containing only tools whose `name()` is
///   in `allowed`.
/// * Tool names in `allowed` that don't exist in `base` are silently ignored
///   (logged via `tracing::warn!`).
pub fn filtered_registry(base: &ToolRegistry, allowed: &[String]) -> ToolRegistry {
    if allowed.is_empty() {
        return base.clone();
    }

    let mut builder = ToolRegistry::builder();
    for name in allowed {
        match base.get(name) {
            Some(tool) => builder = builder.register_arc(tool.clone()),
            None => tracing::warn!(tool = %name, "tool in allowlist not found in base registry; skipping"),
        }
    }
    builder.build()
}

impl AgentRegistry {
    /// Return the tool allowlist for `agent_name`, or `None` if the agent is
    /// not registered.
    pub fn allowlist_for(&self, agent_name: &str) -> Option<&[String]> {
        self.agents.get(agent_name).map(|d| d.tools.as_slice())
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use hone_proto::ToolLayer;
    use hone_tools::{Tool, ToolContext, ToolInput, ToolOutput, ToolError};
    use serde_json::Value;

    // -----------------------------------------------------------------------
    // MockTool — minimal Tool impl for test registries.
    // -----------------------------------------------------------------------

    struct MockTool {
        name: &'static str,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            self.name
        }

        fn description(&self) -> &str {
            "mock tool for testing"
        }

        fn schema(&self) -> Value {
            serde_json::json!({})
        }

        async fn call(
            &self,
            _input: ToolInput,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::new(serde_json::json!({"ok": true})))
        }

        fn layer(&self) -> ToolLayer {
            ToolLayer::Native
        }
    }

    /// Build a [`ToolRegistry`] containing the named tools.
    fn make_registry(names: &[&'static str]) -> ToolRegistry {
        let mut builder = ToolRegistry::builder();
        for &name in names {
            builder = builder.register(Box::new(MockTool { name }));
        }
        builder.build()
    }

    // -----------------------------------------------------------------------
    // filtered_registry tests
    // -----------------------------------------------------------------------

    #[test]
    fn filtered_registry_empty_allowlist_returns_base_unchanged() {
        let base = make_registry(&["read_file", "grep", "shell"]);
        let result = filtered_registry(&base, &[]);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn filtered_registry_with_allowlist_keeps_only_listed_tools() {
        let base = make_registry(&["read_file", "grep", "shell"]);
        let allowed = vec!["read_file".to_string()];
        let result = filtered_registry(&base, &allowed);
        assert_eq!(result.len(), 1);
        assert!(result.get("read_file").is_some());
        assert!(result.get("grep").is_none());
        assert!(result.get("shell").is_none());
    }

    #[test]
    fn filtered_registry_unknown_tool_in_allowlist_is_silently_skipped() {
        let base = make_registry(&["read_file", "grep"]);
        let allowed = vec!["read_file".to_string(), "nonexistent".to_string()];
        // Must not panic; unknown name is silently ignored.
        let result = filtered_registry(&base, &allowed);
        assert_eq!(result.len(), 1);
        assert!(result.get("read_file").is_some());
    }

    #[test]
    fn filtered_registry_preserves_tool_identity() {
        let base = make_registry(&["read_file", "grep"]);
        let allowed = vec!["read_file".to_string()];
        let result = filtered_registry(&base, &allowed);
        let tool = result.get("read_file").expect("tool must be present");
        assert_eq!(tool.name(), "read_file");
    }

    // -----------------------------------------------------------------------
    // AgentRegistry::allowlist_for tests
    // -----------------------------------------------------------------------

    fn make_agent_def(name: &str, tools: Vec<&str>) -> AgentDef {
        AgentDef {
            name: name.to_string(),
            description: "test agent".to_string(),
            tools: tools.into_iter().map(|s| s.to_string()).collect(),
            model: "sonnet".to_string(),
            memory: String::new(),
            sub_agents: vec![],
            system_prompt: String::new(),
            sandbox: None,
        }
    }

    #[test]
    fn allowlist_for_returns_tools_when_agent_registered() {
        let mut registry = AgentRegistry::new();
        registry.register(make_agent_def("researcher", vec!["read_file", "grep"]));

        let allowlist = registry
            .allowlist_for("researcher")
            .expect("agent must be registered");
        assert_eq!(allowlist, &["read_file", "grep"]);
    }

    #[test]
    fn allowlist_for_returns_none_for_unknown_agent() {
        let registry = AgentRegistry::new();
        assert!(registry.allowlist_for("ghost").is_none());
    }

    #[test]
    fn allowlist_for_returns_empty_slice_when_agent_has_no_tools() {
        let mut registry = AgentRegistry::new();
        registry.register(make_agent_def("no-tools-agent", vec![]));

        let allowlist = registry
            .allowlist_for("no-tools-agent")
            .expect("agent must be registered");
        assert!(allowlist.is_empty());
    }

    // -----------------------------------------------------------------------
    // AllowlistRejection JSON shape test
    // -----------------------------------------------------------------------

    #[test]
    fn allowlist_rejection_renders_expected_json() {
        let rejection = AllowlistRejection {
            tool_name: "shell".to_string(),
            agent_name: "researcher".to_string(),
            allowed: vec!["read_file".to_string(), "grep".to_string()],
        };

        let json = rejection.to_tool_error_json();

        assert_eq!(json["error"], "tool_not_allowed");
        assert_eq!(json["tool"], "shell");
        assert_eq!(json["agent"], "researcher");

        let allowed_tools = json["allowed_tools"]
            .as_array()
            .expect("allowed_tools must be an array");
        let names: Vec<&str> = allowed_tools
            .iter()
            .map(|v| v.as_str().expect("each entry must be a string"))
            .collect();
        assert!(names.contains(&"read_file"));
        assert!(names.contains(&"grep"));
        assert_eq!(names.len(), 2);
    }
}
