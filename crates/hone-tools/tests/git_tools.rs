use std::path::Path;
use std::sync::Arc;

use git2::{Repository, Signature};
use hone_sandbox::NullSandbox;
use hone_tools::{GitCommitTool, GitDiffTool, GitStatusTool, Tool, ToolContext, ToolInput};
use serde_json::json;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_ctx(dir: &TempDir) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        working_dir: dir.path().to_path_buf(),
    }
}

/// Initialise a git repo, configure a test identity, and make an initial commit.
fn init_repo_with_commit(dir: &Path) {
    let repo = Repository::init(dir).expect("repo init");

    // Configure identity so commits don't fail on a machine with no global config.
    {
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test User").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
    }

    // Create an initial commit so HEAD is valid.
    let sig = Signature::now("Test User", "test@example.com").unwrap();
    let readme = dir.join("README.md");
    std::fs::write(&readme, "# test repo").unwrap();

    let mut index = repo.index().unwrap();
    index.add_path(Path::new("README.md")).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_oid).unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[])
        .unwrap();
    // `tree` is dropped here before `repo`, satisfying the borrow checker.
}

// ---------------------------------------------------------------------------
// GitStatusTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_git_status_clean_repo() {
    let dir = TempDir::new().unwrap();
    init_repo_with_commit(dir.path());

    let tool = GitStatusTool;
    let ctx = make_ctx(&dir);
    let output = tool
        .call(ToolInput::new(json!({})), &ctx)
        .await
        .expect("call should succeed");

    assert_eq!(output.0["modified"].as_array().unwrap().len(), 0);
    assert_eq!(output.0["staged"].as_array().unwrap().len(), 0);
    assert_eq!(output.0["untracked"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_git_status_with_changes() {
    let dir = TempDir::new().unwrap();
    init_repo_with_commit(dir.path());

    // Modify the tracked file.
    let readme = dir.path().join("README.md");
    std::fs::write(&readme, "# modified").unwrap();

    // Add an untracked file.
    std::fs::write(dir.path().join("new_file.txt"), "untracked").unwrap();

    let tool = GitStatusTool;
    let ctx = make_ctx(&dir);
    let output = tool
        .call(ToolInput::new(json!({})), &ctx)
        .await
        .expect("call should succeed");

    let modified: Vec<&str> = output.0["modified"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        modified.contains(&"README.md"),
        "expected README.md in modified: {modified:?}"
    );

    let untracked: Vec<&str> = output.0["untracked"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(
        untracked.contains(&"new_file.txt"),
        "expected new_file.txt in untracked: {untracked:?}"
    );
}

// ---------------------------------------------------------------------------
// GitCommitTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_git_commit_creates_commit() {
    let dir = TempDir::new().unwrap();
    init_repo_with_commit(dir.path());

    // Stage a new file.
    std::fs::write(dir.path().join("staged.txt"), "staged content").unwrap();
    let repo = Repository::discover(dir.path()).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("staged.txt")).unwrap();
    index.write().unwrap();
    drop(repo);

    let tool = GitCommitTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "message": "add staged.txt" }));
    let output = tool.call(input, &ctx).await.expect("call should succeed");

    let sha = output.0["sha"].as_str().expect("sha should be a string");
    assert!(!sha.is_empty(), "sha should not be empty");
    // A git SHA is 40 hex chars.
    assert_eq!(sha.len(), 40, "SHA should be 40 characters, got: {sha}");
}

// ---------------------------------------------------------------------------
// GitDiffTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_git_diff_shows_changes() {
    let dir = TempDir::new().unwrap();
    init_repo_with_commit(dir.path());

    // Modify the tracked file without staging.
    std::fs::write(dir.path().join("README.md"), "# changed content").unwrap();

    let tool = GitDiffTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "staged": false }));
    let output = tool.call(input, &ctx).await.expect("call should succeed");

    let diff = output.0["diff"].as_str().expect("diff should be a string");
    assert!(!diff.is_empty(), "diff should not be empty for modified file");
    assert!(
        diff.contains("changed content") || diff.contains("README"),
        "diff should mention the changed content or file: {diff}"
    );
}
