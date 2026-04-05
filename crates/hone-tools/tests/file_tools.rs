use std::sync::Arc;

use hone_sandbox::NullSandbox;
use hone_tools::{EditFileTool, GlobTool, ReadFileTool, Tool, ToolContext, ToolInput, WriteFileTool};
use serde_json::json;
use tempfile::TempDir;

fn make_ctx(dir: &TempDir) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        working_dir: dir.path().to_path_buf(),
    }
}

// ---------------------------------------------------------------------------
// ReadFileTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_file_returns_content() {
    let dir = TempDir::new().unwrap();
    let file_path = dir.path().join("hello.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let tool = ReadFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "hello.txt" }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    assert_eq!(output.0["content"], "hello world");
}

#[tokio::test]
async fn test_read_file_nonexistent_errors() {
    let dir = TempDir::new().unwrap();
    let tool = ReadFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "does_not_exist.txt" }));

    let err = tool.call(input, &ctx).await.expect_err("should error");
    assert!(
        matches!(err, hone_tools::ToolError::IoError(_)),
        "expected IoError, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// WriteFileTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_write_file_creates_file() {
    let dir = TempDir::new().unwrap();
    let tool = WriteFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "new_file.txt", "content": "created" }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    assert_eq!(output.0["bytes_written"], 7);

    let on_disk = std::fs::read_to_string(dir.path().join("new_file.txt")).unwrap();
    assert_eq!(on_disk, "created");
}

#[tokio::test]
async fn test_write_file_overwrites_existing() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("existing.txt"), "old content").unwrap();

    let tool = WriteFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "existing.txt", "content": "new content" }));

    tool.call(input, &ctx).await.expect("call should succeed");

    let on_disk = std::fs::read_to_string(dir.path().join("existing.txt")).unwrap();
    assert_eq!(on_disk, "new content");
}

// ---------------------------------------------------------------------------
// EditFileTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_edit_file_applies_replacement() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("edit_me.txt"), "foo bar baz").unwrap();

    let tool = EditFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "path": "edit_me.txt",
        "old_str": "bar",
        "new_str": "QUX"
    }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    assert_eq!(output.0["applied"], true);

    let on_disk = std::fs::read_to_string(dir.path().join("edit_me.txt")).unwrap();
    assert_eq!(on_disk, "foo QUX baz");
}

#[tokio::test]
async fn test_edit_file_no_match_returns_not_applied() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("no_match.txt"), "hello world").unwrap();

    let tool = EditFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "path": "no_match.txt",
        "old_str": "NONEXISTENT",
        "new_str": "replacement"
    }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    assert_eq!(output.0["applied"], false);

    // File should be unchanged.
    let on_disk = std::fs::read_to_string(dir.path().join("no_match.txt")).unwrap();
    assert_eq!(on_disk, "hello world");
}

// ---------------------------------------------------------------------------
// GlobTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_glob_finds_matching_files() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("alpha.rs"), "// rs").unwrap();
    std::fs::write(dir.path().join("beta.rs"), "// rs").unwrap();
    std::fs::write(dir.path().join("gamma.txt"), "txt").unwrap();

    let tool = GlobTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "pattern": "*.rs" }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    let files = output.0["files"]
        .as_array()
        .expect("files should be an array");

    let names: Vec<&str> = files
        .iter()
        .filter_map(|v| v.as_str())
        .map(|p| {
            std::path::Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(p)
        })
        .collect();

    assert!(names.contains(&"alpha.rs"), "alpha.rs not found: {names:?}");
    assert!(names.contains(&"beta.rs"), "beta.rs not found: {names:?}");
    assert!(
        !names.contains(&"gamma.txt"),
        "gamma.txt should not be matched: {names:?}"
    );
}
