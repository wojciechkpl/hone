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

fn git_cmd(dir: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        cmd.env_remove(var);
    }
    cmd
}

fn git_output(dir: &Path, args: &[&str]) -> Option<String> {
    let out = git_cmd(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let s = s.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn git_path(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = PathBuf::from(git_output(dir, &["rev-parse", "--git-path", name])?);
    Some(if p.is_absolute() { p } else { dir.join(p) })
}

/// Returns the git info plus the existing paths whose change should trigger a rebuild.
fn git_info_from_git(dir: &Path) -> Option<(GitInfo, Vec<PathBuf>)> {
    // Only trust git when this crate is tracked by the repository we found.
    git_cmd(dir)
        .args(["ls-files", "--error-unmatch", "--", "Cargo.toml"])
        .output()
        .ok()
        .filter(|o| o.status.success())?;

    let sha = normalize_sha(&git_output(dir, &["rev-parse", "--short=12", "HEAD"])?)?;
    let date = git_output(
        dir,
        &["log", "-1", "--no-show-signature", "--format=%cs", "HEAD"],
    )
    .and_then(|d| normalize_date(&d));

    let mut names = vec!["HEAD".to_string()];
    if let Some(branch) = git_output(dir, &["symbolic-ref", "-q", "HEAD"]) {
        if branch.starts_with("refs/heads/") {
            names.push(branch);
        }
    }
    names.extend(["packed-refs", "logs/HEAD", "reftable"].map(String::from));

    // `--git-path` prints paths whether or not they exist, and cargo treats a
    // missing rerun-if-changed path as always stale, so keep only existing ones.
    let paths = names
        .iter()
        .filter_map(|n| git_path(dir, n))
        .filter(|p| p.exists())
        .collect();
    Some((GitInfo { sha, date }, paths))
}

/// Where the commit info came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Override,
    CargoVcsInfo,
    Git,
    Unknown,
}

/// Outcome of [`resolve`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub info: Option<GitInfo>,
    pub source: Source,
    pub rerun_if_changed: Vec<PathBuf>,
}

/// Resolves commit info: overrides, `.cargo_vcs_info.json`, git, unknown.
pub fn resolve(manifest_dir: &Path, get_env: impl Fn(&str) -> Option<String>) -> Resolved {
    if let Some(info) = git_info_from_overrides(&get_env) {
        return Resolved {
            info: Some(info),
            source: Source::Override,
            rerun_if_changed: Vec::new(),
        };
    }
    let vcs = manifest_dir.join(".cargo_vcs_info.json");
    if let Some(sha) = std::fs::read_to_string(&vcs)
        .ok()
        .and_then(|j| sha_from_cargo_vcs_info(&j))
    {
        return Resolved {
            info: Some(GitInfo { sha, date: None }),
            source: Source::CargoVcsInfo,
            rerun_if_changed: vec![vcs],
        };
    }
    if let Some((info, paths)) = git_info_from_git(manifest_dir) {
        return Resolved {
            info: Some(info),
            source: Source::Git,
            rerun_if_changed: paths,
        };
    }
    Resolved {
        info: None,
        source: Source::Unknown,
        // Possibly missing on purpose: cargo then re-resolves on the next build.
        rerun_if_changed: vec![vcs],
    }
}

/// Resolves commit info and prints the cargo directives. Never fails the build.
pub fn emit() {
    println!("cargo:rerun-if-env-changed={ENV_SHA}");
    println!("cargo:rerun-if-env-changed={ENV_DATE}");

    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .unwrap_or_default();
    let pkg_version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();

    let r = resolve(&manifest_dir, |k| std::env::var(k).ok());
    for p in &r.rerun_if_changed {
        println!("cargo:rerun-if-changed={}", p.display());
    }
    if r.source == Source::Unknown {
        println!(
            "cargo:warning=git commit unknown; set {ENV_SHA} (and {ENV_DATE}) to embed it in --version"
        );
    }
    println!(
        "cargo:rustc-env=HONE_LONG_VERSION={}",
        long_version(&pkg_version, r.info.as_ref())
    );
}
