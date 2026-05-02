//! `memory_read` tool — fetch the full body of a memory by id.
//!
//! The agent's system prompt contains a compact `## Memory index` section that
//! lists memory ids in brackets.  When one of those ids looks relevant the
//! agent calls this tool with the id to retrieve the full content.

use std::sync::Arc;

use async_trait::async_trait;
use hone_memory::MemoryStore;
use hone_proto::ToolLayer;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput};
use serde_json::{Value, json};

pub struct MemoryReadTool {
    store: Arc<dyn MemoryStore>,
}

impl MemoryReadTool {
    pub fn new(store: Arc<dyn MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for MemoryReadTool {
    fn name(&self) -> &str {
        "memory_read"
    }

    fn description(&self) -> &str {
        "Fetch the body of a memory by id. The system prompt's `## Memory index` \
         section lists available memory ids in [brackets]. Call this when one of \
         those memories looks relevant. Returns `found: false` if the id is unknown \
         or the memory has been forgotten/decayed."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": { "type": "string", "description": "The memory id from the index." }
            },
            "required": ["id"]
        })
    }

    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let id = input.get_str("id").ok_or_else(|| ToolError::InvalidInput {
            reason: "missing required field 'id'".to_string(),
        })?;

        if id.is_empty() {
            return Err(ToolError::InvalidInput {
                reason: "'id' must not be empty".to_string(),
            });
        }

        match hone_core::memory_index::read_memory(self.store.as_ref(), id).await {
            Ok(Some(mem)) => {
                let kind_str = serde_json::to_value(mem.kind)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_else(|| "unknown".to_string());
                Ok(ToolOutput::new(json!({
                    "found": true,
                    "id": mem.id,
                    "content": mem.content,
                    "kind": kind_str,
                    "description": mem.description,
                    "weight": mem.weight,
                    "tags": mem.tags,
                })))
            }
            Ok(None) => Ok(ToolOutput::new(json!({ "id": id, "found": false }))),
            Err(e) => Err(ToolError::Other(format!("memory_read failed: {e}"))),
        }
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
