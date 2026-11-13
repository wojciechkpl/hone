#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F31 of `docs/plans/eval-profiles-spec.md` ("Added from the suite review"): suite integrity.
//!
//! Contract pinned:
//! - `[eval] integrity = ["<dir>", ...]` (paths relative to the tasks file; each must be an
//!   existing directory, else `--check` exits 2 naming the path);
//! - the runner hashes those directories (every file's path and content) before the first run and
//!   after every success check; on any change (a file modified, added or deleted) the current run
//!   and every remaining run end `infra_error` with a `message` containing
//!   `suite modified during the run`;
//! - `report.json` has `"integrity_violated": true` then, and `false` otherwise (also when no
//!   `integrity` is configured).
//!
//! The check runs with `EVAL_SUITE_DIR` set to the tasks file's directory (F11), so a check can
//! modify the suite through it, which is what these tests do.

mod eval_support;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use eval_support::*;

const MESSAGE: &str = "suite modified during the run";

fn suite(f: &Fx) {
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::create_dir_all(f.base.join("suite/nested")).unwrap();
    std::fs::write(f.base.join("suite/ref.txt"), "original text\n").unwrap();
    std::fs::write(f.base.join("suite/nested/check.sh"), "exit 0\n").unwrap();
}

fn tasks(first_check: &str) -> String {
    format!(
        "[eval]\nintegrity = [\"suite\"]\n\n{}{}",
        task_toml("t1", "proj", first_check),
        task_toml("t2", "proj", "true")
    )
}

async fn run_of(f: &Fx, raw: &str) -> PathBuf {
    let r = runner(f, raw, quiet_builder(Arc::default()));
    r.run(&opts(f)).await.unwrap()
}

#[tokio::test]
async fn test_f31_a_check_that_modifies_the_suite_should_invalidate_its_run_and_the_remaining_ones() {
    let f = fx();
    suite(&f);

    let run = run_of(&f, &tasks("echo changed >> \"$EVAL_SUITE_DIR/suite/ref.txt\"")).await;

    for task in ["t1", "t2"] {
        let o = outcome(&run, task);
        assert_eq!(o["status"], "infra_error", "{task}: {o}");
        assert!(o["message"].as_str().is_some_and(|m| m.contains(MESSAGE)), "{task}: {o}");
    }
    let report = read_json(&run.join("report.json"));
    assert_eq!(report["integrity_violated"], true, "{report}");
}

#[tokio::test]
async fn test_f31_added_and_deleted_files_under_an_integrity_dir_count_as_changes() {
    for check in [
        "echo new > \"$EVAL_SUITE_DIR/suite/nested/added.txt\"",
        "rm \"$EVAL_SUITE_DIR/suite/nested/check.sh\"",
    ] {
        let f = fx();
        suite(&f);

        let run = run_of(&f, &tasks(check)).await;

        assert_eq!(outcome(&run, "t1")["status"], "infra_error", "{check}");
        assert!(outcome(&run, "t2")["message"].as_str().is_some_and(|m| m.contains(MESSAGE)), "{check}");
        assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], true);
    }
}

#[tokio::test]
async fn test_f31_an_untouched_suite_should_run_normally_and_not_be_flagged() {
    let f = fx();
    suite(&f);

    let run = run_of(&f, &tasks("test -f \"$EVAL_SUITE_DIR/suite/ref.txt\"")).await;

    for task in ["t1", "t2"] {
        assert_eq!(outcome(&run, task)["status"], "pass", "{task}");
    }
    assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], false);
}

#[tokio::test]
async fn test_f31_a_run_without_integrity_dirs_should_not_be_flagged() {
    let f = fx();
    suite(&f);
    let raw = format!("{}{}", task_toml("t1", "proj", "true"), task_toml("t2", "proj", "true"));

    let run = run_of(&f, &raw).await;

    assert_eq!(outcome(&run, "t1")["status"], "pass");
    assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], false);
}

// ---- --check -----------------------------------------------------------------------------------------

fn check(raw: &str, make_suite: bool) -> std::process::Output {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, dir) = (root.join("home"), root.join("evals"));
    std::fs::create_dir_all(home.join(".config/hone")).unwrap();
    std::fs::create_dir_all(dir.join("proj")).unwrap();
    if make_suite {
        std::fs::create_dir_all(dir.join("suite")).unwrap();
    }
    std::fs::write(home.join(".config/hone/config.toml"), "[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n").unwrap();
    std::fs::write(dir.join("tasks.toml"), raw).unwrap();
    Command::new(env!("CARGO_BIN_EXE_hone"))
        .args(["eval"])
        .arg(dir.join("tasks.toml"))
        .arg("--check")
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn test_f31_check_should_reject_a_missing_integrity_path_and_accept_an_existing_one() {
    let body = |dir: &str| {
        format!("model = \"ollama:qwen\"\n\n[eval]\nintegrity = [\"{dir}\"]\n\n{}", task_toml("t1", "proj", "true"))
    };

    let missing = check(&body("nope"), true);
    assert_eq!(missing.status.code(), Some(2));
    let err = String::from_utf8_lossy(&missing.stderr).into_owned();
    assert!(err.contains("nope") && err.contains("integrity"), "names the path and the key: {err}");

    let ok = check(&body("suite"), true);
    assert_eq!(ok.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&ok.stderr));
}
