/// Smoke test: `filtered_registry` is callable from outside `hone-core` and
/// correctly restricts a registry to the tools listed in an `AgentDef.tools`
/// allowlist.
use std::sync::Arc;

use async_trait::async_trait;
use hone_core::{AgentDef, filtered_registry};
use hone_proto::ToolLayer;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput};
use serde_json::json;

// ---------------------------------------------------------------------------
// Minimal mock tool
// ---------------------------------------------------------------------------

struct MockTool {
    name: &'static str,
}

#[async_trait]
impl Tool for MockTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "mock"
    }

    fn schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::new(json!("ok")))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

fn mock_tool(name: &'static str) -> Arc<dyn Tool> {
    Arc::new(MockTool { name })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn filtered_registry_applied_to_agent_def_tools() {
    let base = hone_tools::ToolRegistry::builder()
        .register_arc(mock_tool("read_file"))
        .register_arc(mock_tool("grep"))
        .register_arc(mock_tool("shell"))
        .build();

    let agent_def = AgentDef {
        name: "researcher".to_string(),
        description: "A researcher agent".to_string(),
        system_prompt: "You are a researcher.".to_string(),
        tools: vec!["read_file".to_string(), "grep".to_string()],
        model: "sonnet".to_string(),
        memory: String::new(),
        sub_agents: vec![],
    };

    let filtered = filtered_registry(&base, &agent_def.tools);

    assert!(
        filtered.get("read_file").is_some(),
        "read_file should be in filtered registry"
    );
    assert!(
        filtered.get("grep").is_some(),
        "grep should be in filtered registry"
    );
    assert!(
        filtered.get("shell").is_none(),
        "shell must be excluded by the allowlist"
    );
    assert_eq!(filtered.len(), 2, "filtered registry should have exactly 2 tools");
}
