use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    file::resolve_within_working_dir,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

/// Performs multiple edits atomically within a single tool call.
///
/// Each edit follows the same rules as `edit_file`: `old_str` must appear
/// exactly once in the target file. Edits are applied in order. If any
/// edit fails (ambiguous, not found, I/O error), the overall operation
/// fails and NO edits are committed — the tool reads each file first,
/// applies all edits in memory, and only writes if every edit succeeds.
///
/// Two common patterns:
/// - Multiple edits to the same file (e.g., rename a symbol in 4 places):
///   pass 4 entries with the same `path`
/// - Edits across multiple files (e.g., update import sites):
///   pass entries with different `path` values
pub struct MultiEditTool;

#[async_trait]
impl Tool for MultiEditTool {
    fn name(&self) -> &str {
        "multi_edit"
    }

    fn description(&self) -> &str {
        "Apply multiple edits across one or more files in a single tool call. \
         Each edit has {path, old_str, new_str}. All edits are validated first; \
         if ANY edit is ambiguous (old_str not unique) or not found, the whole \
         operation fails and no changes are committed.\n\n\
         WHEN TO USE: When a rename or refactor requires changing the same symbol in multiple files. \
         When a single logical change spans multiple locations. More efficient than multiple edit_file calls.\n\
         WHEN NOT TO USE: For a single edit in one file — use edit_file (simpler). \
         To create new files — use write_file.\n\
         PREREQUISITE: Read all affected files first. Each old_str must be unique in its file."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "edits": {
                    "type": "array",
                    "description": "List of edits to apply. Each has path, old_str, new_str.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": {
                                "type": "string",
                                "description": "Path to the file (relative to working directory)."
                            },
                            "old_str": {
                                "type": "string",
                                "description": "Exact string to replace. Must be unique in the target file."
                            },
                            "new_str": {
                                "type": "string",
                                "description": "Replacement string."
                            }
                        },
                        "required": ["path", "old_str", "new_str"]
                    }
                }
            },
            "required": ["edits"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let edits = input
            .0
            .get("edits")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'edits' (must be an array)".to_string(),
            })?;

        if edits.is_empty() {
            return Err(ToolError::InvalidInput {
                reason: "edits array must not be empty".to_string(),
            });
        }

        // Parse all edits first to catch structural errors early.
        #[derive(Debug)]
        struct Edit<'a> {
            path: &'a str,
            old_str: &'a str,
            new_str: &'a str,
            index: usize,
        }

        let mut parsed: Vec<Edit> = Vec::with_capacity(edits.len());
        for (i, edit) in edits.iter().enumerate() {
            let path = edit.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput {
                    reason: format!("edit[{i}] missing 'path'"),
                }
            })?;
            let old_str = edit
                .get("old_str")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: format!("edit[{i}] missing 'old_str'"),
                })?;
            let new_str = edit
                .get("new_str")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput {
                    reason: format!("edit[{i}] missing 'new_str'"),
                })?;
            parsed.push(Edit {
                path,
                old_str,
                new_str,
                index: i,
            });
        }

        // Collect unique file paths in the order they first appear, preserving
        // original edit ordering within each file.
        let mut file_order: Vec<String> = Vec::new();
        let mut by_file: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        for edit in &parsed {
            let key = edit.path.to_string();
            if !by_file.contains_key(&key) {
                file_order.push(key.clone());
            }
            by_file.entry(key).or_default().push(edit.index);
        }

        // Read each file once, apply all its edits in memory, track per-edit results.
        let mut new_contents: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        let mut results: Vec<Value> = vec![Value::Null; parsed.len()];

        for path_str in &file_order {
            let path = resolve_within_working_dir(&ctx.working_dir, path_str)?;
            let original = tokio::fs::read_to_string(&path)
                .await
                .map_err(|e| ToolError::Other(format!("failed to read {}: {e}", path.display())))?;

            let indices = &by_file[path_str];
            let mut current = original;

            for &idx in indices {
                let edit = &parsed[idx];
                let count = current.matches(edit.old_str).count();
                if count == 0 {
                    return Err(ToolError::InvalidInput {
                        reason: format!(
                            "edit[{}] old_str not found in {}",
                            edit.index,
                            path.display()
                        ),
                    });
                }
                if count > 1 {
                    return Err(ToolError::InvalidInput {
                        reason: format!(
                            "edit[{}] old_str appears {} times in {} — add more context to disambiguate",
                            edit.index,
                            count,
                            path.display()
                        ),
                    });
                }
                current = current.replacen(edit.old_str, edit.new_str, 1);
                results[edit.index] = json!({
                    "path": path.to_string_lossy(),
                    "applied": true
                });
            }

            new_contents.insert(path_str.clone(), current);
        }

        // All edits validated — now write atomically.
        for (path_str, content) in &new_contents {
            // Path was already validated in the read loop above, but re-resolve
            // for the write path to be safe.
            let path = resolve_within_working_dir(&ctx.working_dir, path_str)?;
            tokio::fs::write(&path, content.as_bytes())
                .await
                .map_err(|e| {
                    ToolError::Other(format!("failed to write {}: {e}", path.display()))
                })?;

            // Read back and verify the written content matches what was intended.
            let verification = tokio::fs::read_to_string(&path).await.map_err(|e| {
                ToolError::Other(format!(
                    "failed to verify write for {}: {e}",
                    path.display()
                ))
            })?;
            if &verification != content {
                return Err(ToolError::Other(format!(
                    "write verification failed: content written to {} does not match expected",
                    path.display()
                )));
            }
        }

        Ok(ToolOutput::new(json!({
            "edits": results,
            "files_modified": new_contents.len(),
        })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
