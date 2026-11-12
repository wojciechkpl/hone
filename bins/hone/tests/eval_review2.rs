#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F26, F28 and F30 of `docs/plans/eval-profiles-spec.md` (review round 2) through the seams
//! of `eval_runs.rs` (harness in `eval_support/`). F27 (`eval_cmd.rs` signal handling behind
//! `#[cfg(unix)]`) cannot be tested on this platform, and the build script's re-run rule
//! (F28) is not testable cheaply: both are skipped. F29 is in `eval_interrupt.rs` and
//! `eval_review1_cli.rs`.
//!
//! Contract pinned:
//! - F26: a task directory that is a SUBDIRECTORY of a git repository or worktree gets, in its
//!   copy, a fresh repository whose only commit holds exactly the task's files (including
//!   uncommitted edits): `git ls-files` lists only those, `git rev-list --count HEAD` is 1, and
//!   `git rev-parse --show-toplevel` is the copy itself. A task directory that IS a repository
//!   root keeps its history.
//! - F28: the manifest has no `git_sha` key (`hone_git_sha` and `suite_git_sha` remain).
//! - F30: `core.worktree` is cleared in a copied `.git/config`.

mod eval_support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eval_support::*;

/// `main` with `a.txt`, `sub/b.txt`, `sub/c.md` committed (plus a second commit), and a linked
/// worktree `wt` with an uncommitted edit of `sub/b.txt`.
fn repo_with_worktree(base: &Path) -> (PathBuf, PathBuf) {
    let main = base.join("main");
    std::fs::create_dir_all(main.join("sub")).unwrap();
    git(&main, &["init", "-q", "-b", "trunk"]);
    std::fs::write(main.join("a.txt"), "outside the task\n").unwrap();
    std::fs::write(main.join("sub/b.txt"), "task file b\n").unwrap();
    std::fs::write(main.join("sub/c.md"), "# task file c\n").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-q", "-m", "first"]);
    std::fs::write(main.join("a.txt"), "outside the task, again\n").unwrap();
    git(&main, &["commit", "-q", "-am", "second"]);
    let wt = base.join("wt");
    git(&main, &["worktree", "add", "-q", "-b", "feature", wt.to_str().unwrap()]);
    std::fs::write(wt.join("sub/b.txt"), "task file b, edited in the worktree\n").unwrap();
    (main, wt)
}

const RECORD: &str = "git ls-files > ls.txt && git rev-list --count HEAD > count.txt && git rev-parse --show-toplevel > top.txt && pwd -P > here.txt";

async fn run_and_read(f: &Fx, dir: &str) -> (std::path::PathBuf, String) {
    let r = runner(f, &task_toml("t", dir, RECORD), quiet_builder(Arc::default()));
    let run = r.run(&hone_eval::EvalOptions { keep_workdirs: true, ..opts(f) }).await.unwrap();
    let o = outcome(&run, "t");
    assert_eq!(o["status"], "pass", "the check ran git in the copy: {o}");
    (run_dir_of(&run, "default", "t", 1).join("workdir"), String::new())
}

fn read(copy: &Path, name: &str) -> String {
    std::fs::read_to_string(copy.join(name)).unwrap().trim().to_owned()
}

#[tokio::test]
async fn test_f26_a_subdirectory_of_a_repository_should_get_only_its_own_files_in_one_commit() {
    let f = fx();
    let (_main, _wt) = repo_with_worktree(&f.base);

    let (copy, _) = run_and_read(&f, "main/sub").await;

    assert_eq!(read(&copy, "ls.txt"), "b.txt\nc.md", "only the task's files are tracked");
    assert_eq!(read(&copy, "count.txt"), "1", "one initial commit, not the repository's history");
    assert_eq!(read(&copy, "top.txt"), read(&copy, "here.txt"), "the copy is its own repository root");
    assert_eq!(std::fs::read_to_string(copy.join("b.txt")).unwrap(), "task file b\n");
}

#[tokio::test]
async fn test_f26_a_subdirectory_of_a_worktree_should_get_the_same_thing_including_uncommitted_edits() {
    let f = fx();
    let (main, wt) = repo_with_worktree(&f.base);
    let before = git(&main, &["rev-list", "--count", "--all"]);

    let (copy, _) = run_and_read(&f, "wt/sub").await;

    assert_eq!(read(&copy, "ls.txt"), "b.txt\nc.md");
    assert_eq!(read(&copy, "count.txt"), "1");
    assert_eq!(read(&copy, "top.txt"), read(&copy, "here.txt"));
    assert_eq!(
        std::fs::read_to_string(copy.join("b.txt")).unwrap(),
        "task file b, edited in the worktree\n",
        "the working tree's content is what gets committed"
    );
    assert_eq!(git(&main, &["rev-list", "--count", "--all"]), before, "the original is untouched");
    assert_eq!(git(&wt, &["status", "--porcelain"]), "M sub/b.txt");

    // A task directory that IS a repository root keeps its own history.
    let (copy, _) = run_and_read(&f, "main").await;
    assert_eq!(read(&copy, "count.txt"), "2", "the root's history is kept");
}

#[tokio::test]
async fn test_f28_the_manifest_should_not_have_a_git_sha_key() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    let r = runner(&f, &task_toml("t", "proj", "true"), quiet_builder(Arc::default()));

    let run = r.run(&opts(&f)).await.unwrap();

    let m = read_json(&run.join("manifest.json"));
    let obj = m.as_object().unwrap();
    assert!(!obj.contains_key("git_sha"), "the ambiguous `git_sha` is gone: {m}");
    assert!(obj.contains_key("hone_git_sha") && obj.contains_key("suite_git_sha"), "{m}");
}

#[tokio::test]
async fn test_f30_a_copied_git_config_should_have_core_worktree_cleared() {
    let f = fx();
    let repo = f.base.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "trunk"]);
    std::fs::write(repo.join("a.txt"), "x\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "first"]);
    git(&repo, &["config", "--file", ".git/config", "core.worktree", "/some/other/path"]);
    let r = runner(&f, &task_toml("t", "repo", "cat .git/config > config.txt"), quiet_builder(Arc::default()));

    let run = r.run(&hone_eval::EvalOptions { keep_workdirs: true, ..opts(&f) }).await.unwrap();

    assert_eq!(outcome(&run, "t")["status"], "pass");
    let config = std::fs::read_to_string(run_dir_of(&run, "default", "t", 1).join("workdir/config.txt")).unwrap();
    assert!(!config.contains("worktree"), "core.worktree must not point the copy's git at another tree:\n{config}");
    // The original's own config is untouched.
    assert!(std::fs::read_to_string(repo.join(".git/config")).unwrap().contains("/some/other/path"));
}
