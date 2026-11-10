#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! R4 (`docs/plans/config-apply-spec.md` section 3b): config files load like `permissions.toml`.
//!
//! PINNED API:
//!   `ConfigLayer::load(path)`      -- user layer: a regular file (symlinks followed), at most
//!                                     64 KiB, valid UTF-8; `Ok(None)` when missing.
//!   `ConfigLayer::load_project(workdir)` -- project layer: `<workdir>/.hone` and
//!                                     `<workdir>/.hone/config.toml` must not be symlinks; then
//!                                     the same file checks. `Ok(None)` when the file is missing.
//!   Parse errors carry the message and `line N:M` only, never a line of the file.
//!
//! The "non-regular file" case uses `/dev/null` (a character device that reads as empty), so
//! an implementation that reads before checking fails the test instead of hanging on `/dev/zero`.

use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use hone_config::ConfigError;
use hone_config::resolve::ConfigLayer;

const LEAK: &str = "leak_marker_line_zzz";

fn canonical_tmp() -> (tempfile::TempDir, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let d = std::fs::canonicalize(t.path()).unwrap();
    (t, d)
}

fn reason(e: &ConfigError) -> String {
    match e {
        ConfigError::InvalidField { reason, .. } => reason.clone(),
        other => panic!("expected InvalidField, got {other:?}"),
    }
}

fn err_path(e: &ConfigError) -> PathBuf {
    match e {
        ConfigError::InvalidField { path, .. } => path.clone(),
        other => panic!("expected InvalidField, got {other:?}"),
    }
}

// -- parse errors never echo the file ------------------------------------------------------------

#[test]
fn test_r4_parse_error_has_line_and_column_but_no_file_content() {
    let (_t, dir) = canonical_tmp();
    let f = dir.join("c.toml");
    std::fs::write(&f, format!("# fine\n{LEAK} = = 1\n")).unwrap();

    let e = ConfigLayer::load(&f).expect_err("broken");

    let r = reason(&e);
    assert!(!r.contains(LEAK), "the reason must not echo file content: {r}");
    assert!(r.contains("line 2"), "the reason should locate the error: {r}");
    assert!(!e.to_string().contains(LEAK), "{e}");
}

#[test]
fn test_r4_from_toml_parse_error_has_no_file_content_either() {
    let e = ConfigLayer::from_toml("/x/config.toml", &format!("{LEAK} = = 1\n")).expect_err("broken");
    assert!(!reason(&e).contains(LEAK), "{}", reason(&e));
}

// -- file checks -------------------------------------------------------------------------------------

#[test]
fn test_r4_non_regular_file_is_an_error() {
    let e = ConfigLayer::load(Path::new("/dev/null")).expect_err("a device is not a config file");
    assert_eq!(err_path(&e), PathBuf::from("/dev/null"));
}

#[test]
fn test_r4_file_over_64_kib_is_an_error_and_exactly_64_kib_is_fine() {
    let (_t, dir) = canonical_tmp();
    let line = "# padding padding padding padding padding padding padding\n";
    let mut body = String::new();
    while body.len() + line.len() <= 64 * 1024 {
        body.push_str(line);
    }
    body.push_str(&"#".repeat(64 * 1024 - body.len()));
    assert_eq!(body.len(), 64 * 1024);
    let ok = dir.join("exact.toml");
    std::fs::write(&ok, &body).unwrap();
    assert!(ConfigLayer::load(&ok).unwrap().is_some(), "exactly 64 KiB is allowed");

    body.push('#');
    let big = dir.join("big.toml");
    std::fs::write(&big, &body).unwrap();
    let e = ConfigLayer::load(&big).expect_err("over 64 KiB");
    assert_eq!(err_path(&e), big);
    assert!(reason(&e).contains("64"), "{}", reason(&e));
}

#[test]
fn test_r4_invalid_utf8_is_an_error() {
    let (_t, dir) = canonical_tmp();
    let f = dir.join("bad.toml");
    std::fs::write(&f, [b'#', b' ', 0xff, 0xfe, b'\n']).unwrap();
    let e = ConfigLayer::load(&f).expect_err("not UTF-8");
    assert_eq!(err_path(&e), f);
    assert!(reason(&e).to_lowercase().contains("utf-8"), "{}", reason(&e));
}

#[test]
fn test_r4_user_file_symlinked_to_a_regular_file_still_loads() {
    // Dotfile managers symlink ~/.config files; permissions.toml follows symlinks too.
    let (_t, dir) = canonical_tmp();
    let real = dir.join("real.toml");
    std::fs::write(&real, "default_model = \"openai:x\"\n").unwrap();
    let link = dir.join("link.toml");
    symlink(&real, &link).unwrap();
    assert!(ConfigLayer::load(&link).unwrap().is_some());
}

// -- the project layer: no symlinks --------------------------------------------------------------------

fn project_dir(dir: &Path) -> PathBuf {
    let hone = dir.join(".hone");
    std::fs::create_dir_all(&hone).unwrap();
    hone
}

#[test]
fn test_r4_load_project_reads_a_regular_file() {
    let (_t, dir) = canonical_tmp();
    std::fs::write(project_dir(&dir).join("config.toml"), "default_model = \"openai:p\"\n").unwrap();
    let l = ConfigLayer::load_project(&dir).unwrap().expect("a layer");
    assert_eq!(l.path, dir.join(".hone/config.toml"));
}

#[test]
fn test_r4_load_project_without_a_file_is_none() {
    let (_t, dir) = canonical_tmp();
    assert!(ConfigLayer::load_project(&dir).unwrap().is_none());
    project_dir(&dir);
    assert!(ConfigLayer::load_project(&dir).unwrap().is_none());
}

#[test]
fn test_r4_symlinked_project_file_is_an_error_that_does_not_leak_the_target() {
    let (_t, dir) = canonical_tmp();
    let outside = dir.join("outside.txt");
    std::fs::write(&outside, format!("{LEAK} = = 1\n")).unwrap();
    symlink(&outside, project_dir(&dir).join("config.toml")).unwrap();

    let e = ConfigLayer::load_project(&dir).expect_err("symlinked project config");

    assert_eq!(err_path(&e), dir.join(".hone/config.toml"));
    assert!(reason(&e).to_lowercase().contains("symlink"), "{}", reason(&e));
    assert!(!e.to_string().contains(LEAK), "{e}");
}

#[test]
fn test_r4_symlinked_project_file_to_valid_toml_is_still_an_error() {
    let (_t, dir) = canonical_tmp();
    let outside = dir.join("valid.txt");
    std::fs::write(&outside, "default_model = \"openai:x\"\n").unwrap();
    symlink(&outside, project_dir(&dir).join("config.toml")).unwrap();
    assert!(ConfigLayer::load_project(&dir).is_err());
}

#[test]
fn test_r4_symlinked_dot_hone_directory_is_an_error() {
    let (_t, dir) = canonical_tmp();
    let real = dir.join("elsewhere");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("config.toml"), format!("{LEAK} = = 1\n")).unwrap();
    symlink(&real, dir.join(".hone")).unwrap();

    let e = ConfigLayer::load_project(&dir).expect_err("symlinked .hone");

    assert_eq!(err_path(&e), dir.join(".hone"));
    assert!(!e.to_string().contains(LEAK), "{e}");
}
