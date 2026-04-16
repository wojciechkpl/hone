use std::sync::Arc;

use hone_sandbox::NullSandbox;
use hone_tools::{EditFileTool, GlobTool, GrepTool, MultiEditTool, ReadFileTool, Tool, ToolContext, ToolInput, WriteFileTool};
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
    let content = output.value["content"].as_str().expect("content is a string");
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
    let content = output.value["content"].as_str().expect("content is a string");
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
    let content = output.value["content"].as_str().expect("content is a string");

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
    assert_eq!(output.value["start_line"], 5);
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
    let content = output.value["content"].as_str().expect("content is a string");
    let returned_lines = content.lines().count();
    assert_eq!(returned_lines, 3, "expected exactly 3 lines, got {returned_lines}");
    assert_eq!(output.value["truncated"], true);
    assert_eq!(output.value["end_line"], 3);
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
    assert_eq!(output.value["total_lines"], 100);
    assert_eq!(output.value["truncated"], true);
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
    assert_eq!(output.value["bytes_written"], 7);

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
    assert_eq!(output.value["applied"], true);

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
    assert_eq!(output.value["applied"], false);

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
    assert_eq!(output.value["applied"], true);

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
// MultiEditTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_multi_edit_applies_multiple_edits_to_one_file() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("target.txt"), "foo\nbar\nbaz\n").unwrap();

    let tool = MultiEditTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "edits": [
            { "path": "target.txt", "old_str": "foo", "new_str": "FOO" },
            { "path": "target.txt", "old_str": "baz", "new_str": "BAZ" }
        ]
    }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    assert_eq!(output.value["files_modified"], 1);

    let on_disk = std::fs::read_to_string(dir.path().join("target.txt")).unwrap();
    assert_eq!(on_disk, "FOO\nbar\nBAZ\n");
}

#[tokio::test]
async fn test_multi_edit_across_multiple_files() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
    std::fs::write(dir.path().join("b.txt"), "rust is great").unwrap();

    let tool = MultiEditTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "edits": [
            { "path": "a.txt", "old_str": "hello", "new_str": "goodbye" },
            { "path": "b.txt", "old_str": "great", "new_str": "awesome" }
        ]
    }));

    let output = tool.call(input, &ctx).await.expect("call should succeed");
    assert_eq!(output.value["files_modified"], 2);

    let a = std::fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert_eq!(a, "goodbye world");

    let b = std::fs::read_to_string(dir.path().join("b.txt")).unwrap();
    assert_eq!(b, "rust is awesome");
}

#[tokio::test]
async fn test_multi_edit_atomic_on_failure() {
    let dir = TempDir::new().unwrap();
    // "foo" appears twice — the second edit targets it, making the overall
    // operation fail. The first edit (valid) must NOT be committed.
    std::fs::write(dir.path().join("atomic.txt"), "foo\nbar\nfoo").unwrap();

    let tool = MultiEditTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "edits": [
            { "path": "atomic.txt", "old_str": "bar", "new_str": "BAR" },
            { "path": "atomic.txt", "old_str": "foo", "new_str": "FOO" }
        ]
    }));

    let err = tool
        .call(input, &ctx)
        .await
        .expect_err("ambiguous second edit should fail");

    match &err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("appears 2 times") || reason.contains("2 times"),
                "error should mention ambiguity; got: {reason:?}"
            );
        }
        other => panic!("expected InvalidInput, got: {other:?}"),
    }

    // File must be untouched — atomic failure.
    let on_disk = std::fs::read_to_string(dir.path().join("atomic.txt")).unwrap();
    assert_eq!(on_disk, "foo\nbar\nfoo", "file should be unchanged after atomic failure");
}

#[tokio::test]
async fn test_multi_edit_empty_array_errors() {
    let dir = TempDir::new().unwrap();

    let tool = MultiEditTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "edits": [] }));

    let err = tool
        .call(input, &ctx)
        .await
        .expect_err("empty edits array should error");

    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("empty"),
                "error should mention empty array; got: {reason:?}"
            );
        }
        other => panic!("expected InvalidInput, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_multi_edit_missing_fields_errors() {
    let dir = TempDir::new().unwrap();

    let tool = MultiEditTool;
    let ctx = make_ctx(&dir);
    // edit[0] is valid; edit[1] is missing 'path'
    let input = ToolInput::new(json!({
        "edits": [
            { "path": "some.txt", "old_str": "x", "new_str": "y" },
            { "old_str": "x", "new_str": "y" }
        ]
    }));

    let err = tool
        .call(input, &ctx)
        .await
        .expect_err("missing path should error");

    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("edit[1]"),
                "error should reference edit index 1; got: {reason:?}"
            );
            assert!(
                reason.contains("path"),
                "error should mention 'path'; got: {reason:?}"
            );
        }
        other => panic!("expected InvalidInput, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_multi_edit_not_found_fails_atomically() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("notfound.txt"), "alpha beta gamma").unwrap();

    let tool = MultiEditTool;
    let ctx = make_ctx(&dir);
    // edit[0] is valid; edit[1] has an old_str that doesn't exist
    let input = ToolInput::new(json!({
        "edits": [
            { "path": "notfound.txt", "old_str": "alpha", "new_str": "ALPHA" },
            { "path": "notfound.txt", "old_str": "NONEXISTENT", "new_str": "X" }
        ]
    }));

    let err = tool
        .call(input, &ctx)
        .await
        .expect_err("not-found old_str should error");

    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("not found"),
                "error should mention not found; got: {reason:?}"
            );
        }
        other => panic!("expected InvalidInput, got: {other:?}"),
    }

    // First edit must not have been committed.
    let on_disk = std::fs::read_to_string(dir.path().join("notfound.txt")).unwrap();
    assert_eq!(on_disk, "alpha beta gamma", "file should be unchanged after atomic failure");
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
    let files = output.value["files"]
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

// ---------------------------------------------------------------------------
// GrepTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_grep_finds_literal_match() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("a.rs"),
        "fn authenticate() {\n    todo!()\n}\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("b.rs"), "fn login() {}\n").unwrap();

    let tool = GrepTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "pattern": "authenticate" }));

    let output = tool.call(input, &ctx).await.expect("grep failed");
    let v = &output.value;
    assert_eq!(v["count"], 1, "expected 1 match, got: {v}");
    assert_eq!(v["matches"][0]["file"], "a.rs");
    assert_eq!(v["matches"][0]["line"], 1);
    assert!(
        v["matches"][0]["content"].as_str().unwrap().contains("authenticate"),
        "content should contain match"
    );
}

#[tokio::test]
async fn test_grep_regex_pattern() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("code.rs"),
        "let x = 42;\nlet y = 100;\nlet z = hello;\n",
    )
    .unwrap();

    let tool = GrepTool;
    let ctx = make_ctx(&dir);
    // Match lines containing a number assignment.
    let input = ToolInput::new(json!({ "pattern": "= \\d+" }));

    let output = tool.call(input, &ctx).await.expect("grep failed");
    let v = &output.value;
    // "let x = 42;" and "let y = 100;" should match; "let z = hello;" should not.
    assert_eq!(v["count"], 2, "expected 2 matches, got: {v}");
}

#[tokio::test]
async fn test_grep_case_insensitive() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("notes.txt"),
        "TODO: fix this\ntodo later\nnothing here\n",
    )
    .unwrap();

    let tool = GrepTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "pattern": "todo",
        "case_insensitive": true
    }));

    let output = tool.call(input, &ctx).await.expect("grep failed");
    let v = &output.value;
    assert_eq!(v["count"], 2, "expected 2 case-insensitive matches, got: {v}");
}

#[tokio::test]
async fn test_grep_honors_gitignore() {
    let dir = TempDir::new().unwrap();
    // Use a .ignore file (always honored by ignore::WalkBuilder regardless of
    // whether the directory is a git repository) to exclude target/.
    std::fs::create_dir(dir.path().join("target")).unwrap();
    std::fs::write(
        dir.path().join("target").join("artifact.rs"),
        "fn secret() {}\n",
    )
    .unwrap();
    std::fs::write(dir.path().join("src.rs"), "fn public() {}\n").unwrap();
    std::fs::write(dir.path().join(".ignore"), "target/\n").unwrap();

    let tool = GrepTool;
    let ctx = make_ctx(&dir);
    // "fn" appears in both files, but target/ should be excluded.
    let input = ToolInput::new(json!({ "pattern": "^fn " }));

    let output = tool.call(input, &ctx).await.expect("grep failed");
    let v = &output.value;
    // Only src.rs should match.
    assert_eq!(v["count"], 1, "target/ should be ignored; got: {v}");
    let file = v["matches"][0]["file"].as_str().unwrap();
    assert_eq!(file, "src.rs", "only src.rs should match; got: {file}");
}

#[tokio::test]
async fn test_grep_respects_max_results() {
    let dir = TempDir::new().unwrap();
    // Write a file with 20 matching lines.
    let content: String = (1..=20).map(|i| format!("match_{i}\n")).collect();
    std::fs::write(dir.path().join("many.txt"), content).unwrap();

    let tool = GrepTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "pattern": "match_",
        "max_results": 5
    }));

    let output = tool.call(input, &ctx).await.expect("grep failed");
    let v = &output.value;
    assert_eq!(v["count"], 5, "expected exactly 5 results (max_results); got: {v}");
    assert_eq!(v["truncated"], true, "truncated should be true");
}

#[tokio::test]
async fn test_grep_include_glob() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.rs"), "fn rust_fn() {}\n").unwrap();
    std::fs::write(dir.path().join("script.py"), "def python_fn(): pass\n").unwrap();

    let tool = GrepTool;
    let ctx = make_ctx(&dir);
    // "fn" matches both files; restrict to only *.rs.
    let input = ToolInput::new(json!({
        "pattern": "fn",
        "include": "*.rs"
    }));

    let output = tool.call(input, &ctx).await.expect("grep failed");
    let v = &output.value;
    assert_eq!(v["count"], 1, "only *.rs files should be searched; got: {v}");
    assert_eq!(v["matches"][0]["file"], "main.rs");
}

#[tokio::test]
async fn test_grep_invalid_regex_errors() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("any.txt"), "content\n").unwrap();

    let tool = GrepTool;
    let ctx = make_ctx(&dir);
    // "[unclosed" is not a valid regex.
    let input = ToolInput::new(json!({ "pattern": "[unclosed" }));

    let err = tool
        .call(input, &ctx)
        .await
        .expect_err("invalid regex should error");

    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("invalid regex"),
                "error message should mention 'invalid regex'; got: {reason:?}"
            );
        }
        other => panic!("expected InvalidInput, got: {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Write / edit verification (G10)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_write_file_verification_succeeds_on_normal_write() {
    let dir = TempDir::new().unwrap();
    let tool = WriteFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "verified.txt", "content": "verified content" }));

    // A successful write should pass verification silently and return the expected output.
    let output = tool.call(input, &ctx).await.expect("write with verification should succeed");
    assert_eq!(output.value["bytes_written"], 16);

    // Confirm the file on disk has the correct content.
    let on_disk = std::fs::read_to_string(dir.path().join("verified.txt")).unwrap();
    assert_eq!(on_disk, "verified content");
}

#[tokio::test]
async fn test_edit_file_verification_succeeds() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("verify_edit.txt"), "before edit").unwrap();

    let tool = EditFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({
        "path": "verify_edit.txt",
        "old_str": "before edit",
        "new_str": "after edit"
    }));

    // A successful edit should pass verification silently and report applied: true.
    let output = tool.call(input, &ctx).await.expect("edit with verification should succeed");
    assert_eq!(output.value["applied"], true);

    // Confirm the file on disk has the updated content.
    let on_disk = std::fs::read_to_string(dir.path().join("verify_edit.txt")).unwrap();
    assert_eq!(on_disk, "after edit");
}

// ---------------------------------------------------------------------------
// Path traversal prevention (resolve_within_working_dir)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_read_file_rejects_dotdot_escape() {
    let dir = TempDir::new().unwrap();
    let tool = ReadFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "../../etc/passwd" }));

    let err = tool.call(input, &ctx).await.expect_err("dotdot escape should be rejected");
    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("outside") || reason.contains("working directory"),
                "error should mention path escape; got: {reason}"
            );
        }
        other => panic!("expected InvalidInput for path traversal, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_write_file_rejects_absolute_escape() {
    let dir = TempDir::new().unwrap();
    let tool = WriteFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "/tmp/hone_escape_test.txt", "content": "pwned" }));

    let err = tool.call(input, &ctx).await.expect_err("absolute path outside working dir should be rejected");
    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("outside") || reason.contains("working directory"),
                "error should mention path escape; got: {reason}"
            );
        }
        other => panic!("expected InvalidInput for absolute escape, got: {other:?}"),
    }
    // Verify the file was NOT created.
    assert!(
        !std::path::Path::new("/tmp/hone_escape_test.txt").exists(),
        "escaped file should not have been created"
    );
}

#[tokio::test]
async fn test_read_file_accepts_nested_path_inside_working_dir() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("inner.txt"), "nested content").unwrap();

    let tool = ReadFileTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "path": "sub/inner.txt" }));

    let output = tool.call(input, &ctx).await.expect("nested path inside working dir should succeed");
    assert!(output.value["content"].as_str().unwrap().contains("nested content"));
}

#[tokio::test]
async fn test_edit_file_rejects_dotdot_in_middle() {
    let dir = TempDir::new().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();

    let tool = EditFileTool;
    let ctx = make_ctx(&dir);
    // Attempt to escape via sub/../../../etc/passwd
    let input = ToolInput::new(json!({
        "path": "sub/../../../etc/passwd",
        "old_str": "root",
        "new_str": "pwned"
    }));

    let err = tool.call(input, &ctx).await.expect_err("dotdot escape via middle should be rejected");
    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("outside") || reason.contains("working directory"),
                "error should mention path escape; got: {reason}"
            );
        }
        // IoError is also acceptable if the path doesn't resolve
        _ => {}
    }
}

#[tokio::test]
async fn test_glob_rejects_dotdot_path() {
    let dir = TempDir::new().unwrap();
    let tool = GlobTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "pattern": "*.txt", "path": "../../" }));

    let err = tool.call(input, &ctx).await.expect_err("dotdot path in glob should be rejected");
    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("outside") || reason.contains("working directory"),
                "error should mention path escape; got: {reason}"
            );
        }
        other => panic!("expected InvalidInput, got: {other:?}"),
    }
}

#[tokio::test]
async fn test_grep_rejects_dotdot_path() {
    let dir = TempDir::new().unwrap();
    let tool = GrepTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "pattern": "root", "path": "../../../etc" }));

    let err = tool.call(input, &ctx).await.expect_err("dotdot path in grep should be rejected");
    match err {
        hone_tools::ToolError::InvalidInput { reason } => {
            assert!(
                reason.contains("outside") || reason.contains("working directory"),
                "error should mention path escape; got: {reason}"
            );
        }
        other => panic!("expected InvalidInput, got: {other:?}"),
    }
}
