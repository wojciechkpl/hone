#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! P2.5 CLI entry point (`docs/plans/policy-p2.5-spec.md` sections 2 and 12): with
//! `HONE_POLICY_BUILTINS=1` and no policy files the policy is non-empty, so a provider that
//! runs its own tools is refused (exit code 2) unless `HONE_ALLOW_UNGATED_PROVIDER=1`.
//!
//! The provider is a fake `claude` shell script on a private `PATH` (the CLI auto-detects
//! `claude` when no API key is set), so no network or real CLI is involved. HOME and the
//! working directory are temp dirs and the environment is cleared, so the developer's own
//! `~/.config/hone` is never read.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

struct Env {
    _tmp: tempfile::TempDir,
    bin_dir: PathBuf,
    home: PathBuf,
    wd: PathBuf,
}

fn env() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let bin_dir = root.join("bin");
    let home = root.join("home");
    let wd = root.join("wd");
    for d in [&bin_dir, &home, &wd] {
        std::fs::create_dir_all(d).unwrap();
    }
    let claude = bin_dir.join("claude");
    std::fs::write(&claude, "#!/bin/sh\necho 1.0.0\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    Env {
        _tmp: tmp,
        bin_dir,
        home,
        wd,
    }
}

fn hone(e: &Env, extra_env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hone"));
    cmd.env_clear()
        .env("HOME", &e.home)
        .env("PATH", format!("{}:/usr/bin:/bin", e.bin_dir.display()))
        .current_dir(&e.wd)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("run hone")
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn write(path: &Path, body: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

#[test]
fn builtins_without_files_refuse_a_provider_that_runs_its_own_tools() {
    let e = env();
    let o = hone(&e, &[("HONE_POLICY_BUILTINS", "1")], &["hi"]);
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert!(err.contains("runs its own tools"), "{err}");
    assert!(err.contains("HONE_ALLOW_UNGATED_PROVIDER=1"), "{err}");
}

#[test]
fn without_the_variable_and_without_files_the_provider_is_not_refused() {
    let e = env();
    let o = hone(&e, &[], &["hi"]);
    let err = stderr(&o);
    assert!(!err.contains("runs its own tools"), "{err}");
    assert_ne!(o.status.code(), Some(2), "{err}");
}

#[test]
fn only_the_exact_value_1_enables_builtins() {
    let e = env();
    for v in ["0", "true", "yes", ""] {
        let o = hone(&e, &[("HONE_POLICY_BUILTINS", v)], &["hi"]);
        let err = stderr(&o);
        assert!(
            !err.contains("runs its own tools"),
            "HONE_POLICY_BUILTINS={v:?} must not enable built-ins: {err}"
        );
    }
}

#[test]
fn the_override_variable_lets_the_run_continue_with_a_warning() {
    let e = env();
    let o = hone(
        &e,
        &[
            ("HONE_POLICY_BUILTINS", "1"),
            ("HONE_ALLOW_UNGATED_PROVIDER", "1"),
        ],
        &["hi"],
    );
    let err = stderr(&o);
    assert_ne!(o.status.code(), Some(2), "{err}");
    assert!(err.contains("warning"), "{err}");
    assert!(err.contains("HONE_ALLOW_UNGATED_PROVIDER=1"), "{err}");
}

#[test]
fn approving_a_builtin_id_is_valid_with_builtins_and_no_files() {
    let e = env();
    let o = hone(
        &e,
        &[
            ("HONE_POLICY_BUILTINS", "1"),
            ("HONE_ALLOW_UNGATED_PROVIDER", "1"),
        ],
        &["--approve", "builtin:git-push", "hi"],
    );
    let err = stderr(&o);
    assert_ne!(o.status.code(), Some(2), "{err}");
    assert!(!err.contains("--approve"), "{err}");
}

#[test]
fn approving_a_builtin_id_without_builtins_and_without_files_is_refused() {
    let e = env();
    let o = hone(&e, &[], &["--approve", "builtin:git-push", "hi"]);
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert!(err.contains("--approve"), "{err}");
}

#[test]
fn an_unknown_builtin_id_is_refused_even_with_builtins() {
    let e = env();
    let o = hone(
        &e,
        &[
            ("HONE_POLICY_BUILTINS", "1"),
            ("HONE_ALLOW_UNGATED_PROVIDER", "1"),
        ],
        &["--approve", "builtin:no-such-rule", "hi"],
    );
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert!(err.contains("builtin:no-such-rule"), "{err}");
}

#[test]
fn a_broken_policy_file_is_still_fatal_with_builtins() {
    let e = env();
    write(&e.wd.join(".hone/permissions.toml"), "this is [[ not toml");
    let o = hone(&e, &[("HONE_POLICY_BUILTINS", "1")], &["hi"]);
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert!(err.contains("permission policy error"), "{err}");
}

// -- 14.11 switch and messages ------------------------------------------------------------------------

#[test]
fn an_unrecognised_value_leaves_builtins_off_and_prints_one_warning_line() {
    let e = env();
    for v in ["true", "yes", "on", "1 ", "0", "2"] {
        let o = hone(&e, &[("HONE_POLICY_BUILTINS", v)], &["hi"]);
        let err = stderr(&o);
        assert!(
            !err.contains("runs its own tools"),
            "{v:?}: built-ins must stay off: {err}"
        );
        let lines: Vec<&str> = err
            .lines()
            .filter(|l| l.contains("HONE_POLICY_BUILTINS"))
            .collect();
        assert_eq!(
            lines.len(),
            1,
            "{v:?}: exactly one warning line expected: {err}"
        );
        assert!(lines[0].contains("off"), "{v:?}: {}", lines[0]);
    }
}

#[test]
fn unset_or_empty_print_no_unrecognised_value_warning() {
    let e = env();
    for extra in [vec![], vec![("HONE_POLICY_BUILTINS", "")]] {
        let o = hone(&e, &extra, &["hi"]);
        assert!(
            !stderr(&o).contains("HONE_POLICY_BUILTINS"),
            "{}",
            stderr(&o)
        );
    }
}

#[test]
fn the_refusal_names_the_builtins_switch_as_the_cause() {
    let e = env();
    let o = hone(&e, &[("HONE_POLICY_BUILTINS", "1")], &["hi"]);
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert!(err.contains("unset HONE_POLICY_BUILTINS"), "{err}");
    assert!(err.contains("use another provider"), "{err}");
    assert!(err.contains("HONE_ALLOW_UNGATED_PROVIDER=1"), "{err}");
}

#[test]
fn the_override_warning_says_tool_output_is_not_redacted() {
    let e = env();
    let o = hone(
        &e,
        &[
            ("HONE_POLICY_BUILTINS", "1"),
            ("HONE_ALLOW_UNGATED_PROVIDER", "1"),
        ],
        &["hi"],
    );
    assert!(stderr(&o).contains("not redacted"), "{}", stderr(&o));
}

#[test]
fn approving_a_deny_id_exits_2_with_the_always_denies_message() {
    let e = env();
    for id in ["builtin:pipe-to-shell", "builtin:sudo"] {
        let o = hone(
            &e,
            &[
                ("HONE_POLICY_BUILTINS", "1"),
                ("HONE_ALLOW_UNGATED_PROVIDER", "1"),
            ],
            &["--approve", id, "hi"],
        );
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(2), "{id}: {err}");
        assert!(
            err.contains(&format!(
                "--approve: rule '{id}' always denies; --approve only lifts ask rules"
            )),
            "{id}: {err}"
        );
    }
}

#[test]
fn approving_a_project_deny_rule_exits_2() {
    let e = env();
    write(
        &e.wd.join(".hone/permissions.toml"),
        "version = 2\n[[rules]]\nid = \"no-curl\"\ncommand = \"curl*\"\ndecision = \"deny\"\n",
    );
    let o = hone(
        &e,
        &[("HONE_ALLOW_UNGATED_PROVIDER", "1")],
        &["--approve", "no-curl", "hi"],
    );
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(2), "{err}");
    assert!(err.contains("always denies"), "{err}");
}
