#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! T5 of `docs/plans/eval-starter-suite-spec.md`: `eval/suites/starter/verify.sh`
//! proves every task's check fails on the untouched task and passes with the
//! reference solution, without a model. The test fails if any task fails either
//! condition or if `tasks.toml` and the directories disagree. Tasks whose toolchain
//! (python3, node, cargo) is missing are skipped by the script, not failed.
//! Skipped entirely when there is no `sh`.

use std::path::PathBuf;
use std::process::Command;

fn suite_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../eval/suites/starter")
}

#[test]
fn test_t5_every_starter_task_check_should_fail_before_and_pass_with_the_reference() {
    if Command::new("sh").arg("-c").arg("true").status().is_err() {
        eprintln!("skipping: no sh available");
        return;
    }
    let suite = suite_dir();
    let out = Command::new("sh")
        .arg(suite.join("verify.sh"))
        // The checks run their own cargo builds: keep this process's cargo settings out.
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("RUSTFLAGS")
        .output()
        .expect("run verify.sh");
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "verify.sh failed:\n{text}");
    let tasks = std::fs::read_to_string(suite.join("tasks.toml")).unwrap();
    let declared = tasks.matches("[[tasks]]").count();
    assert_eq!(declared, 20, "the starter suite has 20 tasks");
    let reported = text.lines().filter(|l| l.starts_with("PASS ") || l.starts_with("SKIP ")).count();
    assert_eq!(reported, declared, "every task is reported as PASS or SKIP:\n{text}");
    assert!(!text.lines().any(|l| l.starts_with("FAIL ")), "{text}");
}
