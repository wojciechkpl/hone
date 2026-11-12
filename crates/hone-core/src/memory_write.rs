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

/// Tag prefixes only the runtime sets: provenance, call ids, project scope, internal.
const RESERVED_TAG_PREFIXES: [&str; 4] = ["source:", "call:", "project:", "__"];

/// Calls whose id tags a memory are kept to the newest this many.
pub(crate) const MAX_CALL_TAGS: usize = 5;

fn is_reserved_tag(tag: &str) -> bool {
    let t = tag.trim().to_ascii_lowercase();
    RESERVED_TAG_PREFIXES.iter().any(|p| t.starts_with(p))
}

/// Collapse every run of control characters (newlines, tabs, ...) into one
/// space, so stored content is one line of data and can't add headings or
/// list items to a later prompt.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending = false;
    for c in text.chars() {
        if c.is_control() {
            pending = true;
            continue;
        }
        if pending && !out.is_empty() && !out.ends_with(' ') {
            out.push(' ');
        }
        pending = false;
        out.push(c);
    }
    out
}

/// The `project:<id>` tag for work in `working_dir`: a hash of the git
/// top-level directory when inside a repository, else of the canonical
/// directory. Stable across runs (FNV-1a, not the std hasher).
pub(crate) fn project_tag(working_dir: &std::path::Path) -> String {
    let dir = std::fs::canonicalize(working_dir).unwrap_or_else(|_| working_dir.to_path_buf());
    let root = dir
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(&dir);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in root.to_string_lossy().as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("project:{hash:016x}")
}

/// Keep only the newest [`MAX_CALL_TAGS`] `call:` tags (they are appended in
/// order); other tags keep their places.
pub(crate) fn keep_newest_call_tags(tags: &mut Vec<String>) {
    let calls = tags.iter().filter(|t| t.starts_with("call:")).count();
    let mut drop = calls.saturating_sub(MAX_CALL_TAGS);
    tags.retain(|t| {
        if drop > 0 && t.starts_with("call:") {
            drop -= 1;
            false
        } else {
            true
        }
    });
}

/// True for a memory the model wrote (`source:tool`).
pub(crate) fn is_model_written(tags: &[String]) -> bool {
    tags.iter().any(|t| t == "source:tool")
}

/// True when a memory should be shown in the project of `project` (a
/// [`project_tag`]): anything but a tool memory tagged with another project.
/// A tool memory with no project tag (written before projects were tracked)
/// is shown everywhere.
pub(crate) fn visible_in_project(tags: &[String], project: &str) -> bool {
    !is_model_written(tags)
        || !tags.iter().any(|t| t.starts_with("project:"))
        || tags.iter().any(|t| t == project)
}

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
    let content = match args
        .get("content")
        .and_then(Value::as_str)
        .map(one_line)
        .map(|c| c.trim().to_owned())
    {
        Some(c) if !c.is_empty() && c.chars().count() <= MAX_CONTENT_CHARS => c,
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
                    Some(t) if is_reserved_tag(t) => {
                        return Err(format!(
                            "tags: \"{t}\" is reserved (tags starting with {} are set by the runtime)",
                            RESERVED_TAG_PREFIXES.join(", ")
                        ));
                    }
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
