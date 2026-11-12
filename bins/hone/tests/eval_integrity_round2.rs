#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F32 and F33 of `docs/plans/eval-profiles-spec.md` ("Final review"): the integrity hash is
//! recomputed after every run whatever its status and once at the end of the eval, excludes the
//! eval's own output directory, and covers mtime and symlink targets.
//!
//! Contract pinned (on top of `eval_integrity.rs`):
//! - F32: an agent that writes into an integrity directory and then times out, or hits an agent
//!   error, or finishes without any check afterwards, is caught: its run and the remaining runs
//!   are `infra_error` ("suite modified during the run") and `report.json` has
//!   `integrity_violated: true`;
//! - F33: the output directory of the eval is not hashed (an integrity dir `.` that contains it
//!   is not falsely flagged); `--check` ALSO rejects an integrity directory that contains the
//!   output directory, naming it (exit 2); a change that is reverted (same content, new mtime)
//!   before the next hash is still detected; a symlink inside an integrity directory whose
//!   target is under another integrity directory is hashed by the target's content.
//!
//! The test agent writes through the harness tool `tamper` (absolute path), scripted by the
//! builder; `__fail__` makes the scripted provider return an HTTP 400 (an agent error).

mod eval_support;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use eval_support::*;
use hone_eval::{AgentBuild, AgentBuilder};
use serde_json::json;

const MESSAGE: &str = "suite modified during the run";

fn suite(f: &Fx) {
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::create_dir_all(f.base.join("suite")).unwrap();
    std::fs::write(f.base.join("suite/ref.txt"), "original text\n").unwrap();
}

fn tamper_round(f: &Fx) -> Vec<hone_proto::ToolCallRequest> {
    vec![call(
        "c1",
        "tamper",
        json!({ "path": f.base.join("suite/ref.txt").display().to_string(), "text": "rewritten by the agent\n" }),
    )]
}

/// The agent of `t1` tampers, then runs `then`; other tasks' agents just answer.
fn builder_for(f: &Fx, then: Vec<Vec<hone_proto::ToolCallRequest>>) -> AgentBuilder {
    let first = tamper_round(f);
    Arc::new(move |b: AgentBuild| {
        let rounds = if b.task.id == "t1" {
            let mut r = vec![first.clone()];
            r.extend(then.clone());
            r
        } else {
            vec![]
        };
        Ok(agent_for(&b, rounds))
    })
}

fn two_tasks(timeout_secs: u32) -> String {
    format!(
        "[eval]\nintegrity = [\"suite\"]\n\n{}{}",
        task_toml("t1", "proj", "true").replace("timeout_secs = 30", &format!("timeout_secs = {timeout_secs}")),
        task_toml("t2", "proj", "true")
    )
}

async fn run(f: &Fx, raw: &str, b: AgentBuilder) -> PathBuf {
    runner(f, raw, b).run(&opts(f)).await.unwrap()
}

fn assert_tampered(run: &std::path::Path, tasks: &[&str]) {
    for t in tasks {
        let o = outcome(run, t);
        assert_eq!(o["status"], "infra_error", "{t}: {o}");
        assert!(o["message"].as_str().is_some_and(|m| m.contains(MESSAGE)), "{t}: {o}");
    }
    assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], true);
}

// ---- F32 -----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_f32_an_agent_that_writes_into_the_suite_and_then_times_out_should_be_caught() {
    let f = fx();
    suite(&f);
    let b = builder_for(&f, vec![vec![call("c2", "slow", json!({}))]]);

    let run = run(&f, &two_tasks(2), b).await;

    assert_tampered(&run, &["t1", "t2"]);
}

#[tokio::test]
async fn test_f32_an_agent_that_writes_into_the_suite_and_then_errors_should_be_caught() {
    let f = fx();
    suite(&f);
    let b = builder_for(&f, vec![vec![call("c2", "__fail__", json!({}))]]);

    let run = run(&f, &two_tasks(30), b).await;

    assert_tampered(&run, &["t1", "t2"]);
}

#[tokio::test]
async fn test_f32_a_change_by_the_last_run_with_no_check_after_it_should_be_caught_at_the_end() {
    let f = fx();
    suite(&f);
    let raw = format!("[eval]\nintegrity = [\"suite\"]\n\n{}", task_toml("t1", "proj", "true").replace("timeout_secs = 30", "timeout_secs = 2"));
    let b = builder_for(&f, vec![vec![call("c2", "slow", json!({}))]]);

    let run = run(&f, &raw, b).await;

    assert_tampered(&run, &["t1"]);
}

// ---- F33 -----------------------------------------------------------------------------------------

#[tokio::test]
async fn test_f33_the_output_directory_inside_an_integrity_directory_should_not_be_hashed() {
    let f = fx();
    suite(&f);
    // The integrity directory is the suite root, and the output directory is below it.
    let raw = format!("[eval]\nintegrity = [\".\"]\n\n{}{}", task_toml("t1", "proj", "true"), task_toml("t2", "proj", "true"));
    let r = runner(&f, &raw, quiet_builder(Arc::default()));

    let run = r
        .run(&hone_eval::EvalOptions {
            out_dir: f.base.join("eval-runs"),
            ..Default::default()
        })
        .await
        .unwrap();

    for t in ["t1", "t2"] {
        assert_eq!(outcome_in(&run, t)["status"], "pass", "{t}: the run's own files are not a suite change");
    }
    assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], false);
}

fn outcome_in(run: &std::path::Path, task: &str) -> serde_json::Value {
    read_json(&run_dir_of(run, "default", task, 1).join("outcome.json"))
}

#[test]
fn test_f33_check_should_reject_an_integrity_directory_that_contains_the_output_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (home, dir) = (root.join("home"), root.join("suite"));
    std::fs::create_dir_all(home.join(".config/hone")).unwrap();
    std::fs::create_dir_all(dir.join("proj")).unwrap();
    std::fs::write(home.join(".config/hone/config.toml"), "[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n").unwrap();
    std::fs::write(
        dir.join("tasks.toml"),
        format!("model = \"ollama:qwen\"\n\n[eval]\nintegrity = [\".\"]\n\n{}", task_toml("t1", "proj", "true")),
    )
    .unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_hone"))
        .args(["eval", "tasks.toml", "-o", "eval-runs", "--check"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(2), "stdout: {}", String::from_utf8_lossy(&out.stdout));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("eval-runs") && err.contains("integrity"), "names the output directory and the key: {err}");
}

#[tokio::test]
async fn test_f33_a_check_that_rewrites_and_restores_a_file_should_still_be_detected_through_its_mtime() {
    let f = fx();
    suite(&f);
    let raw = format!(
        "[eval]\nintegrity = [\"suite\"]\n\n{}{}",
        task_toml(
            "t1",
            "proj",
            "f=\"$EVAL_SUITE_DIR/suite/ref.txt\"; orig=$(cat \"$f\"); sleep 1; echo junk > \"$f\"; printf '%s\\n' \"$orig\" > \"$f\""
        ),
        task_toml("t2", "proj", "true")
    );

    let run = run(&f, &raw, quiet_builder(Arc::default())).await;

    assert_eq!(std::fs::read_to_string(f.base.join("suite/ref.txt")).unwrap(), "original text\n", "harness: the content is restored");
    assert_tampered(&run, &["t1", "t2"]);
}

#[tokio::test]
async fn test_f33_a_symlink_to_a_target_under_another_integrity_directory_should_follow_its_content() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::create_dir_all(f.base.join("checks")).unwrap();
    std::fs::create_dir_all(f.base.join("realdata")).unwrap();
    std::fs::write(f.base.join("realdata/x.txt"), "data v1\n").unwrap();
    std::os::unix::fs::symlink("../realdata/x.txt", f.base.join("checks/linked.txt")).unwrap();
    let raw = format!(
        "[eval]\nintegrity = [\"checks\", \"realdata\"]\n\n{}{}",
        task_toml("t1", "proj", "echo v2 > \"$EVAL_SUITE_DIR/checks/linked.txt\""), // writes THROUGH the link
        task_toml("t2", "proj", "true")
    );

    let run = run(&f, &raw, quiet_builder(Arc::default())).await;

    assert_eq!(std::fs::read_to_string(f.base.join("realdata/x.txt")).unwrap(), "v2\n", "harness: the target changed through the link");
    assert_tampered(&run, &["t1", "t2"]);
}
