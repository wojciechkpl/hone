#![allow(clippy::expect_used, clippy::unwrap_used)]
//! W30 of `docs/plans/worker-tools-spec.md`: built-in agent files list real
//! tool names, so an MCP tool named `read`/`write`/`edit` can never replace a
//! built-in persona's file tools. The aliases stay for user agent files.

use hone_core::AgentRegistry;
use hone_tools::{EditFileTool, GlobTool, GrepTool, ReadFileTool, ShellTool, Tool, WriteFileTool};

fn real_tool_names() -> Vec<String> {
    let tools: Vec<Box<dyn Tool>> = vec![
        Box::new(ReadFileTool),
        Box::new(EditFileTool),
        Box::new(WriteFileTool),
        Box::new(ShellTool),
        Box::new(GrepTool),
        Box::new(GlobTool),
    ];
    tools.iter().map(|t| t.name().to_owned()).collect()
}

#[test]
fn test_w30_builtin_agents_should_list_only_real_tool_names() {
    let real = real_tool_names();
    let mut registry = AgentRegistry::new();
    registry.load_builtins();
    assert!(!registry.list().is_empty());
    for def in registry.list() {
        for tool in &def.tools {
            assert!(
                real.contains(tool),
                "agent {} lists {tool:?}, which is not a real tool name ({real:?})",
                def.name
            );
        }
    }
}

#[test]
fn test_w30_builtin_agents_should_not_use_the_short_aliases() {
    let mut registry = AgentRegistry::new();
    registry.load_builtins();
    for def in registry.list() {
        for alias in ["read", "write", "edit"] {
            assert!(
                !def.tools.iter().any(|t| t == alias),
                "agent {} still lists the alias {alias:?}",
                def.name
            );
        }
    }
}

#[test]
fn test_w30_builtin_agents_should_keep_the_tools_they_had_under_the_aliases() {
    // The rename must not drop anything: rust-expert still has all six.
    let mut registry = AgentRegistry::new();
    registry.load_builtins();
    let mut tools = registry.get("rust-expert").expect("built-in").tools.clone();
    tools.sort();
    let mut want = real_tool_names();
    want.sort();
    assert_eq!(tools, want);
}
