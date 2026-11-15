#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The eval manifest's `hone_git_sha` is supplied by the binary through
//! `EvalRunner::with_hone_git_sha` (hone-eval has no build script). Running `hone eval`
//! itself needs a model provider, so the binary wiring is checked via its inputs: hone-cli's
//! `HONE_GIT_SHA` must agree with the commit in `--version` (`HONE_LONG_VERSION`).

mod eval_support;

use std::sync::Arc;

use eval_support::*;

const LONG: &str = env!("HONE_LONG_VERSION");
const GIT_SHA: &str = env!("HONE_GIT_SHA");
const SOURCE: &str = env!("HONE_BUILD_SOURCE");

/// The sha inside `X.Y.Z (<sha> [date])`, None for "commit unknown".
fn long_version_sha(v: &str) -> Option<&str> {
    let inner = v.split_once('(')?.1.strip_suffix(')')?;
    (inner != "commit unknown").then(|| inner.split(' ').next().unwrap())
}

async fn manifest_sha(sha: Option<Option<&str>>) -> serde_json::Value {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    let mut r = runner(
        &f,
        &task_toml("t", "proj", "true"),
        quiet_builder(Arc::default()),
    );
    if let Some(s) = sha {
        r = r.with_hone_git_sha(s.map(str::to_owned));
    }
    let run = r.run(&opts(&f)).await.unwrap();
    read_json(&run.join("manifest.json"))["hone_git_sha"].clone()
}

#[tokio::test]
async fn test_manifest_hone_git_sha_when_not_set_should_be_null() {
    assert!(manifest_sha(None).await.is_null());
}

#[tokio::test]
async fn test_manifest_hone_git_sha_when_set_none_should_be_null() {
    assert!(manifest_sha(Some(None)).await.is_null());
}

#[tokio::test]
async fn test_manifest_hone_git_sha_when_set_should_be_exactly_that_string() {
    let sha = "abc1234def5678abc1234def5678abc1234def56";
    assert_eq!(manifest_sha(Some(Some(sha))).await, sha);
}

/// hone-cli's own `HONE_GIT_SHA` (what `hone eval` passes in) must match `--version`.
/// Short overrides (7-11 hex) are valid, so compare by prefix, not by fixed length.
#[test]
fn test_cli_git_sha_should_agree_with_the_version_commit() {
    match long_version_sha(LONG) {
        None => {
            assert_eq!(SOURCE, "unknown", "{LONG}");
            assert_eq!(GIT_SHA, "", "unknown commit must export an empty sha");
        }
        Some(short) => {
            assert!(
                !GIT_SHA.is_empty(),
                "known commit {short} but empty HONE_GIT_SHA"
            );
            assert!(
                GIT_SHA.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
                "{GIT_SHA}"
            );
            assert!(GIT_SHA.starts_with(short), "{GIT_SHA} vs {LONG}");
        }
    }
}
