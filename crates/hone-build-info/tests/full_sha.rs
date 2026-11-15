//! Contract tests for `Resolved::full_sha`: the untruncated lowercase commit sha
//! that build scripts export as `HONE_GIT_SHA`. Throwaway repos in tempdirs.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::fs;
use std::path::Path;
use std::process::Command;

use hone_build_info::{Source, resolve};

const DATE: &str = "2024-03-05T12:00:00+0000";
const FULL40: &str = "B3BB3E0A3170F00DBABE1234567890ABCDEF1234";
const FULL64: &str = "b3bb3e0a3170f00dbabe1234567890abcdef1234b3bb3e0a3170f00dbabe1234";

fn no_env(_: &str) -> Option<String> {
    None
}

/// Hermetic git; `None` when the command fails (e.g. unsupported option).
fn try_git(dir: &Path, args: &[&str]) -> Option<String> {
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
        .env("GIT_AUTHOR_DATE", DATE)
        .env("GIT_COMMITTER_DATE", DATE)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn git(dir: &Path, args: &[&str]) -> String {
    try_git(dir, args).unwrap_or_else(|| panic!("git {args:?} failed"))
}

fn commit_first(root: &Path) {
    fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
    git(root, &["add", "Cargo.toml"]);
    git(root, &["commit", "-q", "-m", "first"]);
}

fn is_prefix_invariant(r: &hone_build_info::Resolved) {
    if let (Some(info), Some(full)) = (&r.info, &r.full_sha) {
        assert!(
            full.starts_with(&info.sha),
            "{} not prefix of {full}",
            info.sha
        );
    }
}

#[test]
fn test_full_sha_when_git_source_should_equal_rev_parse_head() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    commit_first(root);

    let r = resolve(root, no_env);

    assert_eq!(r.source, Source::Git);
    let head = git(root, &["rev-parse", "HEAD"]);
    assert_eq!(r.full_sha.as_deref(), Some(head.as_str()));
    assert_eq!(head.len(), 40);
    is_prefix_invariant(&r);
}

#[test]
fn test_full_sha_when_sha256_repo_should_resolve_as_git_with_64_hex() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    if try_git(root, &["init", "-q", "--object-format=sha256"]).is_none() {
        eprintln!("note: git cannot create sha256 repos; skipping");
        return;
    }
    commit_first(root);

    let r = resolve(root, no_env);

    assert_eq!(r.source, Source::Git, "sha256 repo must not be Unknown");
    let head = git(root, &["rev-parse", "HEAD"]);
    assert_eq!(head.len(), 64);
    assert_eq!(r.full_sha.as_deref(), Some(head.as_str()));
    let info = r.info.as_ref().expect("info");
    assert_eq!(info.sha, head[..12]);
    is_prefix_invariant(&r);
}

#[test]
fn test_full_sha_when_cargo_vcs_info_should_be_lowercased_sha1() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    fs::write(
        dir.join(".cargo_vcs_info.json"),
        format!(r#"{{"git":{{"sha1":"{FULL40}"}}}}"#),
    )
    .unwrap();

    let r = resolve(dir, no_env);

    assert_eq!(r.source, Source::CargoVcsInfo);
    assert_eq!(r.full_sha.as_deref(), Some(FULL40.to_lowercase().as_str()));
    is_prefix_invariant(&r);
}

#[test]
fn test_full_sha_when_override_should_be_trimmed_and_lowercased() {
    let tmp = tempfile::tempdir().unwrap();
    let env = |k: &str| (k == "HONE_BUILD_GIT_SHA").then(|| format!("  {FULL40}\n"));

    let r = resolve(tmp.path(), env);

    assert_eq!(r.source, Source::Override);
    assert_eq!(r.full_sha.as_deref(), Some(FULL40.to_lowercase().as_str()));
    assert_eq!(r.info.as_ref().unwrap().sha, "b3bb3e0a3170");
    is_prefix_invariant(&r);
}

#[test]
fn test_full_sha_when_short_override_should_be_that_short_sha() {
    let tmp = tempfile::tempdir().unwrap();
    let env = |k: &str| (k == "HONE_BUILD_GIT_SHA").then(|| "ABCDEF1".to_string());

    let r = resolve(tmp.path(), env);

    assert_eq!(r.source, Source::Override);
    assert_eq!(r.full_sha.as_deref(), Some("abcdef1"));
    is_prefix_invariant(&r);
}

/// Contract choice: a 64-hex (sha256) override is accepted, like a sha256 repo.
#[test]
fn test_full_sha_when_override_is_64_hex_should_be_accepted() {
    let tmp = tempfile::tempdir().unwrap();
    let env = |k: &str| (k == "HONE_BUILD_GIT_SHA").then(|| FULL64.to_string());

    let r = resolve(tmp.path(), env);

    assert_eq!(r.source, Source::Override);
    assert_eq!(r.full_sha.as_deref(), Some(FULL64));
    assert_eq!(r.info.as_ref().unwrap().sha, &FULL64[..12]);
}

#[test]
fn test_full_sha_when_unknown_should_be_none() {
    let tmp = tempfile::tempdir().unwrap();

    let r = resolve(tmp.path(), no_env);

    assert_eq!(r.source, Source::Unknown);
    assert_eq!(r.full_sha, None);
}

#[test]
fn test_normalize_sha_should_accept_exactly_64_and_reject_63_and_65_hex() {
    let h = |n: usize| "ab12".repeat(17)[..n].to_string();
    assert_eq!(
        hone_build_info::normalize_sha(&h(64)).as_deref(),
        Some(&h(64)[..12])
    );
    for n in [41, 63, 65] {
        assert_eq!(hone_build_info::normalize_sha(&h(n)), None, "{n} hex");
    }
}

#[test]
fn test_full_sha_when_override_is_63_or_65_hex_should_not_be_used() {
    let tmp = tempfile::tempdir().unwrap();
    for n in [63usize, 65] {
        let sha = "ab12".repeat(17)[..n].to_string();
        let env = move |k: &str| (k == "HONE_BUILD_GIT_SHA").then(|| sha.clone());

        let r = resolve(tmp.path(), env);

        assert_eq!(r.source, Source::Unknown, "{n} hex");
        assert_eq!(r.full_sha, None, "{n} hex");
    }
}
