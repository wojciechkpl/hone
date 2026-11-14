#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::Path;
use std::process::Command;

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

/// Parse "hone X.Y.Z (<12 hex> YYYY-MM-DD)" strictly; returns (sha, date).
fn parse_version_line(line: &str) -> (String, String) {
    let rest = line.strip_prefix("hone ").unwrap_or_default();
    assert!(!rest.is_empty(), "bad prefix: {line:?}");
    let (semver, tail) = rest.split_once(' ').unwrap_or_default();
    assert!(!tail.is_empty(), "no space after semver: {line:?}");
    let parts: Vec<&str> = semver.split('.').collect();
    assert!(
        parts.len() == 3
            && parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
        "bad semver in {line:?}"
    );
    let inner = tail
        .strip_prefix('(')
        .and_then(|t| t.strip_suffix(')'))
        .unwrap_or_default();
    assert!(!inner.is_empty(), "no parens: {line:?}");
    let (sha, date) = inner.split_once(' ').unwrap_or_default();
    assert!(!date.is_empty(), "no sha/date split: {line:?}");
    assert!(
        sha.len() == 12 && sha.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
        "sha must be 12 lowercase hex: {line:?}"
    );
    let d: Vec<&str> = date.split('-').collect();
    assert!(
        d.len() == 3
            && d[0].len() == 4
            && d[1].len() == 2
            && d[2].len() == 2
            && d.iter().all(|p| p.chars().all(|c| c.is_ascii_digit())),
        "date must be YYYY-MM-DD: {line:?}"
    );
    (sha.to_string(), date.to_string())
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

fn assert_matches_checkout(sha: &str, date: &str) {
    if git(&["rev-parse", "--is-inside-work-tree"]).as_deref() != Some("true") {
        eprintln!("note: not in a git checkout; skipping sha/date comparison");
        return;
    }
    let (Some(head), Some(cs)) = (
        git(&["rev-parse", "--short=12", "HEAD"]),
        git(&["log", "-1", "--format=%cs"]),
    ) else {
        eprintln!("note: git queries failed; skipping sha/date comparison");
        return;
    };
    assert_eq!(sha, head, "sha should equal HEAD short=12");
    assert_eq!(date, cs, "date should equal HEAD committer date");
}

fn single_line(out: &str) -> &str {
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 1, "expected exactly one line, got {out:?}");
    lines[0]
}

#[test]
fn test_version_long_flag_should_print_one_line_with_commit_and_date() {
    let (out, _) = run(&["--version"]);
    let (sha, date) = parse_version_line(single_line(&out));
    assert_matches_checkout(&sha, &date);
}

#[test]
fn test_version_short_flag_should_print_one_line_with_commit_and_date() {
    let (out, _) = run(&["-V"]);
    let (sha, date) = parse_version_line(single_line(&out));
    assert_matches_checkout(&sha, &date);
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
