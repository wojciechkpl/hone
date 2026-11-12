#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F1, F2, F4 (jobs), F7, F8, F10 of `docs/plans/eval-profiles-spec.md` through
//! `hone_eval::EvalRunner` with a scripted mock provider and the real file tools.
//! No model is called. These tests live in `bins/hone/tests` because they need
//! `async_trait`/`futures` (not dev-dependencies of `hone-eval`) and `hone-config`.
//!
//! API required (new, `hone_eval::runner` re-exported at the crate root):
//!
//! ```ignore
//! /// What the runner hands to the agent builder for one run. The real CLI builder
//! /// (the one-shot `hone` builder) is plugged in by `cmd_eval`; tests plug in a scripted one.
//! pub struct AgentBuild {
//!     pub task: TaskSpec, pub profile: String, pub repeat: u32,
//!     pub workdir: PathBuf,                       // the fresh copy of the task dir
//!     pub memory: Arc<dyn MemoryStore>,           // fresh, loaded from the fixture
//!     pub model: String,                          // --model, else file `model`, else default
//!     pub selection: StrategySelection,           // the profile, resolved (hone_config)
//! }
//! pub type AgentBuilder = Arc<dyn Fn(AgentBuild) -> Result<hone_core::Agent, String> + Send + Sync>;
//! #[derive(Default)]
//! pub struct EvalOptions {
//!     pub out_dir: PathBuf, pub profiles: Vec<String>, pub repeats: Option<u32>,
//!     pub jobs: usize /* 0 or 1 = sequential */, pub keep_workdirs: bool,
//!     pub model: Option<String>, pub tag: Option<String>,
//!     pub history: Option<Arc<EvalRunStore>>,
//! }
//! impl EvalRunner {
//!     pub fn with_builder(tasks: TasksFile, base_dir: PathBuf,
//!                         default_model: impl Into<String>, builder: AgentBuilder) -> Self;
//!     /// Runs every planned run and returns `<out_dir>/<run-id>`.
//!     pub async fn run(&self, opts: &EvalOptions) -> Result<PathBuf, EvalRunnerError>;
//! }
//! ```
//!
//! Files per run, `<run>/<profile>/<task>/r<k>/`: `outcome.json` (keys `status` in
//! snake_case: `pass|fail|timeout|agent_error|infra_error`, `duration_ms`, `tokens_input`,
//! `tokens_output`, `tokens_cache` = cache_read + cache_creation, `cost_usd`,
//! `model_calls`, `context_tokens`, `check_exit_code`), `receipts.json` (array of
//! `ContextReceipt`), `transcript.json` (redacted messages), `check.txt` (contains
//! `exit code: <n>`), and `workdir/` only with `keep_workdirs`.
//! `<run>/manifest.json`: `hone_version`, `git_sha`, `model`, `profiles`
//! (`[{ "name", "strategies": [{ "slot", "name", "params_sha256" }] }]`), `tasks_sha256`,
//! `repeats`, `started_at`, `ended_at`. The run id is `<%Y%m%dT%H%M%SZ>-<first 8 hex of
//! tasks_sha256>`. `<run>/report.json`: `{ "profiles": [{ "profile", "runs", "passes",
//! "pass_rate", "wilson_low", "wilson_high", "mean_cost_usd", "median_cost_usd",
//! "mean_context_tokens", "mean_model_calls", "timeouts", "errors", "pareto_cost",
//! "pareto_context", "vs_base" }] }` (`vs_base` null for the base profile, else
//! `{ "tasks", "mean_diff", "better", "worse", "tied", "ci_low", "ci_high" }`) and `report.md`.
//! History (F10): with `opts.history`, the finished run is recorded under `opts.tag`;
//! the record's `outcomes` are the base profile's, repeat 1, one per task with `id` =
//! task id, and it serialises a `profiles` array of `{ "profile", "runs", "passes" }`.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{Agent, AgentConfig, AgentMode, RetryConfig};
use hone_eval::tasks::TasksFile;
use hone_eval::{AgentBuild, AgentBuilder, EvalOptions, EvalRunStore, EvalRunner, RegressionBisector};
use hone_memory::{Memory, MemoryKind};
use hone_proto::{
    Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth, TokenUsage,
    ToolApprovalPolicy, ToolCallRequest,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{ReadFileTool, ToolRegistry, WriteFileTool};
use serde_json::{Value, json};

const SECRET: &str = "ghp_0123456789abcdefghijklmnopqrstuvwxyz";

// ---- scripted provider ------------------------------------------------------------------

struct Script {
    rounds: Mutex<Vec<Vec<ToolCallRequest>>>,
    n: AtomicUsize,
    hang: bool,
}

#[async_trait]
impl ModelProvider for Script {
    fn id(&self) -> &str {
        "script"
    }
    fn display_name(&self) -> &str {
        "Script"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "t".to_owned(),
            context_length: 128_000,
            supports_tools: true,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        if self.hang {
            tokio::time::sleep(Duration::from_secs(120)).await;
        }
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        let round = self.rounds.lock().unwrap().get(i).cloned();
        let mut events: Vec<ProviderEvent> = match round {
            Some(calls) => calls.into_iter().map(ProviderEvent::ToolCall).collect(),
            None => vec![ProviderEvent::Token {
                text: "done".to_owned(),
            }],
        };
        events.push(ProviderEvent::Usage(TokenUsage {
            input: 100,
            output: 20,
            cache_read: 7,
            cache_creation: 3,
        }));
        events.push(ProviderEvent::Done);
        Ok(Box::pin(stream::iter(events)))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn call(id: &str, name: &str, args: Value) -> ToolCallRequest {
    ToolCallRequest {
        id: id.to_owned(),
        name: name.to_owned(),
        arguments: args,
    }
}

/// An agent with the real read_file / write_file tools, working in `b.workdir` with
/// `b.memory`, whose model replays `rounds`.
fn agent_for(b: &AgentBuild, rounds: Vec<Vec<ToolCallRequest>>, hang: bool) -> Agent {
    let tools = ToolRegistry::builder()
        .register(Box::new(ReadFileTool))
        .register(Box::new(WriteFileTool))
        .build();
    Agent::new(AgentConfig {
        provider: Arc::new(Script {
            rounds: Mutex::new(rounds),
            n: AtomicUsize::new(0),
            hang,
        }),
        tools,
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "eval".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 1.0,
            output_per_million: 2.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: b.workdir.clone(),
        memory_store: Some(b.memory.clone()),
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    })
}

/// read data.txt, overwrite it with "changed", answer.
fn rewrite_rounds() -> Vec<Vec<ToolCallRequest>> {
    vec![
        vec![call("c1", "read_file", json!({ "path": "data.txt" }))],
        vec![call("c2", "write_file", json!({ "path": "data.txt", "content": "changed\n" }))],
    ]
}

fn builder(rounds: impl Fn(&AgentBuild) -> Vec<Vec<ToolCallRequest>> + Send + Sync + 'static) -> AgentBuilder {
    Arc::new(move |b: AgentBuild| Ok(agent_for(&b, rounds(&b), false)))
}

// ---- fixtures ---------------------------------------------------------------------------------

struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf, // the tasks file's directory
    out: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (base, out) = (root.join("evals"), root.join("out"));
    std::fs::create_dir_all(base.join("proj/target")).unwrap();
    std::fs::create_dir_all(base.join("proj/node_modules/x")).unwrap();
    std::fs::create_dir_all(base.join("proj/.git")).unwrap();
    std::fs::write(base.join("proj/data.txt"), format!("orig {SECRET}\n")).unwrap();
    std::fs::write(base.join("proj/target/junk"), "junk").unwrap();
    std::fs::write(base.join("proj/node_modules/x/i.js"), "js").unwrap();
    std::fs::write(base.join("proj/.git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(&out).unwrap();
    Fx { _tmp: tmp, base, out }
}

const TASKS: &str = r#"
model = "ollama:from-file"

[[tasks]]
id = "rewrite"
prompt = "rewrite data.txt"
success_check = "grep -q changed data.txt"
working_dir = "proj"
timeout_secs = 30
"#;

fn runner(f: &Fx, raw: &str, b: AgentBuilder) -> EvalRunner {
    let tasks = TasksFile::from_toml_str("tasks.toml", raw).unwrap_or_else(|e| panic!("{e}"));
    EvalRunner::with_builder(tasks, f.base.clone(), "mock:default", b)
}

fn opts(f: &Fx) -> EvalOptions {
    EvalOptions {
        out_dir: f.out.clone(),
        ..Default::default()
    }
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
        .unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn run_dir_of(run: &Path, profile: &str, task: &str, k: u32) -> PathBuf {
    run.join(profile).join(task).join(format!("r{k}"))
}

// ---- F1: a real agent -----------------------------------------------------------------------

#[tokio::test]
async fn test_f1_the_agent_should_use_real_tools_in_the_copy_and_the_check_should_pass() {
    let f = fx();
    let r = runner(&f, TASKS, builder(|_| rewrite_rounds()));

    let run = r.run(&opts(&f)).await.unwrap();

    let d = run_dir_of(&run, "default", "rewrite", 1);
    let outcome = read_json(&d.join("outcome.json"));
    assert_eq!(outcome["status"], "pass", "{outcome}");
    assert_eq!(outcome["check_exit_code"], 0);
}

#[tokio::test]
async fn test_f1_the_model_should_be_the_flag_then_the_file_then_the_default() {
    let f = fx();
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let s = seen.clone();
    let b: AgentBuilder = Arc::new(move |b: AgentBuild| {
        s.lock().unwrap().push(b.model.clone());
        Ok(agent_for(&b, vec![], false))
    });
    let with_file = runner(&f, TASKS, b.clone());
    let no_file = runner(&f, &TASKS.replace("model = \"ollama:from-file\"\n", ""), b);

    with_file.run(&EvalOptions { model: Some("ollama:flag".to_owned()), ..opts(&f) }).await.unwrap();
    with_file.run(&opts(&f)).await.unwrap();
    no_file.run(&opts(&f)).await.unwrap();

    assert_eq!(*seen.lock().unwrap(), ["ollama:flag", "ollama:from-file", "mock:default"]);
}

#[tokio::test]
async fn test_f1_each_run_should_get_its_profiles_resolved_selection() {
    let f = fx();
    let seen = Arc::new(Mutex::new(Vec::<(String, String)>::new()));
    let s = seen.clone();
    let b: AgentBuilder = Arc::new(move |b: AgentBuild| {
        s.lock().unwrap().push((b.profile.clone(), b.selection.compactor.value.clone()));
        Ok(agent_for(&b, vec![], false))
    });
    let raw = format!("{TASKS}\n[profiles.lean.context]\ncompactor = \"trim-only\"\n");

    runner(&f, &raw, b).run(&opts(&f)).await.unwrap();

    let mut got = seen.lock().unwrap().clone();
    got.sort();
    assert_eq!(
        got,
        [("default".to_owned(), "graduated".to_owned()), ("lean".to_owned(), "trim-only".to_owned())]
    );
}

// ---- F2: isolation ----------------------------------------------------------------------------

#[tokio::test]
async fn test_f2_repeats_should_each_see_the_original_files_and_leave_the_source_untouched() {
    let f = fx();
    let r = runner(&f, TASKS, builder(|_| rewrite_rounds()));

    let run = r.run(&EvalOptions { repeats: Some(2), ..opts(&f) }).await.unwrap();

    for k in 1..=2 {
        let t = std::fs::read_to_string(run_dir_of(&run, "default", "rewrite", k).join("transcript.json")).unwrap();
        assert!(t.contains("orig"), "r{k} must read the original file: {t}");
    }
    assert_eq!(
        std::fs::read_to_string(f.base.join("proj/data.txt")).unwrap(),
        format!("orig {SECRET}\n"),
        "the source directory is untouched"
    );
}

#[tokio::test]
async fn test_f2_the_copy_should_be_deleted_unless_keep_workdirs_and_skip_target_and_node_modules() {
    let f = fx();
    let r = runner(&f, TASKS, builder(|_| rewrite_rounds()));

    let run = r.run(&opts(&f)).await.unwrap();
    assert!(!run_dir_of(&run, "default", "rewrite", 1).join("workdir").exists(), "deleted by default");

    let kept = r.run(&EvalOptions { keep_workdirs: true, ..opts(&f) }).await.unwrap();
    let w = run_dir_of(&kept, "default", "rewrite", 1).join("workdir");
    assert_eq!(std::fs::read_to_string(w.join("data.txt")).unwrap(), "changed\n");
    assert!(w.join(".git/HEAD").exists(), ".git is copied");
    assert!(!w.join("target").exists(), "target/ is skipped");
    assert!(!w.join("node_modules").exists(), "node_modules/ is skipped");
}

fn fixture_line(id: &str, content: &str) -> String {
    serde_json::to_string(&Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000_000,
        decay_at: None,
        kind: MemoryKind::Feedback,
        description: None,
        embedding: None,
    })
    .unwrap()
}

#[tokio::test]
async fn test_f2_a_memory_fixture_should_be_loaded_into_a_fresh_store_for_each_run() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("fixtures")).unwrap();
    std::fs::write(
        f.base.join("fixtures/mem.jsonl"),
        format!("{}\n{}\n", fixture_line("fx-1", "zorblax branches are rebased before merge"), fixture_line("fx-2", "unrelated lunch menu")),
    )
    .unwrap();
    let raw = r#"
[[tasks]]
id = "with-fixture"
prompt = "how do we treat zorblax branches"
success_check = "true"
working_dir = "proj"
memory_fixture = "fixtures/mem.jsonl"

[[tasks]]
id = "without"
prompt = "how do we treat zorblax branches"
success_check = "true"
working_dir = "proj"
"#;
    let r = runner(&f, raw, builder(|_| vec![]));

    let run = r.run(&EvalOptions { repeats: Some(2), ..opts(&f) }).await.unwrap();

    for k in 1..=2 {
        let rc = std::fs::read_to_string(run_dir_of(&run, "default", "with-fixture", k).join("receipts.json")).unwrap();
        assert!(rc.contains("fx-1"), "r{k}: the recall receipt names the fixture memory: {rc}");
        assert!(!rc.contains("fx-2"));
    }
    let none = std::fs::read_to_string(run_dir_of(&run, "default", "without", 1).join("receipts.json")).unwrap();
    assert!(!none.contains("fx-1"), "no state leaks between runs: {none}");
}

#[tokio::test]
async fn test_f8_a_missing_fixture_should_be_an_infra_error_and_not_stop_other_runs() {
    let f = fx();
    let raw = r#"
[[tasks]]
id = "broken"
prompt = "p"
success_check = "true"
working_dir = "proj"
memory_fixture = "fixtures/missing.jsonl"

[[tasks]]
id = "fine"
prompt = "p"
success_check = "true"
working_dir = "proj"
"#;
    let run = runner(&f, raw, builder(|_| vec![])).run(&opts(&f)).await.unwrap();

    assert_eq!(read_json(&run_dir_of(&run, "default", "broken", 1).join("outcome.json"))["status"], "infra_error");
    assert_eq!(read_json(&run_dir_of(&run, "default", "fine", 1).join("outcome.json"))["status"], "pass");
}

// ---- F8: outcome ---------------------------------------------------------------------------------

#[tokio::test]
async fn test_f8_outcome_should_carry_usage_cost_calls_and_context_tokens_from_the_receipts() {
    let f = fx();
    let r = runner(&f, TASKS, builder(|_| rewrite_rounds()));

    let run = r.run(&opts(&f)).await.unwrap();

    let d = run_dir_of(&run, "default", "rewrite", 1);
    let o = read_json(&d.join("outcome.json"));
    let receipts = read_json(&d.join("receipts.json"));
    let receipts = receipts.as_array().unwrap();
    assert_eq!(receipts.len(), 3, "read, write, answer");
    assert_eq!(o["model_calls"], 3);
    let sum: u64 = receipts
        .iter()
        .flat_map(|r| r["blocks"].as_array().unwrap())
        .map(|b| b["tokens"].as_u64().unwrap())
        .sum();
    assert!(sum > 0);
    assert_eq!(o["context_tokens"], sum);
    assert_eq!(o["tokens_input"], 300);
    assert_eq!(o["tokens_output"], 60);
    assert_eq!(o["tokens_cache"], 30);
    let cost = o["cost_usd"].as_f64().unwrap();
    assert!((cost - 3.0 * (100.0 * 1.0 + 20.0 * 2.0) / 1e6).abs() < 1e-9, "{cost}");
    assert!(o["duration_ms"].is_u64());
}

#[tokio::test]
async fn test_f8_a_failing_check_should_be_a_fail_with_its_exit_code_and_output_in_check_txt() {
    let f = fx();
    let raw = TASKS.replace("grep -q changed data.txt", "echo nope; exit 3");
    let run = runner(&f, &raw, builder(|_| vec![])).run(&opts(&f)).await.unwrap();

    let d = run_dir_of(&run, "default", "rewrite", 1);
    let o = read_json(&d.join("outcome.json"));
    assert_eq!(o["status"], "fail");
    assert_eq!(o["check_exit_code"], 3);
    let check = std::fs::read_to_string(d.join("check.txt")).unwrap();
    assert!(check.contains("nope") && check.contains("exit code: 3"), "{check}");
}

#[tokio::test]
async fn test_f8_a_run_past_its_timeout_should_be_recorded_as_a_timeout() {
    let f = fx();
    let raw = TASKS.replace("timeout_secs = 30", "timeout_secs = 1");
    let b: AgentBuilder = Arc::new(|b: AgentBuild| Ok(agent_for(&b, vec![], true)));

    let run = runner(&f, &raw, b).run(&opts(&f)).await.unwrap();

    let o = read_json(&run_dir_of(&run, "default", "rewrite", 1).join("outcome.json"));
    assert_eq!(o["status"], "timeout", "{o}");
    assert!(o["duration_ms"].as_u64().unwrap() < 20_000);
}

// ---- F7: run directories --------------------------------------------------------------------

fn two_profiles() -> String {
    format!("{TASKS}\n[profiles.lean.context]\ncompactor = \"trim-only\"\n")
}

#[tokio::test]
async fn test_f7_run_dir_should_hold_the_manifest_and_one_directory_per_profile_task_and_repeat() {
    let f = fx();
    let raw = two_profiles();
    let run = runner(&f, &raw, builder(|_| rewrite_rounds())).run(&EvalOptions { repeats: Some(2), ..opts(&f) }).await.unwrap();

    assert_eq!(run.parent().unwrap(), f.out, "the run dir is <out>/<run-id>");
    let id = run.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(regex::Regex::new(r"^\d{8}T\d{6}Z-[0-9a-f]{8}$").unwrap().is_match(&id), "{id}");
    for profile in ["default", "lean"] {
        for k in 1..=2 {
            let d = run_dir_of(&run, profile, "rewrite", k);
            for file in ["outcome.json", "receipts.json", "transcript.json", "check.txt"] {
                assert!(d.join(file).is_file(), "{}", d.join(file).display());
            }
        }
    }
    assert!(run.join("report.json").is_file() && run.join("report.md").is_file());
}

#[tokio::test]
async fn test_f7_manifest_should_record_version_model_profiles_with_strategy_ids_and_the_file_hash() {
    let f = fx();
    let raw = two_profiles();
    let run = runner(&f, &raw, builder(|_| vec![])).run(&EvalOptions { repeats: Some(2), ..opts(&f) }).await.unwrap();

    let m = read_json(&run.join("manifest.json"));
    assert_eq!(m["hone_version"], env!("CARGO_PKG_VERSION"));
    assert!(m["git_sha"].is_string() || m["git_sha"].is_null());
    assert_eq!(m["model"], "ollama:from-file");
    assert_eq!(m["repeats"], 2);
    assert!(m["started_at"].is_string() && m["ended_at"].is_string());
    let hash = m["tasks_sha256"].as_str().unwrap();
    assert_eq!(hash.len(), 64);
    assert!(run.file_name().unwrap().to_str().unwrap().ends_with(&hash[..8]), "run id carries the hash prefix");
    let profiles = m["profiles"].as_array().unwrap();
    let names: Vec<&str> = profiles.iter().map(|p| p["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["default", "lean"]);
    let compactor = |p: &Value| {
        p["strategies"].as_array().unwrap().iter().find(|s| s["slot"] == "compactor").map(|s| s["name"].clone())
    };
    assert_eq!(compactor(&profiles[0]), Some(json!("graduated")));
    assert_eq!(compactor(&profiles[1]), Some(json!("trim-only")));
    for s in profiles[1]["strategies"].as_array().unwrap() {
        assert_eq!(s["params_sha256"].as_str().unwrap().len(), 64);
    }
}

#[tokio::test]
async fn test_f7_the_transcript_should_be_redacted() {
    let f = fx();
    let run = runner(&f, TASKS, builder(|_| rewrite_rounds())).run(&opts(&f)).await.unwrap();

    let t = std::fs::read_to_string(run_dir_of(&run, "default", "rewrite", 1).join("transcript.json")).unwrap();
    assert!(t.contains("orig"), "the read result is in the transcript");
    assert!(!t.contains(SECRET), "secret-shaped values are redacted: {t}");
}

// ---- F4: jobs, F9: report content ---------------------------------------------------------------

#[tokio::test]
async fn test_f4_parallel_jobs_should_produce_the_same_runs() {
    let f = fx();
    let raw = format!("{}\n[[tasks]]\nid = \"second\"\nprompt = \"p\"\nsuccess_check = \"true\"\nworking_dir = \"proj\"\n", two_profiles());

    let run = runner(&f, &raw, builder(|_| vec![])).run(&EvalOptions { jobs: 3, ..opts(&f) }).await.unwrap();

    for profile in ["default", "lean"] {
        for task in ["rewrite", "second"] {
            assert!(run_dir_of(&run, profile, task, 1).join("outcome.json").is_file(), "{profile}/{task}");
        }
    }
}

#[tokio::test]
async fn test_f9_report_should_summarise_each_profile_and_compare_the_others_with_the_base() {
    let f = fx();
    // `lean` never writes the file, so its check fails; `default` passes.
    let b: AgentBuilder = Arc::new(|b: AgentBuild| {
        let rounds = if b.profile == "default" { rewrite_rounds() } else { vec![] };
        Ok(agent_for(&b, rounds, false))
    });
    let run = runner(&f, &two_profiles(), b).run(&EvalOptions { repeats: Some(2), ..opts(&f) }).await.unwrap();

    let rep = read_json(&run.join("report.json"));
    let profiles = rep["profiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 2);
    let base = &profiles[0];
    assert_eq!(base["profile"], "default");
    assert_eq!((base["runs"].clone(), base["passes"].clone()), (json!(2), json!(2)));
    assert_eq!(base["pass_rate"], 1.0);
    assert!(base["wilson_low"].as_f64().unwrap() < 1.0 && base["wilson_high"].as_f64().unwrap() > 0.99);
    assert!(base["vs_base"].is_null());
    let lean = &profiles[1];
    assert_eq!(lean["passes"], 0);
    assert_eq!(lean["pass_rate"], 0.0);
    assert_eq!(lean["vs_base"]["tasks"], 1);
    assert_eq!(lean["vs_base"]["worse"], 1);
    assert_eq!(lean["vs_base"]["mean_diff"], -1.0);
    for key in ["mean_cost_usd", "median_cost_usd", "mean_context_tokens", "mean_model_calls", "timeouts", "errors", "pareto_cost", "pareto_context"] {
        assert!(!lean[key].is_null(), "{key}");
    }
    let md = std::fs::read_to_string(run.join("report.md")).unwrap();
    assert!(md.contains("default") && md.contains("lean"));
}

// ---- F10: history -----------------------------------------------------------------------------------

#[tokio::test]
async fn test_f10_a_finished_run_should_be_recorded_with_profile_summaries_and_stay_comparable() {
    let f = fx();
    let store = Arc::new(EvalRunStore::open_in_memory().await.unwrap());
    let raw = two_profiles();
    let good = runner(&f, &raw, builder(|_| rewrite_rounds()));
    let bad = runner(&f, &raw, builder(|_| vec![])); // never writes the file: the check fails

    good.run(&EvalOptions { tag: Some("before".to_owned()), history: Some(store.clone()), ..opts(&f) }).await.unwrap();
    bad.run(&EvalOptions { tag: Some("after".to_owned()), history: Some(store.clone()), ..opts(&f) }).await.unwrap();

    let before = store.latest_with_tag("before").await.unwrap().expect("recorded");
    let after = store.latest_with_tag("after").await.unwrap().expect("recorded");
    assert_eq!(before.outcomes.len(), 1, "the base profile's outcomes, one per task");
    assert_eq!(before.outcomes[0].id, "rewrite");
    let profiles = serde_json::to_value(&before).unwrap()["profiles"].clone();
    let names: Vec<&str> = profiles.as_array().unwrap().iter().map(|p| p["profile"].as_str().unwrap()).collect();
    assert_eq!(names, ["default", "lean"]);
    assert_eq!(profiles[0]["passes"], 1);
    let report = RegressionBisector::compare(&before, &after);
    assert_eq!(report.regressed, ["rewrite"]);
}
