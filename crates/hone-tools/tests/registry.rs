use async_trait::async_trait;
use hone_tools::{GlobTool, ReadFileTool, ToolRegistry, WriteFileTool};

struct DummyTool {
    name: &'static str,
}

#[async_trait]
impl hone_tools::Tool for DummyTool {
    fn name(&self) -> &str {
        self.name
    }

    fn description(&self) -> &str {
        "a dummy tool for testing"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({ "type": "object", "properties": {} })
    }

    async fn call(
        &self,
        _input: hone_tools::ToolInput,
        _ctx: &hone_tools::ToolContext,
    ) -> Result<hone_tools::ToolOutput, hone_tools::ToolError> {
        Ok(hone_tools::ToolOutput::new(serde_json::json!({})))
    }

    fn layer(&self) -> hone_proto::ToolLayer {
        hone_proto::ToolLayer::Native
    }
}

#[test]
fn test_register_and_get() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFileTool));

    let tool = registry.get("read_file").expect("should find read_file");
    assert_eq!(tool.name(), "read_file");
}

#[test]
fn test_get_missing_returns_none() {
    let registry = ToolRegistry::new();
    assert!(registry.get("nonexistent").is_none());
}

#[test]
fn test_schemas_returns_all() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(DummyTool { name: "tool_a" }));
    registry.register(Box::new(DummyTool { name: "tool_b" }));
    registry.register(Box::new(DummyTool { name: "tool_c" }));

    let schemas = registry.schemas();
    assert_eq!(schemas.len(), 3);
}

#[test]
fn test_names_returns_all() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(GlobTool));
    registry.register(Box::new(ReadFileTool));

    let mut names = registry.names();
    names.sort_unstable();

    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
    assert!(names.contains(&"glob"));
    assert_eq!(names.len(), 3);
}
