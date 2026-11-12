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

    // Not vacuous: a group whose toolchain is installed must actually PASS, so a script
    // that skips everything cannot satisfy this test. (c01-c03, m*, n*, x01, x03 need
    // python3; c04-c06, x02, x04 need node; c07-c08 need cargo.)
    for (tool, task) in [
        ("python3", "c01-py-fix-median"),
        ("python3", "n01-summarise-notes"),
        ("python3", "x01-ctx-free-shipping"),
        ("node", "c04-js-fix-flatten"),
        ("cargo", "c07-rs-fix-kvparse"),
    ] {
        let installed = Command::new(tool).arg("--version").output().is_ok_and(|o| o.status.success());
        if installed {
            assert!(
                text.lines().any(|l| l.starts_with(&format!("PASS  {task} "))),
                "{tool} is installed, so {task} must PASS (not skip):\n{text}"
            );
        }
    }

    // T12: no task is skipped for a toolchain that is installed.
    for tool in ["python3", "node", "cargo"] {
        let installed = Command::new(tool).arg("--version").output().is_ok_and(|o| o.status.success());
        if installed {
            let skipped: Vec<&str> = text
                .lines()
                .filter(|l| l.starts_with("SKIP ") && l.contains(tool))
                .collect();
            assert!(skipped.is_empty(), "{tool} is installed but tasks were skipped: {skipped:?}");
        }
    }
    // T11: the watchdog self-test ran and passed.
    assert!(text.contains("OK    watchdog: a timed-out check leaves nothing running"), "{text}");
    assert!(text.contains("OK    watchdog: a finished check leaves no background process"), "{text}");

    // The anti-cheating self-tests ran: deleted tests and the known cheats are rejected,
    // the alternative correct solutions accepted.
    for expected in [
        "OK    c01-py-fix-median/tests-deleted",
        "OK    c03-py-refactor-discount/constant-only",
        "OK    c03-py-refactor-discount/one-minus-rate",
        "OK    n01-summarise-notes/cat-notes",
        "OK    n01-summarise-notes/delete-notes",
        "OK    c01-py-fix-median/exit-at-import",
        "OK    c01-py-fix-median/forged-probe-output",
        "OK    c01-py-fix-median/rewrite-check-files",
        "OK    c01-py-fix-median/session-escape",
        "OK    m02-mem-staging-port/shadow-json",
        "OK    n02-table-to-csv/shadow-csv",
        "OK    n04-fill-config/shadow-configparser",
        "OK    n01-summarise-notes/shadow-unicodedata",
        "OK    no cheat could write into the suite",
        "OK    c01-py-fix-median/shadow-unittest-module",
        "OK    c01-py-fix-median/disarm-asserts",
    ] {
        if Command::new("python3").arg("--version").output().is_ok_and(|o| o.status.success()) {
            assert!(text.contains(expected), "missing '{expected}' in:\n{text}");
        }
    }

    if Command::new("cargo").arg("--version").output().is_ok_and(|o| o.status.success()) {
        assert!(text.contains("OK    c07-rs-fix-kvparse/cargo-runner-true"), "{text}");
        assert!(text.contains("OK    c07-rs-fix-kvparse/build-rs-rewrites-tests"), "{text}");
        assert!(text.contains("OK    c07-rs-fix-kvparse/process-exit"), "{text}");
    }
}
