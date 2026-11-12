//! The `memory_write` tool: the model proposes a memory, the agent applies it.
//!
//! The registered [`MemoryWriteTool`] only describes the tool (schema, risk
//! class, permission subject). The agent executes the call itself, so dedup,
//! attribution tags, the session cap, approval and the `MemoryWritten` event
//! stay in one place. See `docs/plans/memory-write-tool-spec.md`.

use async_trait::async_trait;
use hone_config::policy::Subject;
use hone_memory::MemoryKind;
use hone_proto::ToolLayer;
use hone_tools::{
    SubjectContext, Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolSubjects,
};
use serde_json::{Value, json};

/// The tool's name.
pub(crate) const NAME: &str = "memory_write";
/// Successful writes allowed per session.
pub(crate) const MAX_WRITES_PER_SESSION: usize = 20;

const MAX_CONTENT_CHARS: usize = 2000;
const MAX_TAGS: usize = 8;
const MAX_TAG_CHARS: usize = 40;

/// A checked `memory_write` input.
#[derive(Debug)]
pub(crate) struct Validated {
    pub kind: MemoryKind,
    pub content: String,
    pub tags: Vec<String>,
}

/// Check the input. The error text names the offending field.
pub(crate) fn validate(args: &Value) -> Result<Validated, String> {
    let kind = match args.get("kind").and_then(Value::as_str) {
        Some("user") => MemoryKind::User,
        Some("feedback") => MemoryKind::Feedback,
        Some("project") => MemoryKind::Project,
        Some("reference") => MemoryKind::Reference,
        _ => {
            return Err("kind must be one of: user, feedback, project, reference".to_owned());
        }
    };
    let content = match args.get("content").and_then(Value::as_str).map(str::trim) {
        Some(c) if !c.is_empty() && c.chars().count() <= MAX_CONTENT_CHARS => c.to_owned(),
        _ => {
            return Err(format!(
                "content must be text of 1 to {MAX_CONTENT_CHARS} characters"
            ));
        }
    };
    let tags = match args.get("tags") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut tags = Vec::new();
            for item in items {
                match item.as_str() {
                    Some(t) if t.chars().count() <= MAX_TAG_CHARS => tags.push(t.to_owned()),
                    _ => {
                        return Err(format!(
                            "tags must be at most {MAX_TAGS} strings of at most {MAX_TAG_CHARS} characters"
                        ));
                    }
                }
            }
            if tags.len() > MAX_TAGS {
                return Err(format!(
                    "tags must be at most {MAX_TAGS} strings of at most {MAX_TAG_CHARS} characters"
                ));
            }
            tags
        }
        Some(_) => {
            return Err(format!(
                "tags must be an array of at most {MAX_TAGS} strings of at most {MAX_TAG_CHARS} characters"
            ));
        }
    };
    Ok(Validated {
        kind,
        content,
        tags,
    })
}

/// The registered face of `memory_write`.
pub(crate) struct MemoryWriteTool;

#[async_trait]
impl Tool for MemoryWriteTool {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> &str {
        "Keep a durable note for future sessions: a fact you learned, a preference the \
         user stated, a decision, or a pointer to where something lives. Not for \
         scratch work or things visible in the repository. kind is user, feedback, \
         project or reference; content is one self-contained statement (at most 2000 \
         characters). A near-duplicate of an existing memory updates it."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kind": {
                    "type": "string",
                    "enum": ["user", "feedback", "project", "reference"],
                    "description": "What the memory is about."
                },
                "content": {
                    "type": "string",
                    "description": "The memory, 1-2000 characters."
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string", "maxLength": 40 },
                    "maxItems": 8,
                    "description": "Optional short labels."
                }
            },
            "required": ["kind", "content"]
        })
    }

    async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        // The agent applies the write itself (it needs the call id, the store,
        // the session cap and the event channel).
        Err(ToolError::Other(
            "memory_write is applied by the agent".to_owned(),
        ))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn subjects(&self, _input: &ToolInput, _sctx: &SubjectContext) -> ToolSubjects {
        ToolSubjects {
            subjects: vec![Subject::Command(NAME.to_owned())],
            ..Default::default()
        }
    }
}
