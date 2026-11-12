#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F12, F15, F16, F18, F19, F21, F22 and F23 (suite SHA) of `docs/plans/eval-profiles-spec.md`
//! (review round 1) through the seams of `eval_runs.rs`; harness in `eval_support/`.
//!
//! New API pinned:
//! - (F16 and the F21 size warning, with their API, are in `eval_review1_api.rs`.)
//! - manifest fields: `hone_git_sha` (string or null: the build's git SHA, from build
//!   metadata), `suite_git_sha` (the tasks file directory's `git rev-parse HEAD`, or null),
//!   `warnings`.
//! - `outcome.json` of a run that never started because of an unsafe copy has
//!   `status: "infra_error"` and a `message` naming the offending path.
//! - Skipped special files (FIFOs, sockets, devices) are listed in the run's `check.txt`
//!   with the word `skipped`.

mod eval_support;

use std::os::unix::fs::symlink;
use std::sync::{Arc, Mutex};

use eval_support::*;
use hone_eval::tasks::{ExpandOptions, TasksFile};
use hone_eval::{AgentBuild, AgentBuilder};
use hone_memory::{Memory, MemoryKind};
use serde_json::json;

// ---- F12: a copy never shares git state --------------------------------------------------------

/// A main repo with one commit, a linked worktree `wt` on branch `feature`, and (in the
/// worktree) a modified tracked file and an untracked one. Returns (main, worktree).
fn repo_with_worktree(base: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let main = base.join("main");
    std::fs::create_dir_all(&main).unwrap();
    git(&main, &["init", "-q", "-b", "trunk"]);
    std::fs::write(main.join("a.txt"), "one\n").unwrap();
    std::fs::create_dir_all(main.join("sub")).unwrap();
    std::fs::write(main.join("sub/b.txt"), "bee\n").unwrap();
    git(&main, &["add", "."]);
    git(&main, &["commit", "-q", "-m", "first"]);
    let wt = base.join("wt");
    git(&main, &["worktree", "add", "-q", "-b", "feature", wt.to_str().unwrap()]);
    std::fs::write(wt.join("a.txt"), "one, edited\n").unwrap();
    std::fs::write(wt.join("untracked.txt"), "new\n").unwrap();
    (main, wt)
}

const COMMIT_IN_COPY: &str = "git rev-parse HEAD > head-before.txt && git -c user.name=e -c user.email=e@e commit -q -am evalcommit && git checkout -q -b evalbranch";

fn snapshot(dir: &std::path::Path) -> Vec<String> {
    vec![
        git(dir, &["rev-parse", "HEAD"]),
        git(dir, &["rev-parse", "--abbrev-ref", "HEAD"]),
        git(dir, &["branch", "--list"]),
        git(dir, &["status", "--porcelain"]),
    ]
}

#[tokio::test]
async fn test_f12_a_git_worktree_copy_should_not_move_the_originals_branch_or_add_commits() {
    let f = fx();
    let (main, wt) = repo_with_worktree(&f.base);
    let (main_before, wt_before) = (snapshot(&main), snapshot(&wt));
    let head_before = git(&wt, &["rev-parse", "HEAD"]);
    let raw = task_toml("t", "wt", COMMIT_IN_COPY);
    let built = Arc::new(Mutex::new(Vec::new()));
    let r = runner(&f, &raw, quiet_builder(built));

    let run = r
        .run(&hone_eval::EvalOptions {
            keep_workdirs: true,
            ..opts(&f)
        })
        .await
        .unwrap();

    let o = outcome(&run, "t");
    assert_eq!(o["status"], "pass", "the check committed in the copy: {o}");
    assert_eq!(snapshot(&main), main_before, "the main repository is untouched");
    assert_eq!(snapshot(&wt), wt_before, "the original worktree is untouched");
    assert!(git(&main, &["branch", "--list", "evalbranch"]).is_empty());
    assert_eq!(git(&main, &["rev-list", "--count", "--all"]), "1", "no commits were added");
    // The copy is an independent repository at the same HEAD with the same working tree.
    let copy = run_dir_of(&run, "default", "t", 1).join("workdir");
    assert_eq!(std::fs::read_to_string(copy.join("head-before.txt")).unwrap().trim(), head_before);
    assert_eq!(git(&copy, &["rev-parse", "HEAD~1"]), head_before, "the copy's commit sits on the original HEAD");
    assert_eq!(std::fs::read_to_string(copy.join("a.txt")).unwrap(), "one, edited\n");
    assert_eq!(std::fs::read_to_string(copy.join("untracked.txt")).unwrap(), "new\n");
    let git_dir = git(&copy, &["rev-parse", "--absolute-git-dir"]);
    assert!(
        git_dir.starts_with(copy.to_str().unwrap()),
        "the copy's git dir lives inside the copy, not at {git_dir}"
    );

    // A plain repository (a `.git` directory) with uncommitted changes stays isolated too.
    std::fs::write(main.join("a.txt"), "one, locally edited\n").unwrap();
    let before = snapshot(&main);
    let r = runner(&f, &task_toml("p", "main", COMMIT_IN_COPY), quiet_builder(Arc::default()));
    let run = r.run(&hone_eval::EvalOptions { keep_workdirs: true, ..opts(&f) }).await.unwrap();
    assert_eq!(outcome(&run, "p")["status"], "pass");
    assert_eq!(snapshot(&main), before);
    let copy = run_dir_of(&run, "default", "p", 1).join("workdir");
    assert_eq!(std::fs::read_to_string(copy.join("a.txt")).unwrap(), "one, locally edited\n");
}

#[tokio::test]
async fn test_f12_a_task_directory_inside_a_worktree_should_get_an_independent_repository_too() {
    let f = fx();
    let (main, wt) = repo_with_worktree(&f.base);
    let before = (snapshot(&main), snapshot(&wt));
    let raw = task_toml("t", "wt/sub", "git -c user.name=e -c user.email=e@e commit -q --allow-empty -m evalcommit");
    let r = runner(&f, &raw, quiet_builder(Arc::default()));

    let run = r.run(&opts(&f)).await.unwrap();

    let o = outcome(&run, "t");
    assert!(
        o["status"] == "pass" || o["status"] == "infra_error",
        "either a working independent repository or a clear refusal: {o}"
    );
    if o["status"] == "infra_error" {
        assert!(o["message"].as_str().is_some_and(|m| m.contains("git")), "{o}");
    }
    assert_eq!((snapshot(&main), snapshot(&wt)), before, "the originals never change");
    assert_eq!(git(&wt, &["rev-list", "--count", "HEAD"]), "1");
}

// ---- F15: runs recall like the CLI ----------------------------------------------------------------

fn memory_line(id: &str, kind: MemoryKind, content: &str) -> String {
    serde_json::to_string(&Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    })
    .unwrap()
}

#[tokio::test]
async fn test_f15_outcome_rows_should_not_crowd_the_project_memory_out_of_recall() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::create_dir_all(f.base.join("fixtures")).unwrap();
    let mut lines: Vec<String> = (0..20)
        .map(|i| memory_line(&format!("out-{i}"), MemoryKind::Outcome, &format!("zorblax zorblax zorblax branch rebasing log {i}")))
        .collect();
    lines.push(memory_line("the-project", MemoryKind::Project, "zorblax branch rebasing policy: rebase before merge"));
    std::fs::write(f.base.join("fixtures/mem.jsonl"), lines.join("\n") + "\n").unwrap();
    let raw = format!(
        "[[tasks]]\nid = \"t\"\nprompt = \"how do we handle zorblax branch rebasing\"\nsuccess_check = \"true\"\nworking_dir = \"proj\"\nmemory_fixture = \"fixtures/mem.jsonl\"\n"
    );
    let r = runner(&f, &raw, quiet_builder(Arc::default()));

    let run = r.run(&opts(&f)).await.unwrap();

    let receipts = std::fs::read_to_string(run_dir_of(&run, "default", "t", 1).join("receipts.json")).unwrap();
    assert!(receipts.contains("the-project"), "the recall receipt names the project memory: {receipts}");
    assert!(!receipts.contains("out-"), "outcome logs are excluded before the cap");
}

const BASIC: &str = "[[tasks]]\nid = \"t\"\nprompt = \"p\"\nsuccess_check = \"true\"\n\n[profiles.lean.context]\ncompactor = \"trim-only\"\n";

// ---- F18: unknown keys ------------------------------------------------------------------------------

#[test]
fn test_f18_unknown_keys_in_tasks_eval_and_profile_tables_should_be_errors_naming_file_and_key() {
    let cases = [
        ("[[tasks]]\nid = \"a\"\nprompt = \"p\"\nsuccess_check = \"true\"\ntimout_secs = 5\n", "timout_secs"),
        ("[[tasks]]\nid = \"a\"\nprompt = \"p\"\nsuccess_check = \"true\"\n[eval]\nrepeat = 3\n", "repeat"),
        ("[[tasks]]\nid = \"a\"\nprompt = \"p\"\nsuccess_check = \"true\"\n[eval]\nbase_profile = \"default\"\nbase = 1\n", "base"),
        ("[[tasks]]\nid = \"a\"\nprompt = \"p\"\nsuccess_check = \"true\"\n[profiles.p.context]\ncolour = \"x\"\n", "colour"),
    ];
    for (raw, key) in cases {
        let e = TasksFile::from_toml_str("/evals/tasks.toml", raw).err().unwrap_or_else(|| panic!("should fail: {raw}")).to_string();
        assert!(e.contains("/evals/tasks.toml") && e.contains(key), "{key}: {e}");
    }
}

// ---- F19: symlinks and special files ----------------------------------------------------------------------

// Sync test: the run goes on its own OS thread, so a copy that blocks forever on a FIFO
// (the current behaviour) fails the test by timeout instead of hanging the whole binary.
#[test]
fn test_f19_links_to_outside_should_fail_the_run_links_inside_should_be_recreated_and_fifos_skipped() {
    let f = fx();
    let outside = f.base.parent().unwrap().join("outside.txt");
    std::fs::write(&outside, "precious\n").unwrap();
    for d in ["abs", "rel", "inside", "fifo"] {
        std::fs::create_dir_all(f.base.join(d)).unwrap();
    }
    symlink(&outside, f.base.join("abs/link-abs")).unwrap();
    // ../../outside.txt from `rel/` resolves to evals/../outside.txt: outside the task directory.
    symlink("../../outside.txt", f.base.join("rel/link-rel")).unwrap();
    std::fs::create_dir_all(f.base.join("inside/sub")).unwrap();
    std::fs::write(f.base.join("inside/real.txt"), "real\n").unwrap();
    symlink("real.txt", f.base.join("inside/alias")).unwrap();
    symlink("sub", f.base.join("inside/dirlink")).unwrap();
    assert!(std::process::Command::new("mkfifo").arg(f.base.join("fifo/pipe")).status().unwrap().success());
    let raw = ["abs", "rel", "inside", "fifo"]
        .iter()
        .map(|d| task_toml(d, d, "true"))
        .collect::<String>();
    let built = Arc::new(Mutex::new(Vec::new()));
    let r = runner(&f, &raw, quiet_builder(built.clone()));

    let o = hone_eval::EvalOptions { keep_workdirs: true, ..opts(&f) };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _ = tx.send(rt.block_on(r.run(&o)));
    });
    let run = rx
        .recv_timeout(std::time::Duration::from_secs(30))
        .expect("a FIFO must not hang the copy")
        .unwrap();

    for (task, link) in [("abs", "link-abs"), ("rel", "link-rel")] {
        let o = outcome(&run, task);
        assert_eq!(o["status"], "infra_error", "{task}: {o}");
        assert!(o["message"].as_str().unwrap().contains(link), "the message names the link: {o}");
    }
    let built = built.lock().unwrap().clone();
    assert!(!built.contains(&"abs".to_owned()) && !built.contains(&"rel".to_owned()), "no agent ran: {built:?}");
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "precious\n");

    assert_eq!(outcome(&run, "inside")["status"], "pass");
    let copy = run_dir_of(&run, "default", "inside", 1).join("workdir");
    assert_eq!(std::fs::read_link(copy.join("alias")).unwrap(), std::path::Path::new("real.txt"));
    assert_eq!(std::fs::read_link(copy.join("dirlink")).unwrap(), std::path::Path::new("sub"));

    assert_eq!(outcome(&run, "fifo")["status"], "pass");
    let copy = run_dir_of(&run, "default", "fifo", 1).join("workdir");
    assert!(!copy.join("pipe").exists(), "the FIFO is not copied");
    let check = std::fs::read_to_string(run_dir_of(&run, "default", "fifo", 1).join("check.txt")).unwrap();
    assert!(check.contains("pipe") && check.contains("skipped"), "{check}");
}

// ---- F21: cheaper, bounded copies --------------------------------------------------------------------------

#[tokio::test]
async fn test_f21_the_copy_should_respect_gitignore_keep_git_and_skip_target_only_at_the_top() {
    let f = fx();
    let p = f.base.join("proj");
    for d in ["build", ".git", "src/target", "src/node_modules", "target", "node_modules"] {
        std::fs::create_dir_all(p.join(d)).unwrap();
    }
    std::fs::write(p.join(".gitignore"), "*.log\nbuild/\n").unwrap();
    for (rel, text) in [
        ("keep.txt", "k"), ("debug.log", "l"), ("build/out.o", "o"), (".git/HEAD", "ref: refs/heads/main\n"),
        ("src/target/gen.rs", "g"), ("src/node_modules/m.js", "m"), ("target/junk", "j"), ("node_modules/junk", "j"),
    ] {
        std::fs::write(p.join(rel), text).unwrap();
    }
    let r = runner(&f, &task_toml("t", "proj", "true"), quiet_builder(Arc::default()));

    let run = r.run(&hone_eval::EvalOptions { keep_workdirs: true, ..opts(&f) }).await.unwrap();

    let w = run_dir_of(&run, "default", "t", 1).join("workdir");
    for kept in ["keep.txt", ".gitignore", ".git/HEAD", "src/target/gen.rs", "src/node_modules/m.js"] {
        assert!(w.join(kept).exists(), "{kept} is copied");
    }
    for skipped in ["debug.log", "build", "target", "node_modules"] {
        assert!(!w.join(skipped).exists(), "{skipped} is not copied");
    }
}

// ---- F22: small correctness fixes ---------------------------------------------------------------------------

#[test]
fn test_f22_zero_repeats_should_be_an_error() {
    let tf = TasksFile::from_toml_str("tasks.toml", BASIC).unwrap();
    let e = tf.expand(&ExpandOptions { profiles: vec![], repeats: Some(0) }).err().expect("--repeats 0").to_string();
    assert!(e.contains("repeats"), "{e}");
    let e = TasksFile::from_toml_str("tasks.toml", &format!("{BASIC}\n[eval]\nrepeats = 0\n")).err().expect("[eval] repeats = 0").to_string();
    assert!(e.contains("repeats"), "{e}");
}

#[test]
fn test_f22_names_that_cannot_be_path_components_should_be_rejected() {
    let task = |id: &str| format!("[[tasks]]\nid = {id:?}\nprompt = \"p\"\nsuccess_check = \"true\"\n");
    for bad in ["../evil", "a/b", "x\u{7}y", "..", "", "a\\b"] {
        let e = TasksFile::from_toml_str("tasks.toml", &task(bad)).err().unwrap_or_else(|| panic!("task id {bad:?} must be rejected")).to_string();
        assert!(e.contains("tasks.toml"), "{bad:?}: {e}");
    }
    for bad in ["../x", "a/b", "p\u{1b}q", ".."] {
        let raw = format!("{}[profiles.{bad:?}.context]\ncompactor = \"trim-only\"\n", task("ok"));
        assert!(TasksFile::from_toml_str("tasks.toml", &raw).is_err(), "profile name {bad:?} must be rejected");
    }
    assert!(TasksFile::from_toml_str("tasks.toml", &task("fine-name_1.2")).is_ok());
}

fn panicking_builder() -> AgentBuilder {
    Arc::new(|b: AgentBuild| {
        if b.task.id == "boom" {
            panic!("builder exploded");
        }
        Ok(agent_for(&b, vec![]))
    })
}

fn two_tasks() -> String {
    format!("{}{}", task_toml("boom", "proj", "true"), task_toml("fine", "proj", "true"))
}

#[tokio::test]
async fn test_f22_a_panicking_run_should_be_recorded_as_agent_error_with_parallel_jobs() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    let r = runner(&f, &two_tasks(), panicking_builder());

    let run = r.run(&hone_eval::EvalOptions { jobs: 2, ..opts(&f) }).await.unwrap();

    let o = outcome(&run, "boom");
    assert_eq!(o["status"], "agent_error", "{o}");
    assert!(o["message"].as_str().is_some_and(|m| m.contains("builder exploded") || m.contains("panic")), "{o}");
    assert_eq!(outcome(&run, "fine")["status"], "pass");
    let rep = read_json(&run.join("report.json"));
    assert_eq!(rep["profiles"][0]["runs"], 2, "the panicked run is counted: {rep}");
}

#[tokio::test]
async fn test_f22_a_panicking_run_should_not_take_down_a_sequential_eval() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    let r = runner(&f, &two_tasks(), panicking_builder());
    let opts = opts(&f);

    let result = tokio::spawn(async move { r.run(&opts).await }).await;

    let run = result.expect("the runner itself must not panic").unwrap();
    assert_eq!(outcome(&run, "boom")["status"], "agent_error");
    assert_eq!(outcome(&run, "fine")["status"], "pass");
}

#[tokio::test]
async fn test_f22_a_timeout_should_report_the_tokens_the_session_used() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    let raw = task_toml("t", "proj", "true").replace("timeout_secs = 30", "timeout_secs = 2");
    // Round 1: a model call (100 in / 20 out) asking for the slow tool, which outlives the timeout.
    let b: AgentBuilder = Arc::new(|b: AgentBuild| Ok(agent_for(&b, vec![vec![call("c1", "slow", json!({}))]])));

    let run = runner(&f, &raw, b).run(&opts(&f)).await.unwrap();

    let o = outcome(&run, "t");
    assert_eq!(o["status"], "timeout", "{o}");
    assert_eq!(o["tokens_input"], 100, "{o}");
    assert_eq!(o["tokens_output"], 20, "{o}");
    assert!(o["cost_usd"].as_f64().unwrap() > 0.0);
}

// ---- F23: the manifest says what ran (suite and build SHAs) -------------------------------------------------------

#[tokio::test]
async fn test_f23_manifest_should_record_the_suite_git_sha_separately_from_hones() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    git(&f.base, &["init", "-q", "-b", "trunk"]);
    std::fs::write(f.base.join("tasks.toml"), "x").unwrap();
    git(&f.base, &["add", "."]);
    git(&f.base, &["commit", "-q", "-m", "suite"]);
    let suite_sha = git(&f.base, &["rev-parse", "HEAD"]);
    let r = runner(&f, &task_toml("t", "proj", "true"), quiet_builder(Arc::default()));

    let run = r.run(&opts(&f)).await.unwrap();

    let m = read_json(&run.join("manifest.json"));
    assert_eq!(m["suite_git_sha"], suite_sha.as_str());
    assert_eq!(m["hone_version"], env!("CARGO_PKG_VERSION"));
    let hone_sha = &m["hone_git_sha"];
    assert!(
        hone_sha.is_null() || hone_sha.as_str().is_some_and(|s| s.len() >= 7 && s.chars().all(|c| c.is_ascii_hexdigit())),
        "hone_git_sha is build metadata (a hex SHA) or null: {hone_sha}"
    );
    assert_ne!(m["hone_git_sha"], m["suite_git_sha"], "different things: the test suite's repository is not hone's build");
}

#[tokio::test]
async fn test_f23_a_suite_outside_git_should_have_a_null_suite_sha() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    let r = runner(&f, &task_toml("t", "proj", "true"), quiet_builder(Arc::default()));

    let run = r.run(&opts(&f)).await.unwrap();

    let m = read_json(&run.join("manifest.json"));
    assert!(m.as_object().unwrap().contains_key("suite_git_sha"));
    assert!(m["suite_git_sha"].is_null(), "{m}");
}
