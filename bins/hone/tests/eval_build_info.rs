#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! The eval manifest's `hone_git_sha` must agree with what `hone --version` reports:
//! both come from the same resolution (`hone_build_info`), so the manifest value is null
//! exactly when the build source is unknown, else a hex sha starting with the 12-char
//! sha embedded in `HONE_LONG_VERSION`.

mod eval_support;

use std::sync::Arc;

use eval_support::*;

const LONG: &str = env!("HONE_LONG_VERSION");
const SOURCE: &str = env!("HONE_BUILD_SOURCE");

/// The sha inside `X.Y.Z (<sha> [date])`, None for "commit unknown".
fn long_version_sha(v: &str) -> Option<&str> {
    let inner = v.split_once('(')?.1.strip_suffix(')')?;
    (inner != "commit unknown").then(|| inner.split(' ').next().unwrap())
}

#[tokio::test]
async fn test_manifest_hone_git_sha_should_agree_with_the_version_commit() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    let r = runner(
        &f,
        &task_toml("t", "proj", "true"),
        quiet_builder(Arc::default()),
    );

    let run = r.run(&opts(&f)).await.unwrap();

    let m = read_json(&run.join("manifest.json"));
    let sha = &m["hone_git_sha"];
    if SOURCE == "unknown" {
        assert!(sha.is_null(), "unknown build source must give null: {sha}");
        assert_eq!(long_version_sha(LONG), None, "{LONG}");
    } else {
        let s = sha
            .as_str()
            .unwrap_or_else(|| panic!("expected a sha string: {sha}"));
        assert!(
            s.len() >= 12 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "lowercase hex sha expected: {s}"
        );
        let short = long_version_sha(LONG).expect("known source implies a sha in the version");
        assert_eq!(&s[..12], short, "manifest vs {LONG}");
    }
}
