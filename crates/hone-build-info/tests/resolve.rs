//! Contract tests for `resolve()`: where the commit info comes from and which
//! paths the build script must watch. Uses throwaway git repos in tempdirs and
//! never touches the process environment.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use hone_build_info::{Resolved, Source, resolve};

const DATE_OLD: &str = "2024-03-05T12:00:00+0000";
const DATE_NEW: &str = "2025-07-09T12:00:00+0000";
const FULL: &str = "b3bb3e0a3170f00dbabe1234567890abcdef1234";

fn no_env(_: &str) -> Option<String> {
    None
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run git in `dir` with a hermetic identity; returns trimmed stdout.
fn git(dir: &Path, date: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        // Hermetic: never let an inherited repo (e.g. `git rebase -x`, hooks in a
        // linked worktree) redirect these commands into the real repository.
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

/// Repo with `bins/x/Cargo.toml` tracked and one commit at DATE_OLD.
fn init_repo(root: &Path) {
    git(root, DATE_OLD, &["init", "-q"]);
    write(&root.join("bins/x/Cargo.toml"), "[package]\nname = \"x\"\n");
    git(root, DATE_OLD, &["add", "bins/x/Cargo.toml"]);
    git(root, DATE_OLD, &["commit", "-q", "-m", "first"]);
}

fn commit_more(root: &Path) {
    write(&root.join("file.txt"), "more\n");
    git(root, DATE_NEW, &["add", "file.txt"]);
    git(root, DATE_NEW, &["commit", "-q", "-m", "second"]);
}

fn head_sha(root: &Path) -> String {
    git(root, DATE_NEW, &["rev-parse", "--short=12", "HEAD"])
}

fn head_date(root: &Path) -> String {
    git(
        root,
        DATE_NEW,
        &["log", "-1", "--no-show-signature", "--format=%cs", "HEAD"],
    )
}

fn assert_paths_exist(r: &Resolved) {
    assert!(
        !r.rerun_if_changed.is_empty(),
        "rerun list must not be empty"
    );
    for p in &r.rerun_if_changed {
        assert!(p.exists(), "rerun path does not exist: {}", p.display());
    }
}

fn find_logs_head(r: &Resolved) -> Option<&PathBuf> {
    r.rerun_if_changed.iter().find(|p| p.ends_with("logs/HEAD"))
}

macro_rules! need_git {
    () => {
        if !git_available() {
            eprintln!("note: git not available; skipping");
            return;
        }
    };
}

#[test]
fn test_resolve_when_plain_repo_should_use_git_and_watch_existing_paths() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    commit_more(root);

    let r = resolve(&root.join("bins/x"), no_env);

    assert_eq!(r.source, Source::Git);
    let info = r.info.as_ref().expect("git info");
    assert_eq!(info.sha, head_sha(root));
    assert_eq!(info.date.as_deref(), Some(head_date(root).as_str()));
    assert_eq!(info.date.as_deref(), Some("2025-07-09"));
    assert_paths_exist(&r);
    assert!(
        find_logs_head(&r).is_some(),
        "must watch logs/HEAD: {:?}",
        r.rerun_if_changed
    );
}

#[test]
fn test_resolve_when_linked_worktree_should_use_worktree_head() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let main = tmp.path().join("main");
    fs::create_dir_all(&main).unwrap();
    init_repo(&main);
    let wt = tmp.path().join("wt");
    git(
        &main,
        DATE_OLD,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            wt.to_str().unwrap(),
        ],
    );
    // Advance the worktree only, so the main HEAD and worktree HEAD differ.
    commit_more(&wt);
    assert_ne!(head_sha(&main), head_sha(&wt));

    let r = resolve(&wt.join("bins/x"), no_env);

    assert_eq!(r.source, Source::Git);
    assert_eq!(r.info.as_ref().expect("git info").sha, head_sha(&wt));
    assert_paths_exist(&r);
    let logs = find_logs_head(&r).expect("must watch logs/HEAD");
    assert!(
        logs.components().any(|c| c.as_os_str() == "worktrees"),
        "watched logs/HEAD must be the worktree's own: {}",
        logs.display()
    );
}

#[test]
fn test_resolve_when_refs_are_packed_should_find_sha_and_watch_only_existing_paths() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    commit_more(root);
    git(root, DATE_NEW, &["pack-refs", "--all"]);

    let r = resolve(&root.join("bins/x"), no_env);

    assert_eq!(r.source, Source::Git);
    assert_eq!(r.info.as_ref().expect("git info").sha, head_sha(root));
    assert_paths_exist(&r);
}

#[test]
fn test_resolve_when_fresh_repo_without_packed_refs_should_watch_only_existing_paths() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    assert!(!root.join(".git/packed-refs").exists());

    let r = resolve(&root.join("bins/x"), no_env);

    assert_eq!(r.source, Source::Git);
    assert_eq!(r.info.as_ref().expect("git info").sha, head_sha(root));
    assert_paths_exist(&r);
}

#[test]
fn test_resolve_when_head_is_detached_should_use_that_commit() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    let older = head_sha(root);
    commit_more(root);
    assert_ne!(older, head_sha(root));
    git(root, DATE_NEW, &["checkout", "-q", "--detach", &older]);

    let r = resolve(&root.join("bins/x"), no_env);

    assert_eq!(r.source, Source::Git);
    let info = r.info.as_ref().expect("git info");
    assert_eq!(info.sha, older);
    assert_eq!(info.date.as_deref(), Some("2024-03-05"));
    assert_paths_exist(&r);
}

#[test]
fn test_resolve_when_cargo_vcs_info_present_should_beat_git() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    let dir = root.join("bins/x");
    let vcs = dir.join(".cargo_vcs_info.json");
    write(
        &vcs,
        &format!(r#"{{"git":{{"sha1":"{FULL}"}},"path_in_vcs":"bins/x"}}"#),
    );

    let r = resolve(&dir, no_env);

    assert_eq!(r.source, Source::CargoVcsInfo);
    let info = r.info.as_ref().expect("vcs info");
    assert_eq!(info.sha, "b3bb3e0a3170");
    assert_eq!(info.date, None);
    assert!(
        r.rerun_if_changed.contains(&vcs),
        "must watch the vcs info file: {:?}",
        r.rerun_if_changed
    );
}

#[test]
fn test_resolve_when_override_env_set_should_beat_vcs_info_and_git() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    let dir = root.join("bins/x");
    write(
        &dir.join(".cargo_vcs_info.json"),
        r#"{"git":{"sha1":"1111111111111111111111111111111111111111"}}"#,
    );
    let env = |k: &str| match k {
        "HONE_BUILD_GIT_SHA" => Some(FULL.to_string()),
        "HONE_BUILD_GIT_DATE" => Some("2026-11-13".to_string()),
        _ => None,
    };

    let r = resolve(&dir, env);

    assert_eq!(r.source, Source::Override);
    let info = r.info.as_ref().expect("override info");
    assert_eq!(info.sha, "b3bb3e0a3170");
    assert_eq!(info.date.as_deref(), Some("2026-11-13"));
}

#[test]
fn test_resolve_when_crate_is_untracked_inside_unrelated_repo_should_be_unknown() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    // A vendored/extracted crate inside the outer repo's tree but not tracked by it.
    let nested = root.join("vendor/other");
    write(&nested.join("Cargo.toml"), "[package]\nname = \"other\"\n");

    let r = resolve(&nested, no_env);

    assert_eq!(r.source, Source::Unknown);
    assert!(
        r.info.is_none(),
        "must not report the outer repo commit: {:?}",
        r.rerun_if_changed
    );
}

#[test]
fn test_resolve_when_no_git_repo_should_be_unknown_and_watch_existing_manifest_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("crate");
    write(&dir.join("Cargo.toml"), "[package]\nname = \"c\"\n");

    let r = resolve(&dir, no_env);

    assert_eq!(r.source, Source::Unknown);
    assert!(r.info.is_none());
    assert!(
        r.rerun_if_changed.contains(&dir),
        "unknown must watch the manifest dir itself: {:?}",
        r.rerun_if_changed
    );
    assert_paths_exist(&r);
}

#[test]
fn test_source_as_str_should_return_stable_names() {
    assert_eq!(Source::Override.as_str(), "override");
    assert_eq!(Source::CargoVcsInfo.as_str(), "cargo-vcs-info");
    assert_eq!(Source::Git.as_str(), "git");
    assert_eq!(Source::Unknown.as_str(), "unknown");
}

/// Snapshot of everything cargo would look at for the given watched paths:
/// file contents, and for directories the entry names with len and mtime.
fn watched_state(r: &Resolved) -> Vec<String> {
    r.rerun_if_changed
        .iter()
        .map(|p| {
            if p.is_dir() {
                let mut entries: Vec<String> = fs::read_dir(p)
                    .unwrap()
                    .map(|e| {
                        let e = e.unwrap();
                        let m = e.metadata().unwrap();
                        format!("{:?}:{}:{:?}", e.file_name(), m.len(), m.modified().ok())
                    })
                    .collect();
                entries.sort();
                format!("dir {} {entries:?}", p.display())
            } else {
                let bytes = fs::read(p).unwrap_or_default();
                format!("file {} {bytes:?}", p.display())
            }
        })
        .collect()
}

/// Resolve, make a new commit, and assert some watched path changed state.
fn assert_new_commit_changes_watched_state(root: &Path) {
    let r = resolve(&root.join("bins/x"), no_env);
    assert_eq!(r.source, Source::Git);
    assert_paths_exist(&r);
    let before = watched_state(&r);
    // Defeat coarse filesystem mtime granularity.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    commit_more(root);
    let after = watched_state(&r);
    assert_ne!(
        before, after,
        "a new commit must change something cargo watches: {:?}",
        r.rerun_if_changed
    );
}

#[test]
fn test_resolve_when_branch_only_in_packed_refs_should_watch_something_a_commit_changes() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root);
    git(root, DATE_NEW, &["pack-refs", "--all"]);
    assert!(root.join(".git/logs/HEAD").exists(), "reflog should be on");

    assert_new_commit_changes_watched_state(root);
}

#[test]
fn test_resolve_when_reflog_disabled_and_packed_refs_should_watch_something_a_commit_changes() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, DATE_OLD, &["init", "-q"]);
    git(
        root,
        DATE_OLD,
        &["config", "core.logAllRefUpdates", "false"],
    );
    write(&root.join("bins/x/Cargo.toml"), "[package]\nname = \"x\"\n");
    git(root, DATE_OLD, &["add", "bins/x/Cargo.toml"]);
    git(root, DATE_OLD, &["commit", "-q", "-m", "first"]);
    git(root, DATE_NEW, &["pack-refs", "--all"]);
    assert!(
        !root.join(".git/logs/HEAD").exists(),
        "reflog should be off"
    );

    assert_new_commit_changes_watched_state(root);
}

#[test]
fn test_resolve_when_reftable_repo_should_watch_something_a_commit_changes() {
    need_git!();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let init = Command::new("git")
        .args(["init", "-q", "--ref-format=reftable"])
        .current_dir(root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .output();
    if !init.map(|o| o.status.success()).unwrap_or(false) {
        eprintln!("note: git lacks reftable support; skipping");
        return;
    }
    write(&root.join("bins/x/Cargo.toml"), "[package]\nname = \"x\"\n");
    git(root, DATE_OLD, &["add", "bins/x/Cargo.toml"]);
    git(root, DATE_OLD, &["commit", "-q", "-m", "first"]);

    assert_new_commit_changes_watched_state(root);
}
