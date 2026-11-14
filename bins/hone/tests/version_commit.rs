#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

/// The long version baked in by build.rs (reaches integration tests via cargo:rustc-env).
const LONG: &str = env!("HONE_LONG_VERSION");

fn run(args: &[&str]) -> (String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_hone"))
        .args(args)
        .output()
        .expect("spawn binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_digit())
}

fn is_lower_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

fn is_date(s: &str) -> bool {
    let d: Vec<&str> = s.split('-').collect();
    d.len() == 3
        && d[0].len() == 4
        && d[1].len() == 2
        && d[2].len() == 2
        && d.iter().all(|p| all_digits(p))
}

/// Parsed long version: (sha, date). Both None for "commit unknown".
/// Accepts `X.Y.Z (<12 hex> <date>)`, `X.Y.Z (<7..12 hex>)`, `X.Y.Z (commit unknown)`.
fn parse_long_version(v: &str) -> (Option<String>, Option<String>) {
    let (semver, tail) = v.split_once(' ').unwrap_or_default();
    let parts: Vec<&str> = semver.split('.').collect();
    assert!(
        parts.len() == 3 && parts.iter().all(|p| all_digits(p)),
        "bad semver in {v:?}"
    );
    let inner = tail
        .strip_prefix('(')
        .and_then(|t| t.strip_suffix(')'))
        .unwrap_or_default();
    assert!(!inner.is_empty(), "no parenthesised part in {v:?}");
    if inner == "commit unknown" {
        return (None, None);
    }
    match inner.split_once(' ') {
        Some((sha, date)) => {
            assert!(sha.len() == 12 && is_lower_hex(sha), "bad sha in {v:?}");
            assert!(is_date(date), "bad date in {v:?}");
            (Some(sha.to_string()), Some(date.to_string()))
        }
        None => {
            assert!(
                (7..=12).contains(&inner.len()) && is_lower_hex(inner),
                "bad sha in {v:?}"
            );
            (Some(inner.to_string()), None)
        }
    }
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(Path::new(env!("CARGO_MANIFEST_DIR")))
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Strict comparison with the surrounding checkout, only when the build
/// could have read it (no packager override, commit known, git usable).
fn assert_matches_checkout() {
    let (sha, date) = parse_long_version(LONG);
    let Some(sha) = sha else {
        eprintln!("note: version is 'commit unknown'; skipping checkout comparison");
        return;
    };
    if option_env!("HONE_BUILD_GIT_SHA").is_some() {
        eprintln!("note: HONE_BUILD_GIT_SHA override set; skipping checkout comparison");
        return;
    }
    if git(&["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        eprintln!("note: not in a git checkout; skipping checkout comparison");
        return;
    }
    let (Some(head), Some(cs)) = (
        git(&["rev-parse", "--short=12", "HEAD"]),
        git(&["log", "-1", "--no-show-signature", "--format=%cs", "HEAD"]),
    ) else {
        eprintln!("note: git queries failed; skipping checkout comparison");
        return;
    };
    assert_eq!(sha, head, "sha should equal HEAD short=12");
    assert_eq!(
        date.as_deref(),
        Some(cs.as_str()),
        "date should equal HEAD committer date"
    );
}

#[test]
fn test_version_long_flag_should_print_exactly_the_embedded_long_version() {
    let (out, _) = run(&["--version"]);
    assert_eq!(out, format!("hone {LONG}\n"));
}

#[test]
fn test_version_short_flag_should_print_exactly_the_embedded_long_version() {
    let (out, _) = run(&["-V"]);
    assert_eq!(out, format!("hone {LONG}\n"));
}

#[test]
fn test_embedded_long_version_should_have_a_valid_shape() {
    let _ = parse_long_version(LONG);
}

#[test]
fn test_embedded_long_version_should_match_checkout_when_resolvable() {
    assert_matches_checkout();
}

#[test]
fn test_help_should_use_receipts_tagline_not_universal_terminal() {
    let (out, err) = run(&["--help"]);
    let all = format!("{out}{err}");
    assert!(
        all.contains("The coding agent that shows its receipts"),
        "help was: {all}"
    );
    assert!(!all.contains("universal terminal"), "help was: {all}");
}
