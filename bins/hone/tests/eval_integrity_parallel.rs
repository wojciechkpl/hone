#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F34 of `docs/plans/eval-profiles-spec.md` ("Final review"): with `--jobs` above 1 the
//! integrity hash is re-checked right before each check starts, and every run whose check
//! started after the last good hash is marked `infra_error` ("suite modified during the run"),
//! not only the run where the violation was found.
//!
//! Ordering is deterministic without sleeps: agents use the harness tools `tamper` (writes a
//! file anywhere), `waitfor` (polls for a file, or for a file's content to change) and `slow`.
//! Task `a` tampers with `suite/ref.txt` and then stays busy until its 6 s timeout; task `b`
//! has two variants (both with `jobs = 2`, so both start at once):
//! - after: `b`'s agent waits until the tamper has happened, so `b`'s check starts after it;
//! - before (control): `b` finishes (its check writes a marker) well before `a` is allowed to
//!   tamper (`a`'s agent waits for the marker plus 1.5 s).
//!
//! Pinned: after-variant: `b` is `infra_error` with the message, `integrity_violated` is true,
//! and `b`'s success check did NOT run to completion as a pass (its marker file is absent: the
//! check is not started once the pre-check hash has found the change). Control: `b` keeps its
//! real status (`pass`), `a` is `infra_error`, `integrity_violated` is true.

mod eval_support;

use std::path::Path;
use std::sync::Arc;

use eval_support::*;
use hone_eval::{AgentBuild, AgentBuilder, EvalOptions};
use serde_json::json;

const MESSAGE: &str = "suite modified during the run";
const ORIGINAL: &str = "original text\n";

fn suite(f: &Fx) {
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::create_dir_all(f.base.join("suite")).unwrap();
    std::fs::create_dir_all(f.base.join("markers")).unwrap();
    std::fs::write(f.base.join("suite/ref.txt"), ORIGINAL).unwrap();
}

fn abs(f: &Fx, rel: &str) -> String {
    f.base.join(rel).display().to_string()
}

fn tasks(b_check: &str) -> String {
    format!(
        "[eval]\nintegrity = [\"suite\"]\n\n{}{}",
        task_toml("a", "proj", "true").replace("timeout_secs = 30", "timeout_secs = 6"),
        task_toml("b", "proj", b_check)
    )
}

fn builder(a_rounds: Vec<Vec<hone_proto::ToolCallRequest>>, b_rounds: Vec<Vec<hone_proto::ToolCallRequest>>) -> AgentBuilder {
    Arc::new(move |b: AgentBuild| {
        let rounds = if b.task.id == "a" { a_rounds.clone() } else { b_rounds.clone() };
        Ok(agent_for(&b, rounds))
    })
}

async fn run(f: &Fx, raw: &str, b: AgentBuilder) -> std::path::PathBuf {
    runner(f, raw, b)
        .run(&EvalOptions {
            jobs: 2,
            ..opts(f)
        })
        .await
        .unwrap()
}

fn exists(p: &Path) -> bool {
    p.exists()
}

#[tokio::test]
async fn test_f34_a_run_whose_check_starts_after_the_tamper_should_be_invalidated_too() {
    let f = fx();
    suite(&f);
    let tamper = call(
        "a1",
        "tamper",
        json!({ "path": abs(&f, "suite/ref.txt"), "text": "rewritten by the agent\n" }),
    );
    // a: tamper, then stay busy until the 6 s timeout.
    let a = vec![vec![tamper], vec![call("a2", "slow", json!({}))]];
    // b: wait until the tamper happened (and a little longer), then let its check start.
    let b = vec![vec![call(
        "b1",
        "waitfor",
        json!({ "path": abs(&f, "suite/ref.txt"), "unless_equals": ORIGINAL, "sleep_after_ms": 300 }),
    )]];
    let marker = f.base.join("markers/b-check-ran");
    let raw = tasks(&format!("touch '{}'", marker.display()));

    let run = run(&f, &raw, builder(a, b)).await;

    let ob = outcome(&run, "b");
    assert_eq!(ob["status"], "infra_error", "b's check started after the tamper: {ob}");
    assert!(ob["message"].as_str().is_some_and(|m| m.contains(MESSAGE)), "{ob}");
    assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], true);
    assert!(!exists(&marker), "b's success check must not be started once the pre-check hash found the change");
}

#[tokio::test]
async fn test_f34_a_run_whose_check_finished_before_the_tamper_should_keep_its_real_status() {
    let f = fx();
    suite(&f);
    let done = f.base.join("markers/b-done");
    // b: finish at once; its check writes the marker.
    let b = vec![];
    // a: wait for b's marker plus 1.5 s (b's whole run is over by then), then tamper and stay busy.
    let a = vec![
        vec![call("a0", "waitfor", json!({ "path": done.display().to_string(), "sleep_after_ms": 1500 }))],
        vec![call(
            "a1",
            "tamper",
            json!({ "path": abs(&f, "suite/ref.txt"), "text": "rewritten by the agent\n" }),
        )],
        vec![call("a2", "slow", json!({}))],
    ];
    let raw = tasks(&format!("touch '{}'", done.display()));

    let run = run(&f, &raw, builder(a, b)).await;

    assert_eq!(outcome(&run, "b")["status"], "pass", "b was done before anything changed");
    assert_eq!(outcome(&run, "a")["status"], "infra_error");
    assert!(outcome(&run, "a")["message"].as_str().is_some_and(|m| m.contains(MESSAGE)));
    assert_eq!(read_json(&run.join("report.json"))["integrity_violated"], true);
}
