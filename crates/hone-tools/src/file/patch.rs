use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    file::resolve_within_working_dir,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum PatchError {
    #[error("malformed diff: {0}")]
    Malformed(String),
    #[error("hunk context mismatch in {file} at @@ -{start} +{target}")]
    ContextMismatch {
        file: String,
        start: usize,
        target: usize,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum HunkLine {
    Context(String),
    Add(String),
    Remove(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub source_start: usize,
    pub source_count: usize,
    pub target_start: usize,
    pub target_count: usize,
    pub lines: Vec<HunkLine>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileDiff {
    pub source_path: Option<PathBuf>,
    pub target_path: Option<PathBuf>,
    pub hunks: Vec<Hunk>,
}

// ---------------------------------------------------------------------------
// Pure helpers (public for direct testing)
// ---------------------------------------------------------------------------

/// Parse a unified diff into a list of FileDiff structs.
pub fn parse_unified_diff(diff: &str) -> Result<Vec<FileDiff>, PatchError> {
    if diff.is_empty() {
        return Ok(vec![]);
    }

    let mut results: Vec<FileDiff> = Vec::new();

    // State machine:
    //   Scanning   — looking for a --- line
    //   GotSource  — have source path, looking for +++ line
    //   InFile     — have both paths, collecting hunks
    enum State {
        Scanning,
        GotSource(Option<PathBuf>),
        InFile(FileDiff),
    }

    let mut state = State::Scanning;

    for line in diff.lines() {
        match &mut state {
            State::Scanning => {
                if let Some(rest) = line.strip_prefix("--- ") {
                    let src_path = parse_path(rest);
                    state = State::GotSource(src_path);
                } else if line.starts_with("+++ ") {
                    // +++ without preceding --- is malformed
                    return Err(PatchError::Malformed(
                        "+++ header without preceding --- header".to_string(),
                    ));
                }
                // Skip git extended headers, index lines, etc.
            }
            State::GotSource(src_path) => {
                if let Some(rest) = line.strip_prefix("+++ ") {
                    let tgt_path = parse_path(rest);
                    let fd = FileDiff {
                        source_path: src_path.clone(),
                        target_path: tgt_path,
                        hunks: Vec::new(),
                    };
                    state = State::InFile(fd);
                } else {
                    return Err(PatchError::Malformed(format!(
                        "expected +++ line after --- line, got: {line}"
                    )));
                }
            }
            State::InFile(fd) => {
                if line.starts_with("@@ ") {
                    let hunk = parse_hunk_header(line)?;
                    fd.hunks.push(hunk);
                } else if let Some(rest) = line.strip_prefix("--- ") {
                    // Start of the next file diff
                    let finished = std::mem::replace(
                        fd,
                        FileDiff {
                            source_path: None,
                            target_path: None,
                            hunks: Vec::new(),
                        },
                    );
                    results.push(finished);
                    let src_path = parse_path(rest);
                    state = State::GotSource(src_path);
                } else if line.starts_with('+')
                    || line.starts_with('-')
                    || line.starts_with(' ')
                {
                    // Hunk body line — must have an active hunk
                    match fd.hunks.last_mut() {
                        Some(hunk) => {
                            if let Some(rest) = line.strip_prefix('+') {
                                hunk.lines.push(HunkLine::Add(rest.to_string()));
                            } else if let Some(rest) = line.strip_prefix('-') {
                                hunk.lines.push(HunkLine::Remove(rest.to_string()));
                            } else {
                                // context line (starts with ' ')
                                hunk.lines.push(HunkLine::Context(line[1..].to_string()));
                            }
                        }
                        None => {
                            return Err(PatchError::Malformed(format!(
                                "hunk body line with no preceding @@ header: {line}"
                            )));
                        }
                    }
                }
                // Ignore other lines (e.g. "\ No newline at end of file")
            }
        }
    }

    // Flush any in-progress file diff
    if let State::InFile(fd) = state {
        results.push(fd);
    } else if let State::GotSource(_) = state {
        return Err(PatchError::Malformed(
            "diff ended after --- line without +++ line".to_string(),
        ));
    }

    Ok(results)
}

/// Parse a path from after the `--- ` or `+++ ` prefix.
/// Strips `a/` / `b/` prefixes; returns None for `/dev/null`.
fn parse_path(rest: &str) -> Option<PathBuf> {
    // There may be a tab followed by a timestamp — take only up to the first tab
    let path_part = rest.split('\t').next().unwrap_or(rest).trim();
    if path_part == "/dev/null" {
        return None;
    }
    let stripped = if let Some(s) = path_part.strip_prefix("a/") {
        s
    } else if let Some(s) = path_part.strip_prefix("b/") {
        s
    } else {
        path_part
    };
    Some(PathBuf::from(stripped))
}

/// Parse a hunk header line like `@@ -1,3 +1,3 @@ optional context`.
fn parse_hunk_header(line: &str) -> Result<Hunk, PatchError> {
    // Format: @@ -<src_start>[,<src_count>] +<tgt_start>[,<tgt_count>] @@
    let inner = line
        .strip_prefix("@@ ")
        .ok_or_else(|| PatchError::Malformed(format!("invalid hunk header: {line}")))?;

    // Find the closing @@
    let end = inner
        .find(" @@")
        .ok_or_else(|| PatchError::Malformed(format!("invalid hunk header (no closing @@): {line}")))?;

    let ranges = &inner[..end];
    // ranges = "-<src> +<tgt>"
    let parts: Vec<&str> = ranges.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(PatchError::Malformed(format!(
            "invalid hunk header ranges: {ranges}"
        )));
    }

    let (source_start, source_count) = parse_range(parts[0], '-')?;
    let (target_start, target_count) = parse_range(parts[1], '+')?;

    Ok(Hunk {
        source_start,
        source_count,
        target_start,
        target_count,
        lines: Vec::new(),
    })
}

/// Parse `-<start>[,<count>]` or `+<start>[,<count>]`.
/// Count defaults to 1 when omitted.
fn parse_range(s: &str, prefix: char) -> Result<(usize, usize), PatchError> {
    let s = s
        .strip_prefix(prefix)
        .ok_or_else(|| PatchError::Malformed(format!("expected '{prefix}' prefix in range: {s}")))?;

    if let Some((start_s, count_s)) = s.split_once(',') {
        let start = start_s.parse::<usize>().map_err(|_| {
            PatchError::Malformed(format!("invalid range start: {start_s}"))
        })?;
        let count = count_s.parse::<usize>().map_err(|_| {
            PatchError::Malformed(format!("invalid range count: {count_s}"))
        })?;
        Ok((start, count))
    } else {
        let start = s.parse::<usize>().map_err(|_| {
            PatchError::Malformed(format!("invalid range value: {s}"))
        })?;
        Ok((start, 1))
    }
}

/// Apply a single FileDiff to file content in-memory.
pub fn apply_to_content(original: &str, file_diff: &FileDiff) -> Result<String, PatchError> {
    // Split original into lines (preserve content, trailing newline awareness)
    let original_lines: Vec<&str> = original.lines().collect();
    let has_trailing_newline = original.ends_with('\n');

    let mut result_lines: Vec<String> = original_lines.iter().map(|s| s.to_string()).collect();

    // We apply hunks sequentially. Each hunk's source_start is 1-indexed relative
    // to the ORIGINAL file. We track offset = (inserted lines - deleted lines) from
    // prior hunks so we know where in the growing result_lines to apply the next hunk.
    let mut offset: isize = 0;

    let file_name = file_diff
        .source_path
        .as_ref()
        .or(file_diff.target_path.as_ref())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "<unknown>".to_string());

    for hunk in &file_diff.hunks {
        // source_start is 1-indexed; 0 means "before the first line" (new file case)
        let base_idx = if hunk.source_start == 0 {
            0isize
        } else {
            hunk.source_start as isize - 1
        };
        let actual_idx = base_idx + offset;

        // Walk through hunk lines, building replacement
        let mut read_pos = actual_idx as usize; // position in result_lines
        let mut additions: Vec<String> = Vec::new();

        for hl in &hunk.lines {
            match hl {
                HunkLine::Context(expected) => {
                    // Verify context line matches
                    let actual = result_lines.get(read_pos).map(String::as_str).unwrap_or("");
                    if actual != expected.as_str() {
                        return Err(PatchError::ContextMismatch {
                            file: file_name.clone(),
                            start: hunk.source_start,
                            target: hunk.target_start,
                        });
                    }
                    additions.push(expected.clone());
                    read_pos += 1;
                    // Context lines are consumed from source AND kept in target
                }
                HunkLine::Remove(expected) => {
                    // Verify remove line matches
                    let actual = result_lines.get(read_pos).map(String::as_str).unwrap_or("");
                    if actual != expected.as_str() {
                        return Err(PatchError::ContextMismatch {
                            file: file_name.clone(),
                            start: hunk.source_start,
                            target: hunk.target_start,
                        });
                    }
                    read_pos += 1;
                    // Remove: do NOT add to additions
                }
                HunkLine::Add(new_line) => {
                    additions.push(new_line.clone());
                    // Add: does NOT consume a source line
                }
            }
        }

        // The span we're replacing in result_lines starts at actual_idx and covers
        // (context + removes) lines. We replace it with additions.
        let span = read_pos - actual_idx as usize;
        let insert_start = actual_idx as usize;

        // Splice: remove span lines starting at insert_start, insert additions
        let new_count = additions.len() as isize;
        let old_count = span as isize;
        result_lines.splice(insert_start..insert_start + span, additions);

        offset += new_count - old_count;
    }

    // Reconstruct the string
    let mut out = result_lines.join("\n");
    if (has_trailing_newline || !out.is_empty()) && !result_lines.is_empty() {
        out.push('\n');
    }

    Ok(out)
}

// ---------------------------------------------------------------------------
// Tool struct
// ---------------------------------------------------------------------------

pub struct PatchTool;

#[async_trait]
impl Tool for PatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply a unified diff to one or more files atomically. Rolls back all changes on any hunk conflict."
    }

    fn is_read_only(&self) -> bool {
        false
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "diff": {
                    "type": "string",
                    "description": "Unified diff text (output of `diff -u` or `git diff`)"
                }
            },
            "required": ["diff"]
        })
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let diff_str = input
            .get_str("diff")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'diff'".to_string(),
            })?;

        let file_diffs = parse_unified_diff(diff_str)
            .map_err(|e| ToolError::Other(e.to_string()))?;

        if file_diffs.is_empty() && !diff_str.trim().is_empty() {
            return Err(ToolError::Other(
                "diff text contains no file diffs".to_string(),
            ));
        }

        // Snapshot phase: read all existing files into memory
        // key = resolved absolute path; value = original content (None = doesn't exist yet)
        let mut snapshot: HashMap<PathBuf, Option<String>> = HashMap::new();
        // Ordered list of (resolved_path, Option<new_content>) — None means delete
        let mut operations: Vec<(PathBuf, Option<String>)> = Vec::new();

        let mut total_hunks: usize = 0;

        for fd in &file_diffs {
            total_hunks += fd.hunks.len();

            let resolved_path = match (&fd.source_path, &fd.target_path) {
                (None, Some(tgt)) => {
                    // New file — resolve target path
                    let path_str = tgt.to_string_lossy();
                    resolve_within_working_dir(&ctx.working_dir, &path_str)?
                }
                (Some(src), _) => {
                    let path_str = src.to_string_lossy();
                    resolve_within_working_dir(&ctx.working_dir, &path_str)?
                }
                (None, None) => {
                    return Err(ToolError::Other(
                        "file diff has neither source nor target path".to_string(),
                    ));
                }
            };

            // Read original content (empty for new files)
            let original = if fd.source_path.is_none() {
                // New file — start from empty
                String::new()
            } else if resolved_path.exists() {
                tokio::fs::read_to_string(&resolved_path).await?
            } else {
                String::new()
            };

            snapshot.entry(resolved_path.clone()).or_insert_with(|| Some(original.clone()));

            // Apply the diff in memory
            if fd.target_path.is_none() {
                // Deletion
                operations.push((resolved_path, None));
            } else {
                let new_content = apply_to_content(&original, fd)
                    .map_err(|e| ToolError::Other(e.to_string()))?;
                operations.push((resolved_path, Some(new_content)));
            }
        }

        // Write phase: all in-memory applies succeeded, now write to disk
        let mut files_changed: Vec<String> = Vec::new();

        for (path, new_content) in &operations {
            match new_content {
                Some(content) => {
                    // Ensure parent directory exists
                    if let Some(parent) = path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(path, content.as_bytes()).await?;
                }
                None => {
                    // Delete the file
                    tokio::fs::remove_file(path).await?;
                }
            }
            files_changed.push(path.to_string_lossy().into_owned());
        }

        Ok(ToolOutput::new(json!({
            "applied": true,
            "files_changed": files_changed,
            "hunks_applied": total_hunks,
        })))
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::TempDir;

    // ------------------------------------------------------------------
    // Parsing tests
    // ------------------------------------------------------------------

    #[test]
    fn parse_empty_diff_returns_empty_vec() {
        let result = parse_unified_diff("").unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn parse_single_file_single_hunk() {
        let diff = "--- a/f\n+++ b/f\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n";
        let diffs = parse_unified_diff(diff).unwrap();
        assert_eq!(diffs.len(), 1);
        let fd = &diffs[0];
        assert_eq!(fd.source_path, Some(PathBuf::from("f")));
        assert_eq!(fd.target_path, Some(PathBuf::from("f")));
        assert_eq!(fd.hunks.len(), 1);
        let hunk = &fd.hunks[0];
        assert_eq!(hunk.source_start, 1);
        assert_eq!(hunk.source_count, 3);
        assert_eq!(hunk.target_start, 1);
        assert_eq!(hunk.target_count, 3);
        assert_eq!(
            hunk.lines,
            vec![
                HunkLine::Context("a".to_string()),
                HunkLine::Remove("b".to_string()),
                HunkLine::Add("B".to_string()),
                HunkLine::Context("c".to_string()),
            ]
        );
    }

    #[test]
    fn parse_strips_a_b_prefixes() {
        let diff = "--- a/src/foo.rs\n+++ b/src/foo.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        let diffs = parse_unified_diff(diff).unwrap();
        assert_eq!(diffs[0].source_path, Some(PathBuf::from("src/foo.rs")));
        assert_eq!(diffs[0].target_path, Some(PathBuf::from("src/foo.rs")));
    }

    #[test]
    fn parse_dev_null_source_marks_new_file() {
        let diff = "--- /dev/null\n+++ b/new_file.txt\n@@ -0,0 +1,2 @@\n+line1\n+line2\n";
        let diffs = parse_unified_diff(diff).unwrap();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].source_path, None);
        assert_eq!(diffs[0].target_path, Some(PathBuf::from("new_file.txt")));
    }

    #[test]
    fn parse_dev_null_target_marks_deletion() {
        let diff = "--- a/old_file.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-line1\n-line2\n";
        let diffs = parse_unified_diff(diff).unwrap();
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].source_path, Some(PathBuf::from("old_file.txt")));
        assert_eq!(diffs[0].target_path, None);
    }

    #[test]
    fn parse_multiple_files_in_one_diff() {
        let diff = concat!(
            "--- a/alpha.rs\n+++ b/alpha.rs\n@@ -1,1 +1,1 @@\n-a\n+A\n",
            "--- a/beta.rs\n+++ b/beta.rs\n@@ -1,1 +1,1 @@\n-b\n+B\n",
            "--- a/gamma.rs\n+++ b/gamma.rs\n@@ -1,1 +1,1 @@\n-g\n+G\n",
        );
        let diffs = parse_unified_diff(diff).unwrap();
        assert_eq!(diffs.len(), 3);
        assert_eq!(diffs[0].source_path, Some(PathBuf::from("alpha.rs")));
        assert_eq!(diffs[1].source_path, Some(PathBuf::from("beta.rs")));
        assert_eq!(diffs[2].source_path, Some(PathBuf::from("gamma.rs")));
    }

    #[test]
    fn parse_hunk_count_omitted_defaults_to_one() {
        let diff = "--- a/f.txt\n+++ b/f.txt\n@@ -5 +5 @@\n context\n";
        let diffs = parse_unified_diff(diff).unwrap();
        let hunk = &diffs[0].hunks[0];
        assert_eq!(hunk.source_start, 5);
        assert_eq!(hunk.source_count, 1);
        assert_eq!(hunk.target_start, 5);
        assert_eq!(hunk.target_count, 1);
    }

    #[test]
    fn parse_malformed_header_errors() {
        // +++ line with no preceding --- line is malformed
        let diff = "+++ b/foo.rs\n@@ -1,1 +1,1 @@\n-old\n+new\n";
        let result = parse_unified_diff(diff);
        assert!(matches!(result, Err(PatchError::Malformed(_))));
    }

    #[test]
    fn parse_hunk_without_at_at_errors() {
        // A file header with no @@ hunk header following it
        let diff = "--- a/foo.rs\n+++ b/foo.rs\n-old line\n+new line\n";
        let result = parse_unified_diff(diff);
        assert!(matches!(result, Err(PatchError::Malformed(_))));
    }

    // ------------------------------------------------------------------
    // apply_to_content (pure, in-memory) tests
    // ------------------------------------------------------------------

    fn make_file_diff(
        source: &str,
        target: &str,
        hunks: Vec<Hunk>,
    ) -> FileDiff {
        FileDiff {
            source_path: Some(PathBuf::from(source)),
            target_path: Some(PathBuf::from(target)),
            hunks,
        }
    }

    #[test]
    fn apply_simple_replace() {
        let original = "a\nb\nc\n";
        let fd = make_file_diff(
            "f",
            "f",
            vec![Hunk {
                source_start: 1,
                source_count: 3,
                target_start: 1,
                target_count: 3,
                lines: vec![
                    HunkLine::Context("a".to_string()),
                    HunkLine::Remove("b".to_string()),
                    HunkLine::Add("B".to_string()),
                    HunkLine::Context("c".to_string()),
                ],
            }],
        );
        let result = apply_to_content(original, &fd).unwrap();
        assert_eq!(result, "a\nB\nc\n");
    }

    #[test]
    fn apply_pure_addition_at_start() {
        let original = "existing\n";
        let fd = make_file_diff(
            "f",
            "f",
            vec![Hunk {
                source_start: 1,
                source_count: 1,
                target_start: 1,
                target_count: 3,
                lines: vec![
                    HunkLine::Add("prepended1\n".trim_end_matches('\n').to_string()),
                    HunkLine::Add("prepended2\n".trim_end_matches('\n').to_string()),
                    HunkLine::Context("existing".to_string()),
                ],
            }],
        );
        let result = apply_to_content(original, &fd).unwrap();
        assert_eq!(result, "prepended1\nprepended2\nexisting\n");
    }

    #[test]
    fn apply_pure_deletion() {
        let original = "keep\ndelete_me\nalso_keep\n";
        let fd = make_file_diff(
            "f",
            "f",
            vec![Hunk {
                source_start: 1,
                source_count: 3,
                target_start: 1,
                target_count: 2,
                lines: vec![
                    HunkLine::Context("keep".to_string()),
                    HunkLine::Remove("delete_me".to_string()),
                    HunkLine::Context("also_keep".to_string()),
                ],
            }],
        );
        let result = apply_to_content(original, &fd).unwrap();
        assert_eq!(result, "keep\nalso_keep\n");
    }

    #[test]
    fn apply_with_trailing_newline_preserved() {
        let original = "line1\nline2\n";
        let fd = make_file_diff(
            "f",
            "f",
            vec![Hunk {
                source_start: 1,
                source_count: 2,
                target_start: 1,
                target_count: 2,
                lines: vec![
                    HunkLine::Remove("line1".to_string()),
                    HunkLine::Add("LINE1".to_string()),
                    HunkLine::Context("line2".to_string()),
                ],
            }],
        );
        let result = apply_to_content(original, &fd).unwrap();
        assert_eq!(result, "LINE1\nline2\n");
    }

    #[test]
    fn apply_context_mismatch_returns_error() {
        // File has "B" but hunk context says "b" (lowercase)
        let original = "a\nB\nc\n";
        let fd = make_file_diff(
            "f.txt",
            "f.txt",
            vec![Hunk {
                source_start: 1,
                source_count: 3,
                target_start: 1,
                target_count: 3,
                lines: vec![
                    HunkLine::Context("a".to_string()),
                    HunkLine::Remove("b".to_string()), // expects lowercase; file has "B"
                    HunkLine::Add("X".to_string()),
                    HunkLine::Context("c".to_string()),
                ],
            }],
        );
        let result = apply_to_content(original, &fd);
        assert!(matches!(result, Err(PatchError::ContextMismatch { .. })));
    }

    #[test]
    fn apply_multiple_hunks_in_order() {
        // Line numbers in BOTH hunks are relative to the original file.
        let original = "1\n2\n3\n4\n5\n6\n7\n8\n";
        let fd = make_file_diff(
            "f",
            "f",
            vec![
                // Replace line 2 ("2" -> "TWO")
                Hunk {
                    source_start: 1,
                    source_count: 3,
                    target_start: 1,
                    target_count: 3,
                    lines: vec![
                        HunkLine::Context("1".to_string()),
                        HunkLine::Remove("2".to_string()),
                        HunkLine::Add("TWO".to_string()),
                        HunkLine::Context("3".to_string()),
                    ],
                },
                // Replace line 7 ("7" -> "SEVEN")
                Hunk {
                    source_start: 6,
                    source_count: 3,
                    target_start: 6,
                    target_count: 3,
                    lines: vec![
                        HunkLine::Context("6".to_string()),
                        HunkLine::Remove("7".to_string()),
                        HunkLine::Add("SEVEN".to_string()),
                        HunkLine::Context("8".to_string()),
                    ],
                },
            ],
        );
        let result = apply_to_content(original, &fd).unwrap();
        assert_eq!(result, "1\nTWO\n3\n4\n5\n6\nSEVEN\n8\n");
    }

    #[test]
    fn apply_empty_hunks_returns_unchanged_content() {
        let original = "hello\nworld\n";
        let fd = make_file_diff("f", "f", vec![]);
        let result = apply_to_content(original, &fd).unwrap();
        assert_eq!(result, original);
    }

    // ------------------------------------------------------------------
    // Tool integration tests
    // ------------------------------------------------------------------

    fn make_tool_context(dir: &TempDir) -> ToolContext {
        use hone_sandbox::NullSandbox;
        ToolContext {
            sandbox: Arc::new(NullSandbox),
            working_dir: dir.path().to_path_buf(),
        }
    }

    #[test]
    fn tool_name_and_schema() {
        let tool = PatchTool;
        assert_eq!(tool.name(), "apply_patch");
        let schema = tool.schema();
        // "diff" must be in required
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("diff")));
        // "diff" property must exist
        assert!(schema["properties"]["diff"].is_object());
    }

    #[test]
    fn tool_is_not_read_only() {
        assert!(!PatchTool.is_read_only());
    }

    #[tokio::test]
    async fn tool_call_modifies_existing_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("hello.txt");
        std::fs::write(&file_path, "foo\nbar\nbaz\n").unwrap();

        let diff = "--- a/hello.txt\n+++ b/hello.txt\n@@ -1,3 +1,3 @@\n foo\n-bar\n+BAR\n baz\n";
        let input = ToolInput::new(json!({ "diff": diff }));
        let ctx = make_tool_context(&dir);

        let output = PatchTool.call(input, &ctx).await.unwrap();
        let val = output.into_value();
        assert_eq!(val["applied"], json!(true));
        assert_eq!(val["hunks_applied"], json!(1));
        let files: Vec<&str> = val["files_changed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(files.iter().any(|f| f.contains("hello.txt")));

        let content = std::fs::read_to_string(&file_path).unwrap();
        assert_eq!(content, "foo\nBAR\nbaz\n");
    }

    #[tokio::test]
    async fn tool_call_creates_new_file() {
        let dir = TempDir::new().unwrap();
        let diff =
            "--- /dev/null\n+++ b/created.txt\n@@ -0,0 +1,2 @@\n+first line\n+second line\n";
        let input = ToolInput::new(json!({ "diff": diff }));
        let ctx = make_tool_context(&dir);

        let output = PatchTool.call(input, &ctx).await.unwrap();
        assert_eq!(output.value["applied"], json!(true));

        let content = std::fs::read_to_string(dir.path().join("created.txt")).unwrap();
        assert_eq!(content, "first line\nsecond line\n");
    }

    #[tokio::test]
    async fn tool_call_deletes_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("to_delete.txt");
        std::fs::write(&file_path, "line1\nline2\n").unwrap();

        let diff =
            "--- a/to_delete.txt\n+++ /dev/null\n@@ -1,2 +0,0 @@\n-line1\n-line2\n";
        let input = ToolInput::new(json!({ "diff": diff }));
        let ctx = make_tool_context(&dir);

        let output = PatchTool.call(input, &ctx).await.unwrap();
        assert_eq!(output.value["applied"], json!(true));
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn tool_call_atomic_rollback_on_conflict() {
        let dir = TempDir::new().unwrap();
        // first.txt applies cleanly
        std::fs::write(dir.path().join("first.txt"), "aaa\nbbb\nccc\n").unwrap();
        // second.txt has wrong context — will fail
        std::fs::write(dir.path().join("second.txt"), "xxx\nYYY\nzzz\n").unwrap();

        let diff = concat!(
            "--- a/first.txt\n+++ b/first.txt\n@@ -1,3 +1,3 @@\n aaa\n-bbb\n+BBB\n ccc\n",
            // context says "yyy" but file has "YYY" — mismatch
            "--- a/second.txt\n+++ b/second.txt\n@@ -1,3 +1,3 @@\n xxx\n-yyy\n+ZZZ\n zzz\n",
        );
        let input = ToolInput::new(json!({ "diff": diff }));
        let ctx = make_tool_context(&dir);

        let result = PatchTool.call(input, &ctx).await;
        assert!(result.is_err());

        // first.txt must be ROLLED BACK to original content
        let first_content = std::fs::read_to_string(dir.path().join("first.txt")).unwrap();
        assert_eq!(first_content, "aaa\nbbb\nccc\n");
    }

    #[tokio::test]
    async fn tool_call_returns_files_changed_in_order() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("one.txt"), "a\n").unwrap();
        std::fs::write(dir.path().join("two.txt"), "b\n").unwrap();
        std::fs::write(dir.path().join("three.txt"), "c\n").unwrap();

        let diff = concat!(
            "--- a/one.txt\n+++ b/one.txt\n@@ -1,1 +1,1 @@\n-a\n+A\n",
            "--- a/two.txt\n+++ b/two.txt\n@@ -1,1 +1,1 @@\n-b\n+B\n",
            "--- a/three.txt\n+++ b/three.txt\n@@ -1,1 +1,1 @@\n-c\n+C\n",
        );
        let input = ToolInput::new(json!({ "diff": diff }));
        let ctx = make_tool_context(&dir);

        let output = PatchTool.call(input, &ctx).await.unwrap();
        let files: Vec<&str> = output.value["files_changed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(files.len(), 3);
        assert!(files[0].contains("one.txt"));
        assert!(files[1].contains("two.txt"));
        assert!(files[2].contains("three.txt"));
    }

    #[tokio::test]
    async fn tool_call_invalid_diff_returns_tool_error() {
        let dir = TempDir::new().unwrap();
        let input = ToolInput::new(json!({ "diff": "not a diff at all" }));
        let ctx = make_tool_context(&dir);
        let result = PatchTool.call(input, &ctx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn tool_call_missing_diff_arg_returns_tool_error() {
        let dir = TempDir::new().unwrap();
        let input = ToolInput::new(json!({}));
        let ctx = make_tool_context(&dir);
        let result = PatchTool.call(input, &ctx).await;
        assert!(result.is_err());
    }
}
