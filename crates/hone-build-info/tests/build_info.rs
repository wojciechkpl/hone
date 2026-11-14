//! Contract tests for the pure API of `hone-build-info`.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use hone_build_info::{
    GitInfo, git_info_from_overrides, long_version, normalize_date, normalize_sha,
    sha_from_cargo_vcs_info,
};

const FULL: &str = "b3bb3e0a3170f00dbabe1234567890abcdef1234";

fn gi(sha: &str, date: Option<&str>) -> GitInfo {
    GitInfo {
        sha: sha.to_string(),
        date: date.map(str::to_string),
    }
}

#[test]
fn test_long_version_when_sha_and_date_should_show_both() {
    let g = gi("b3bb3e0a3170", Some("2026-11-13"));
    assert_eq!(
        long_version("0.1.0", Some(&g)),
        "0.1.0 (b3bb3e0a3170 2026-11-13)"
    );
}

#[test]
fn test_long_version_when_no_date_should_show_sha_only() {
    let g = gi("b3bb3e0a3170", None);
    assert_eq!(long_version("0.1.0", Some(&g)), "0.1.0 (b3bb3e0a3170)");
}

#[test]
fn test_long_version_when_no_git_should_say_commit_unknown() {
    assert_eq!(long_version("0.1.0", None), "0.1.0 (commit unknown)");
}

#[test]
fn test_normalize_sha_when_full_sha_should_truncate_to_12() {
    assert_eq!(normalize_sha(FULL).as_deref(), Some("b3bb3e0a3170"));
}

#[test]
fn test_normalize_sha_when_uppercase_should_lowercase() {
    assert_eq!(
        normalize_sha("B3BB3E0A3170F00D").as_deref(),
        Some("b3bb3e0a3170")
    );
}

#[test]
fn test_normalize_sha_when_surrounded_by_whitespace_should_trim() {
    assert_eq!(normalize_sha("  b3bb3e0\n").as_deref(), Some("b3bb3e0"));
}

#[test]
fn test_normalize_sha_when_exactly_7_or_12_should_keep_as_is() {
    assert_eq!(normalize_sha("abcdef1").as_deref(), Some("abcdef1"));
    assert_eq!(
        normalize_sha("abcdef123456").as_deref(),
        Some("abcdef123456")
    );
}

#[test]
fn test_normalize_sha_when_exactly_40_should_accept() {
    let s = "a".repeat(40);
    assert_eq!(normalize_sha(&s).as_deref(), Some("aaaaaaaaaaaa"));
}

#[test]
fn test_normalize_sha_when_invalid_should_return_none() {
    for bad in [
        "",
        "   ",
        "abcdef",        // 6 chars, too short
        "xyzxyzxyzxyz",  // non-hex
        "b3bb3e0a317g",  // one non-hex char
        &"a".repeat(41), // too long
        "b3bb3e0 a3170", // inner whitespace
    ] {
        assert_eq!(normalize_sha(bad), None, "input {bad:?}");
    }
}

#[test]
fn test_normalize_date_when_iso_date_should_accept_and_trim() {
    assert_eq!(normalize_date("2026-11-13").as_deref(), Some("2026-11-13"));
    assert_eq!(
        normalize_date(" 2026-11-13\n").as_deref(),
        Some("2026-11-13")
    );
}

#[test]
fn test_normalize_date_when_not_yyyy_mm_dd_should_return_none() {
    for bad in [
        "",
        "2026-11-1",
        "2026/11/13",
        "20261113",
        "2026-11-13T10:00:00",
        "26-11-13",
        "abcd-ef-gh",
        "2026-11-13 10:00",
    ] {
        assert_eq!(normalize_date(bad), None, "input {bad:?}");
    }
}

#[test]
fn test_sha_from_cargo_vcs_info_when_valid_should_normalise_sha1() {
    let json = format!(r#"{{"git":{{"sha1":"{FULL}"}},"path_in_vcs":"bins/hone"}}"#);
    assert_eq!(
        sha_from_cargo_vcs_info(&json).as_deref(),
        Some("b3bb3e0a3170")
    );
}

#[test]
fn test_sha_from_cargo_vcs_info_when_bad_input_should_return_none() {
    for bad in [
        "not json",
        "",
        "{}",
        r#"{"git":{}}"#,
        r#"{"path_in_vcs":"bins/hone"}"#,
        r#"{"git":{"sha1":"zzzz"}}"#,
        r#"{"git":{"sha1":""}}"#,
        r#"{"git":{"sha1":123}}"#,
    ] {
        assert_eq!(sha_from_cargo_vcs_info(bad), None, "input {bad:?}");
    }
}

fn env_of<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |k| {
        pairs
            .iter()
            .find(|(key, _)| *key == k)
            .map(|(_, v)| v.to_string())
    }
}

#[test]
fn test_overrides_when_sha_and_date_valid_should_return_both() {
    let get = env_of(&[
        ("HONE_BUILD_GIT_SHA", FULL),
        ("HONE_BUILD_GIT_DATE", "2026-11-13"),
    ]);
    let g = git_info_from_overrides(get).expect("some");
    assert_eq!(g.sha, "b3bb3e0a3170");
    assert_eq!(g.date.as_deref(), Some("2026-11-13"));
}

#[test]
fn test_overrides_when_date_missing_should_have_no_date() {
    let get = env_of(&[("HONE_BUILD_GIT_SHA", FULL)]);
    let g = git_info_from_overrides(get).expect("some");
    assert_eq!(g.sha, "b3bb3e0a3170");
    assert_eq!(g.date, None);
}

#[test]
fn test_overrides_when_date_bad_should_have_no_date() {
    let get = env_of(&[
        ("HONE_BUILD_GIT_SHA", FULL),
        ("HONE_BUILD_GIT_DATE", "yesterday"),
    ]);
    let g = git_info_from_overrides(get).expect("some");
    assert_eq!(g.date, None);
}

#[test]
fn test_overrides_when_sha_missing_or_bad_should_return_none() {
    assert!(git_info_from_overrides(env_of(&[])).is_none());
    assert!(git_info_from_overrides(env_of(&[("HONE_BUILD_GIT_DATE", "2026-11-13")])).is_none());
    assert!(
        git_info_from_overrides(env_of(&[
            ("HONE_BUILD_GIT_SHA", "nothex!"),
            ("HONE_BUILD_GIT_DATE", "2026-11-13"),
        ]))
        .is_none()
    );
}
