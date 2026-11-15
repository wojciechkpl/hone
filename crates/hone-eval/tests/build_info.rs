//! hone-eval's build script must use `hone_build_info::emit()`, so its `HONE_GIT_SHA`
//! agrees with the long version and build source it now also exports.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::Path;
use std::process::Command;

const LONG: &str = env!("HONE_LONG_VERSION");
const SOURCE: &str = env!("HONE_BUILD_SOURCE");
const GIT_SHA: Option<&str> = option_env!("HONE_GIT_SHA");

fn head() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
fn test_build_source_should_be_one_of_the_known_names() {
    assert!(
        ["override", "cargo-vcs-info", "git", "unknown"].contains(&SOURCE),
        "unexpected HONE_BUILD_SOURCE {SOURCE:?}"
    );
}

#[test]
fn test_git_sha_should_be_consistent_with_long_version_and_source() {
    let sha = GIT_SHA.expect("build script must set HONE_GIT_SHA (possibly empty)");
    if SOURCE == "unknown" {
        assert_eq!(sha, "");
        assert!(LONG.ends_with("(commit unknown)"), "{LONG}");
    } else {
        assert!(sha.len() >= 7, "sha too short: {sha:?}");
        assert!(
            sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "{sha:?}"
        );
        let short = &sha[..sha.len().min(12)];
        assert!(LONG.contains(short), "{short} not in {LONG}");
    }
}

#[test]
fn test_git_sha_when_source_is_git_should_equal_rev_parse_head() {
    if SOURCE != "git" {
        eprintln!("note: build source is {SOURCE:?}, not git; skipping");
        return;
    }
    let Some(head) = head() else {
        eprintln!("note: git unavailable here; skipping");
        return;
    };
    assert_eq!(GIT_SHA, Some(head.as_str()));
    assert!(LONG.contains(&head[..12]), "{LONG}");
}
