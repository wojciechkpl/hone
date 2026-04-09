use async_trait::async_trait;
use hone_proto::ToolLayer;
use ignore::WalkBuilder;
use regex::RegexBuilder;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    file::resolve_within_working_dir,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

const DEFAULT_MAX_RESULTS: usize = 100;
const DEFAULT_MAX_FILES: usize = 10_000;
/// Skip files larger than 5 MB to avoid loading huge binaries into memory.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;

/// Search file contents for a regex pattern, honoring .gitignore.
///
/// Returns structured matches `{file, line, content}` sorted by file path
/// then line number. Limits results to avoid context overflow.
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents for a regex pattern. Returns matching lines with \
         file path and line number. Honors .gitignore and .ignore files. Use this \
         to find all uses of a symbol, all TODO comments, all references to a \
         config key, etc. Prefer this over read_file + manual search for content \
         queries."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for. Use \\\\ to escape special chars for literal matching."
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (relative to working directory). Default: current working directory."
                },
                "include": {
                    "type": "string",
                    "description": "Optional glob pattern to filter files (e.g. \"*.rs\", \"src/**/*.py\")."
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case-insensitive matching. Default: false."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matches to return. Default: 100.",
                    "minimum": 1,
                    "maximum": 1000
                }
            },
            "required": ["pattern"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let pattern_str = input
            .get_str("pattern")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'pattern'".to_string(),
            })?
            .to_string();

        let case_insensitive = input.get_bool("case_insensitive").unwrap_or(false);
        let max_results = input
            .get_u64("max_results")
            .unwrap_or(DEFAULT_MAX_RESULTS as u64) as usize;

        let path_arg = input.get_str("path").map(|s| s.to_string());
        let include_arg = input.get_str("include").map(|s| s.to_string());

        let search_root = match path_arg.as_deref() {
            Some(p) => resolve_within_working_dir(&ctx.working_dir, p)?,
            None => ctx.working_dir.clone(),
        };

        // Compile the regex — fail fast before any I/O.
        let regex = RegexBuilder::new(&pattern_str)
            .case_insensitive(case_insensitive)
            .build()
            .map_err(|e| ToolError::InvalidInput {
                reason: format!("invalid regex: {e}"),
            })?;

        // Compile the optional glob-based include filter.
        // We use ignore's OverrideBuilder which interprets globs relative to the
        // search root and integrates with the walker's gitignore logic.
        let include_matcher = if let Some(pat) = &include_arg {
            let mut builder = ignore::overrides::OverrideBuilder::new(&search_root);
            builder.add(pat).map_err(|e| ToolError::InvalidInput {
                reason: format!("invalid include glob '{pat}': {e}"),
            })?;
            Some(builder.build().map_err(|e| ToolError::InvalidInput {
                reason: format!("failed to build include matcher: {e}"),
            })?)
        } else {
            None
        };

        // File traversal is synchronous; run it on the blocking thread pool so we
        // don't stall the async runtime.
        let search_root_clone = search_root.clone();
        let matches =
            tokio::task::spawn_blocking(move || -> Result<Vec<Value>, ToolError> {
                let walker = WalkBuilder::new(&search_root_clone)
                    .standard_filters(true) // respects .gitignore, .ignore, hidden files
                    .build();

                let mut results: Vec<Value> = Vec::new();
                let mut files_scanned: usize = 0;

                'outer: for entry in walker {
                    if results.len() >= max_results || files_scanned >= DEFAULT_MAX_FILES {
                        break;
                    }

                    let Ok(entry) = entry else { continue };
                    if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                        continue;
                    }

                    let path = entry.path();

                    // Apply optional include filter.
                    if let Some(matcher) = &include_matcher {
                        if matcher.matched(path, false).is_ignore() {
                            continue;
                        }
                    }

                    // Skip files that are too large to avoid OOM on huge repos.
                    match std::fs::metadata(path) {
                        Ok(m) if m.len() > MAX_FILE_BYTES => continue,
                        Err(_) => continue,
                        _ => {}
                    }

                    files_scanned += 1;

                    // Attempt to read as UTF-8; silently skip binary files.
                    let content = match std::fs::read_to_string(path) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };

                    for (idx, line) in content.lines().enumerate() {
                        if regex.is_match(line) {
                            let rel_path =
                                path.strip_prefix(&search_root_clone).unwrap_or(path);
                            results.push(json!({
                                "file": rel_path.to_string_lossy(),
                                "line": idx + 1,
                                "content": line,
                            }));
                            if results.len() >= max_results {
                                break 'outer;
                            }
                        }
                    }
                }

                Ok(results)
            })
            .await
            .map_err(|e| ToolError::Other(format!("grep task panicked: {e}")))??;

        let truncated = matches.len() >= max_results;

        Ok(ToolOutput::new(json!({
            "pattern": pattern_str,
            "count": matches.len(),
            "truncated": truncated,
            "matches": matches,
        })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}
