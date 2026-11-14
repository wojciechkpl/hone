//! Build-script helper: embeds the git commit and date in `--version` output.
//!
//! Call [`emit`] from a `build.rs`; it sets `HONE_LONG_VERSION` for the crate.

use std::path::{Path, PathBuf};
use std::process::Command;

const ENV_SHA: &str = "HONE_BUILD_GIT_SHA";
const ENV_DATE: &str = "HONE_BUILD_GIT_DATE";
const SHA_LEN: usize = 12;

/// Commit identity of a build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitInfo {
    /// Abbreviated (at most 12 hex chars) lowercase commit sha.
    pub sha: String,
    /// Commit date as `YYYY-MM-DD`, when known.
    pub date: Option<String>,
}

/// Formats `<version> (<sha> <date>)`, degrading gracefully.
pub fn long_version(pkg_version: &str, git: Option<&GitInfo>) -> String {
    match git {
        Some(GitInfo { sha, date: Some(d) }) => format!("{pkg_version} ({sha} {d})"),
        Some(GitInfo { sha, date: None }) => format!("{pkg_version} ({sha})"),
        None => format!("{pkg_version} (commit unknown)"),
    }
}

/// Accepts 7..=40 hex chars (after trimming), lowercases and truncates to 12.
pub fn normalize_sha(raw: &str) -> Option<String> {
    let s = raw.trim();
    if !(7..=40).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(s[..s.len().min(SHA_LEN)].to_ascii_lowercase())
}

/// Accepts exactly `YYYY-MM-DD` (after trimming).
pub fn normalize_date(raw: &str) -> Option<String> {
    let s = raw.trim();
    let b = s.as_bytes();
    let ok = b.len() == 10
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            _ => c.is_ascii_digit(),
        });
    ok.then(|| s.to_string())
}

/// Extracts the commit sha from the contents of `.cargo_vcs_info.json`.
pub fn sha_from_cargo_vcs_info(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    normalize_sha(v.get("git")?.get("sha1")?.as_str()?)
}

/// Reads the override variables through `get`. A valid sha is required.
pub fn git_info_from_overrides(get: impl Fn(&str) -> Option<String>) -> Option<GitInfo> {
    let sha = normalize_sha(&get(ENV_SHA)?)?;
    let date = get(ENV_DATE).and_then(|d| normalize_date(&d));
    Some(GitInfo { sha, date })
}

fn git_output(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

fn git_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(git_output(dir, &["rev-parse", "--git-path", name])?);
    Some(if p.is_absolute() { p } else { dir.join(p) })
}

/// Returns the git info plus the paths whose change should trigger a rebuild.
fn git_info_from_git(dir: &Path) -> Option<(GitInfo, Vec<PathBuf>)> {
    let sha = normalize_sha(&git_output(dir, &["rev-parse", "--short=12", "HEAD"])?)?;
    let date = git_output(dir, &["log", "-1", "--format=%cs"]).and_then(|d| normalize_date(&d));

    let mut paths = Vec::new();
    paths.extend(git_path(dir, "HEAD"));
    if let Some(branch) = git_output(dir, &["symbolic-ref", "-q", "HEAD"])
        && branch.starts_with("refs/heads/")
    {
        paths.extend(git_path(dir, &branch));
    }
    paths.extend(git_path(dir, "packed-refs"));
    Some((GitInfo { sha, date }, paths))
}

/// Resolves commit info and prints the cargo directives. Never fails the build.
///
/// Order: env overrides, `.cargo_vcs_info.json`, git, none.
pub fn emit() {
    println!("cargo:rerun-if-env-changed={ENV_SHA}");
    println!("cargo:rerun-if-env-changed={ENV_DATE}");

    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from);
    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();

    let info = git_info_from_overrides(|k| std::env::var(k).ok())
        .or_else(|| {
            let dir = manifest_dir.as_ref()?;
            let vcs = dir.join(".cargo_vcs_info.json");
            let sha = sha_from_cargo_vcs_info(&std::fs::read_to_string(&vcs).ok()?)?;
            Some(GitInfo { sha, date: None })
        })
        .or_else(|| {
            let (info, paths) = git_info_from_git(manifest_dir.as_ref()?)?;
            for p in paths {
                println!("cargo:rerun-if-changed={}", p.display());
            }
            Some(info)
        });

    println!(
        "cargo:rustc-env=HONE_LONG_VERSION={}",
        long_version(&pkg_version, info.as_ref())
    );
}
