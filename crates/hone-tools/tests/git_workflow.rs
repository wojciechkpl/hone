#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::sync::Arc;

use git2::{Repository, Signature};
use hone_sandbox::NullSandbox;
use hone_tools::{
    AutoCommitter, GitBranchTool, GitCheckoutTool, GitUndoTool, Tool, ToolContext, ToolInput,
    UndoStack,
};
use serde_json::json;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_ctx(dir: &TempDir) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        working_dir: dir.path().to_path_buf(),
    }
}

/// Initialise a git repo, configure a test identity, and make an initial commit.
/// Also configures `init.defaultBranch` so HEAD is called "main" consistently.
fn init_repo_with_commit(dir: &Path) {
    let repo = Repository::init(dir).expect("repo init");

    {
        let mut cfg = repo.config().unwrap();
        cfg.set_str("user.name", "Test User").unwrap();
        cfg.set_str("user.email", "test@example.com").unwrap();
    }

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
}

/// Returns the current branch name by reading HEAD via `git rev-parse --abbrev-ref HEAD`.
async fn current_branch(dir: &Path) -> String {
    let out = tokio::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(dir)
        .output()
        .await
        .expect("git rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Returns the number of commits in the current branch log.
async fn commit_count(dir: &Path) -> usize {
    let out = tokio::process::Command::new("git")
        .args(["rev-list", "--count", "HEAD"])
        .current_dir(dir)
        .output()
        .await
        .expect("git rev-list");
    String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// AutoCommitter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_auto_commit_after_write() {
    let dir = TempDir::new().unwrap();
    init_repo_with_commit(dir.path());

    let file_path = dir.path().join("hello.txt");
    std::fs::write(&file_path, "hello world").unwrap();

    let sha = AutoCommitter::commit_file(dir.path(), &file_path, "add hello.txt")
        .await
        .expect("commit_file should succeed");

    assert_eq!(sha.len(), 40, "SHA should be 40 hex chars, got: {sha}");

    // There should now be 2 commits: initial + the auto-commit.
    assert_eq!(commit_count(dir.path()).await, 2);
}

// ---------------------------------------------------------------------------
// GitUndoTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_undo_reverts_last_commit() {
    let dir = TempDir::new().unwrap();
    init_repo_with_commit(dir.path());

    // Create a second commit that adds a file.
    let file_path = dir.path().join("to_revert.txt");
    std::fs::write(&file_path, "will be reverted").unwrap();
    AutoCommitter::commit_file(dir.path(), &file_path, "add to_revert.txt")
        .await
        .expect("setup commit");

    let before_count = commit_count(dir.path()).await;

    let tool = GitUndoTool;
    let ctx = make_ctx(&dir);
    let output = tool
        .call(ToolInput::new(json!({})), &ctx)
        .await
        .expect("undo should succeed");

    let reverted_sha = output.value["reverted_sha"]
        .as_str()
        .expect("reverted_sha should be a string");
    assert_eq!(reverted_sha.len(), 40, "SHA should be 40 hex chars");

    // There should be one more commit (the revert commit).
    assert_eq!(commit_count(dir.path()).await, before_count + 1);

    // The git log should mention "Revert".
    let log_out = tokio::process::Command::new("git")
        .args(["log", "-1", "--pretty=%s"])
        .current_dir(dir.path())
        .output()
        .await
        .unwrap();
    let subject = String::from_utf8_lossy(&log_out.stdout);
    assert!(
        subject.contains("Revert"),
        "expected 'Revert' in subject: {subject}"
    );
}

// ---------------------------------------------------------------------------
// GitBranchTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_branch_create_and_switch() {
    let dir = TempDir::new().unwrap();
    init_repo_with_commit(dir.path());

    let tool = GitBranchTool;
    let ctx = make_ctx(&dir);
    let input = ToolInput::new(json!({ "name": "feature/new-thing", "create": true }));
    let output = tool
        .call(input, &ctx)
        .await
        .expect("branch create should succeed");

    assert_eq!(
        output.value["branch"].as_str().unwrap(),
        "feature/new-thing"
    );
    assert_eq!(current_branch(dir.path()).await, "feature/new-thing");
}

// ---------------------------------------------------------------------------
// GitCheckoutTool
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_checkout_switches_branch() {
    let dir = TempDir::new().unwrap();
    init_repo_with_commit(dir.path());

    // Detect the default branch name (could be "main" or "master").
    let default_branch = current_branch(dir.path()).await;

    // Create a new branch and switch to it.
    let branch_tool = GitBranchTool;
    let ctx = make_ctx(&dir);
    branch_tool
        .call(
            ToolInput::new(json!({ "name": "other-branch", "create": true })),
            &ctx,
        )
        .await
        .expect("create other-branch");

    assert_eq!(current_branch(dir.path()).await, "other-branch");

    // Switch back to the default branch using GitCheckoutTool.
    let checkout_tool = GitCheckoutTool;
    let output = checkout_tool
        .call(ToolInput::new(json!({ "branch": default_branch })), &ctx)
        .await
        .expect("checkout should succeed");

    let result_branch = output.value["branch"].as_str().unwrap();
    assert_eq!(result_branch, default_branch);
    assert_eq!(current_branch(dir.path()).await, default_branch);

    let head_sha = output.value["head"].as_str().unwrap();
    assert_eq!(head_sha.len(), 40);

    // Now use GitCheckoutTool to switch back to other-branch.
    let output2 = checkout_tool
        .call(ToolInput::new(json!({ "branch": "other-branch" })), &ctx)
        .await
        .expect("checkout other-branch");

    assert_eq!(output2.value["branch"].as_str().unwrap(), "other-branch");
    assert_eq!(current_branch(dir.path()).await, "other-branch");
}

// ---------------------------------------------------------------------------
// UndoStack
// ---------------------------------------------------------------------------

#[test]
fn test_undo_stack_is_bounded_at_50() {
    let mut stack = UndoStack::new(50);
    for i in 0..60 {
        stack.push(format!("sha{i}"));
    }
    assert_eq!(stack.len(), 50);
}

#[test]
fn test_undo_redo_cycle() {
    let mut stack = UndoStack::new(50);
    stack.push("sha1".to_string());
    stack.push("sha2".to_string());
    stack.push("sha3".to_string());

    // Undo twice.
    assert_eq!(stack.undo(), Some("sha3".to_string()));
    assert_eq!(stack.undo(), Some("sha2".to_string()));
    assert_eq!(stack.len(), 1, "one entry should remain after two undos");

    // Redo once — sha2 comes back.
    assert_eq!(stack.redo(), Some("sha2".to_string()));
    assert_eq!(stack.len(), 2, "two entries after one redo");

    // Redo again — sha3 comes back.
    assert_eq!(stack.redo(), Some("sha3".to_string()));
    assert_eq!(stack.len(), 3);

    // No more redos.
    assert_eq!(stack.redo(), None);
}
