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
    // Content is now line-numbered; the single line "hello world" should appear.
    let content = output.0["content"].as_str().expect("content is a string");
    assert!(
        content.contains("hello world"),
        "expected 'hello world' in content, got: {content:?}"
    );
    assert!(
        content.contains("1 |"),
        "expected line number prefix in content, got: {content:?}"
    );
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

#[tokio::test]
async fn test_read_file_returns_line_numbered_content() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("three.txt"), "alpha\nbeta\ngamma").unwrap();

    let tool = ReadFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "three.txt" }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    let content = output.0["content"].as_str().expect("content is a string");
    assert!(content.contains("1 |"), "missing line 1 prefix");
    assert!(content.contains("2 |"), "missing line 2 prefix");
    assert!(content.contains("3 |"), "missing line 3 prefix");
    assert!(content.contains("alpha"), "missing 'alpha'");
    assert!(content.contains("beta"), "missing 'beta'");
    assert!(content.contains("gamma"), "missing 'gamma'");
}

#[tokio::test]
async fn test_read_file_respects_offset() {
    let dir = TempDir::new().unwrap();
    let lines: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    std::fs::write(dir.path().join("ten.txt"), &lines).unwrap();

    let tool = ReadFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "ten.txt", "offset": 5 }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    let content = output.0["content"].as_str().expect("content is a string");

    // First returned line should be line 5.
    let first_line = content.lines().next().unwrap_or("");
    assert!(
        first_line.contains("5 |"),
        "expected first line to be line 5, got: {first_line:?}"
    );
    assert!(
        first_line.contains("line5"),
        "expected 'line5' in first output line, got: {first_line:?}"
    );
    // Lines before offset must not appear. Use exact line content with leading "1 |"
    // pattern to avoid "line1" matching "line10".
    assert!(
        !content.contains("1 | line1\n"),
        "line1 should not appear when offset=5"
    );
    assert_eq!(output.0["start_line"], 5);
}

#[tokio::test]
async fn test_read_file_respects_limit() {
    let dir = TempDir::new().unwrap();
    let lines: String = (1..=10).map(|i| format!("line{i}\n")).collect();
    std::fs::write(dir.path().join("ten.txt"), &lines).unwrap();

    let tool = ReadFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "ten.txt", "limit": 3 }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    let content = output.0["content"].as_str().expect("content is a string");
    let returned_lines = content.lines().count();
    assert_eq!(returned_lines, 3, "expected exactly 3 lines, got {returned_lines}");
    assert_eq!(output.0["truncated"], true);
    assert_eq!(output.0["end_line"], 3);
}

#[tokio::test]
async fn test_read_file_reports_total_lines() {
    let dir = TempDir::new().unwrap();
    let lines: String = (1..=100).map(|i| format!("line{i}\n")).collect();
    std::fs::write(dir.path().join("hundred.txt"), &lines).unwrap();

    let tool = ReadFileTool;
    let ctx = make_ctx(&dir);
    // Use limit large enough to verify total_lines is accurate regardless.
    let input = ToolInput::new(json!({ "path": "hundred.txt", "limit": 10 }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    assert_eq!(output.0["total_lines"], 100);
    assert_eq!(output.0["truncated"], true);
}

#[tokio::test]
async fn test_read_file_missing_file_errors() {
    let dir = TempDir::new().unwrap();
    let tool = ReadFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "ghost.txt" }));

    let err = tool.call(input, &ctx).await.expect_err("should error on missing file");
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
    // "bar" appears exactly once — "foo bar baz" has no duplicate "bar".
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

#[tokio::test]
async fn test_edit_file_rejects_ambiguous_old_str() {
    let dir = TempDir::new().unwrap();
    // "foo" appears twice on separate lines.
    std::fs::write(
        dir.path().join("ambiguous.txt"),
        "foo first\nsome middle line\nfoo second",
    )
    .unwrap();

    let tool = EditFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "path": "ambiguous.txt",
        "old_str": "foo",
        "new_str": "bar"
    }));

    let err = tool
        .call(input, &ctx)
        .await
        .expect_err("ambiguous old_str should error");

    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("appears 2 times"),
                "error should mention count; got: {reason:?}"
            );
        }
        other => panic!("expected InvalidInput, got: {other:?}"),
    }

    // File must be untouched.
    let on_disk = std::fs::read_to_string(dir.path().join("ambiguous.txt")).unwrap();
    assert_eq!(on_disk, "foo first\nsome middle line\nfoo second");
}

#[tokio::test]
async fn test_edit_file_unique_match_succeeds() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("unique.txt"),
        "fn old_function() {}\nfn another() {}",
    )
    .unwrap();

    let tool = EditFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "path": "unique.txt",
        "old_str": "fn old_function() {}",
        "new_str": "fn new_function() {}"
    }));

    let output = tool
        .call(input, &ctx)
        .await
        .expect("unique match should succeed");
    assert_eq!(output.0["applied"], true);

    let on_disk = std::fs::read_to_string(dir.path().join("unique.txt")).unwrap();
    assert_eq!(on_disk, "fn new_function() {}\nfn another() {}");
}

#[tokio::test]
async fn test_edit_file_ambiguous_error_includes_line_numbers() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("lines.txt"),
        "alpha\nTARGET\nbeta\nTARGET\ngamma",
    )
    .unwrap();

    let tool = EditFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "path": "lines.txt",
        "old_str": "TARGET",
        "new_str": "REPLACED"
    }));

    let err = tool.call(input, &ctx).await.expect_err("should error");
    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            // Both line 2 and line 4 should appear in the error.
            assert!(
                reason.contains('2'),
                "error should include line 2; got: {reason:?}"
            );
            assert!(
                reason.contains('4'),
                "error should include line 4; got: {reason:?}"
            );
        }
        other => panic!("expected InvalidInput, got: {other:?}"),
    }
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
