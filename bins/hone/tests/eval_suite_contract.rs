#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! F11 of `docs/plans/eval-profiles-spec.md`, reusing the seams pinned by `eval_runs.rs`
//! (`EvalRunner::with_builder`, `AgentBuild`, `EvalOptions`, `run`):
//!
//! - the success check runs with the run's working-directory copy as its current
//!   directory and `EVAL_SUITE_DIR` set to the ABSOLUTE path of the tasks file's directory
//!   (the `base_dir` the runner was built with);
//! - `working_dir` and `memory_fixture` are relative to the tasks file, not to the process's
//!   current directory;
//! - a `memory_fixture` is JSONL, one `hone_memory::Memory` per line; `id`, `kind` and
//!   `content` are required, the other fields default (`tags` empty, `weight` 1.0,
//!   `created_at` 0 or now, `decay_at` and `description` none); blank lines are ignored;
//! - a malformed line (not JSON, or a missing required field) makes that run an
//!   `infra_error`; the run still produces its `outcome.json`, whose string field
//!   `message` names the fixture file and the line (`<file>` and `line <n>`).
//!
//! The excerpt in `tests/fixtures/starter_excerpt/` is a two-task copy of the starter suite
//! (`eval/suites/starter`, a sibling branch): a Python bug fix and a memory task, with
//! checks, references and a fixture. Tests that run its checks need `python3`.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{Agent, AgentConfig, AgentMode, RetryConfig};
use hone_eval::tasks::TasksFile;
use hone_eval::{AgentBuild, AgentBuilder, EvalOptions, EvalRunner};
use hone_memory::{MemoryQuery, MemoryStore};
use hone_proto::{
    Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth, ToolApprovalPolicy,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use serde_json::Value;

// ---- a model that just answers --------------------------------------------------------------

struct Answer;

#[async_trait]
impl ModelProvider for Answer {
    fn id(&self) -> &str {
        "answer"
    }
    fn display_name(&self) -> &str {
        "Answer"
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
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::Token { text: "done".to_owned() },
            ProviderEvent::Done,
        ])))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn agent_for(b: &AgentBuild) -> Agent {
    Agent::new(AgentConfig {
        provider: Arc::new(Answer),
        tools: ToolRegistry::new(),
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

// ---- helpers ------------------------------------------------------------------------------------

struct Fx {
    _tmp: tempfile::TempDir,
    base: PathBuf, // the tasks file's directory (absolute, canonical)
    out: PathBuf,
}

fn fx() -> Fx {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let (base, out) = (root.join("suite"), root.join("out"));
    std::fs::create_dir_all(&base).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    Fx { _tmp: tmp, base, out }
}

fn opts(f: &Fx) -> EvalOptions {
    EvalOptions { out_dir: f.out.clone(), ..Default::default() }
}

fn read_json(p: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))).unwrap()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

fn run_dir(run: &Path, task: &str) -> PathBuf {
    run.join("default").join(task).join("r1")
}

/// A builder that keeps every run's memory store and workdir, and does nothing else.
#[derive(Clone, Default)]
struct Seen {
    memories: Arc<Mutex<Vec<Arc<dyn MemoryStore>>>>,
    workdirs: Arc<Mutex<Vec<PathBuf>>>,
    calls: Arc<AtomicUsize>,
}

fn recording_builder(seen: &Seen) -> AgentBuilder {
    let seen = seen.clone();
    Arc::new(move |b: AgentBuild| {
        seen.calls.fetch_add(1, Ordering::SeqCst);
        seen.memories.lock().unwrap().push(b.memory.clone());
        seen.workdirs.lock().unwrap().push(b.workdir.clone());
        Ok(agent_for(&b))
    })
}

fn runner(base: &Path, raw: &str, b: AgentBuilder) -> EvalRunner {
    let tasks = TasksFile::from_toml_str("tasks.toml", raw).unwrap_or_else(|e| panic!("{e}"));
    EvalRunner::with_builder(tasks, base.to_path_buf(), "mock:default", b)
}

async fn all_memories(store: &Arc<dyn MemoryStore>) -> Vec<hone_memory::Memory> {
    let mut v = store
        .recall(&MemoryQuery { text: String::new(), limit: 100, kind: None })
        .await
        .unwrap();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()
}

// ---- the check's environment ------------------------------------------------------------------------

const ECHO_TASK: &str = r#"
[[tasks]]
id = "t"
prompt = "p"
success_check = 'echo "SUITE=$EVAL_SUITE_DIR"; echo "CWD=$(pwd -P)"; ls; test -f data.txt'
working_dir = "proj"
timeout_secs = 30
"#;

fn check_lines(run: &Path, task: &str) -> (String, String, String) {
    let text = std::fs::read_to_string(run_dir(run, task).join("check.txt")).unwrap();
    let find = |prefix: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(prefix))
            .unwrap_or_else(|| panic!("no {prefix} line in check.txt:\n{text}"))
            .to_owned()
    };
    (find("SUITE="), find("CWD="), text)
}

#[tokio::test]
async fn test_f11_the_check_should_see_the_tasks_directory_and_run_in_the_copy() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::write(f.base.join("proj/data.txt"), "x").unwrap();
    let seen = Seen::default();
    let r = runner(&f.base, ECHO_TASK, recording_builder(&seen));

    let run = r.run(&opts(&f)).await.unwrap();

    let (suite, cwd, text) = check_lines(&run, "t");
    assert_eq!(std::fs::canonicalize(&suite).unwrap(), f.base, "EVAL_SUITE_DIR is the tasks file's directory");
    assert!(Path::new(&suite).is_absolute(), "{suite}");
    let source = f.base.join("proj");
    assert_ne!(std::fs::canonicalize(&cwd).unwrap(), source, "the check runs in the copy, not the source");
    assert!(text.contains("data.txt"), "the copy has the task's files:\n{text}");
    let copy = seen.workdirs.lock().unwrap()[0].clone();
    assert_eq!(std::fs::canonicalize(&cwd).unwrap(), std::fs::canonicalize(&copy).unwrap(), "cwd is the run's workdir");
    assert_eq!(read_json(&run_dir(&run, "t").join("outcome.json"))["status"], "pass");
}

#[tokio::test]
async fn test_f11_eval_suite_dir_should_be_absolute_even_for_a_relative_base_dir() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::write(f.base.join("proj/data.txt"), "x").unwrap();
    // The same directory, spelled relative to this process's current directory.
    let cwd = std::env::current_dir().unwrap();
    let up = "../".repeat(cwd.components().count() - 1);
    let relative = PathBuf::from(format!("{up}{}", f.base.strip_prefix("/").unwrap().display()));
    assert!(relative.is_relative());
    let r = runner(&relative, ECHO_TASK, recording_builder(&Seen::default()));

    let run = r.run(&opts(&f)).await.unwrap();

    let (suite, _, _) = check_lines(&run, "t");
    assert!(Path::new(&suite).is_absolute(), "EVAL_SUITE_DIR must be absolute: {suite}");
    assert_eq!(std::fs::canonicalize(&suite).unwrap(), f.base);
}

#[tokio::test]
async fn test_f11_a_check_script_in_the_suite_directory_should_be_reachable_through_the_variable() {
    let f = fx();
    std::fs::create_dir_all(f.base.join("proj")).unwrap();
    std::fs::create_dir_all(f.base.join("checks")).unwrap();
    std::fs::write(f.base.join("proj/data.txt"), "x").unwrap();
    std::fs::write(f.base.join("checks/t.sh"), "test -f data.txt && echo from-script\n").unwrap();
    let raw = r#"
[[tasks]]
id = "t"
prompt = "p"
success_check = 'sh "$EVAL_SUITE_DIR/checks/t.sh"'
working_dir = "proj"
timeout_secs = 30
"#;

    let run = runner(&f.base, raw, recording_builder(&Seen::default())).run(&opts(&f)).await.unwrap();

    let text = std::fs::read_to_string(run_dir(&run, "t").join("check.txt")).unwrap();
    assert!(text.contains("from-script"), "{text}");
    assert_eq!(read_json(&run_dir(&run, "t").join("outcome.json"))["status"], "pass");
}

// ---- relative paths -----------------------------------------------------------------------------------------

const FIXTURE_OK: &str = concat!(
    r#"{"id":"m1","kind":"feedback","content":"alpha rule"}"#,
    "\n\n",
    r#"{"id":"m2","kind":"project","content":"beta fact","tags":["x"],"weight":2.0,"created_at":42}"#,
    "\n",
);

fn nested_suite(f: &Fx, fixture: &str) {
    std::fs::create_dir_all(f.base.join("tasks/one")).unwrap();
    std::fs::create_dir_all(f.base.join("fx")).unwrap();
    std::fs::write(f.base.join("tasks/one/data.txt"), "x").unwrap();
    std::fs::write(f.base.join("fx/m.jsonl"), fixture).unwrap();
}

const NESTED_TASK: &str = r#"
[[tasks]]
id = "t"
prompt = "p"
success_check = "test -f data.txt"
working_dir = "tasks/one"
memory_fixture = "fx/m.jsonl"
timeout_secs = 30
"#;

#[tokio::test]
async fn test_f11_working_dir_and_memory_fixture_should_resolve_against_the_tasks_file_not_the_process_cwd() {
    let f = fx();
    nested_suite(&f, FIXTURE_OK);
    let seen = Seen::default();

    let run = runner(&f.base, NESTED_TASK, recording_builder(&seen)).run(&opts(&f)).await.unwrap();

    assert_eq!(read_json(&run_dir(&run, "t").join("outcome.json"))["status"], "pass");
    let store = seen.memories.lock().unwrap()[0].clone();
    let ids: Vec<String> = all_memories(&store).await.into_iter().map(|m| m.id).collect();
    assert_eq!(ids, ["m1", "m2"], "the fixture next to the tasks file was loaded");
}

#[tokio::test]
async fn test_f11_fixture_lines_without_the_optional_fields_should_load_with_defaults() {
    let f = fx();
    nested_suite(&f, FIXTURE_OK);
    let seen = Seen::default();

    runner(&f.base, NESTED_TASK, recording_builder(&seen)).run(&opts(&f)).await.unwrap();

    let store = seen.memories.lock().unwrap()[0].clone();
    let got = all_memories(&store).await;
    let (m1, m2) = (&got[0], &got[1]);
    assert_eq!((m1.content.as_str(), m1.kind), ("alpha rule", hone_memory::MemoryKind::Feedback));
    assert!(m1.tags.is_empty() && m1.weight == 1.0, "defaults: {m1:?}");
    assert!(m1.decay_at.is_none() && m1.description.is_none());
    assert_eq!((m2.tags.clone(), m2.weight, m2.created_at), (vec!["x".to_owned()], 2.0, 42), "given fields are kept");
}

#[tokio::test]
async fn test_f11_runs_should_each_get_a_fresh_store_loaded_from_the_fixture() {
    let f = fx();
    nested_suite(&f, FIXTURE_OK);
    let seen = Seen::default();

    runner(&f.base, NESTED_TASK, recording_builder(&seen))
        .run(&EvalOptions { repeats: Some(2), ..opts(&f) })
        .await
        .unwrap();

    let stores = seen.memories.lock().unwrap().clone();
    assert_eq!(stores.len(), 2);
    for s in &stores {
        assert_eq!(all_memories(s).await.len(), 2);
    }
}

// ---- malformed fixtures ----------------------------------------------------------------------------------------

async fn bad_fixture_outcome(fixture: &str) -> (Value, Seen) {
    let f = fx();
    nested_suite(&f, fixture);
    let seen = Seen::default();
    let run = runner(&f.base, NESTED_TASK, recording_builder(&seen)).run(&opts(&f)).await.unwrap();
    (read_json(&run_dir(&run, "t").join("outcome.json")), seen)
}

#[tokio::test]
async fn test_f11_a_line_that_is_not_json_should_be_an_infra_error_naming_the_file_and_line() {
    let (outcome, _) = bad_fixture_outcome(&format!(
        "{}\n{{this is not json\n{}\n",
        r#"{"id":"m1","kind":"feedback","content":"ok"}"#,
        r#"{"id":"m3","kind":"user","content":"fine"}"#
    ))
    .await;

    assert_eq!(outcome["status"], "infra_error", "{outcome}");
    let msg = outcome["message"].as_str().unwrap_or_default();
    assert!(msg.contains("m.jsonl"), "names the file: {msg}");
    assert!(msg.contains("line 2"), "names the line: {msg}");
}

#[tokio::test]
async fn test_f11_a_line_missing_a_required_field_should_be_an_infra_error_naming_the_line() {
    for (fixture, line) in [
        (r#"{"id":"m1","content":"no kind"}"#, "line 1"),
        ("\n{\"id\":\"m1\",\"kind\":\"user\"}\n", "line 2"),
        ("{\"kind\":\"user\",\"content\":\"no id\"}\n", "line 1"),
        (r#"{"id":"m1","kind":"not-a-kind","content":"x"}"#, "line 1"),
    ] {
        let (outcome, _) = bad_fixture_outcome(fixture).await;
        assert_eq!(outcome["status"], "infra_error", "{fixture}: {outcome}");
        let msg = outcome["message"].as_str().unwrap_or_default();
        assert!(msg.contains("m.jsonl") && msg.contains(line), "{fixture}: {msg}");
    }
}

#[tokio::test]
async fn test_f11_a_malformed_fixture_should_not_run_the_agent_or_fail_the_whole_run() {
    let f = fx();
    nested_suite(&f, "{nope\n");
    let seen = Seen::default();
    let raw = format!(
        "{NESTED_TASK}\n[[tasks]]\nid = \"u\"\nprompt = \"p\"\nsuccess_check = \"true\"\nworking_dir = \"tasks/one\"\ntimeout_secs = 30\n"
    );

    let run = runner(&f.base, &raw, recording_builder(&seen)).run(&opts(&f)).await.expect("the run itself succeeds");

    assert_eq!(read_json(&run_dir(&run, "t").join("outcome.json"))["status"], "infra_error");
    assert_eq!(read_json(&run_dir(&run, "u").join("outcome.json"))["status"], "pass", "the other task still ran");
    assert_eq!(seen.calls.load(Ordering::SeqCst), 1, "no agent was built for the broken task");
}

// ---- the starter excerpt ---------------------------------------------------------------------------------------

fn excerpt() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/starter_excerpt")
}

/// An agent builder that "solves" the task by overlaying the reference solution
/// (when `solve`), then answers.
fn excerpt_builder(base: PathBuf, solve: bool) -> AgentBuilder {
    Arc::new(move |b: AgentBuild| {
        if solve {
            copy_dir(&base.join("reference").join(&b.task.id), &b.workdir);
        }
        Ok(agent_for(&b))
    })
}

fn excerpt_runner(solve: bool) -> (EvalRunner, PathBuf) {
    let src = excerpt();
    let raw = std::fs::read_to_string(src.join("tasks.toml")).unwrap();
    let tasks = TasksFile::from_toml_str("tasks.toml", &raw).unwrap_or_else(|e| panic!("the starter format must parse: {e}"));
    (EvalRunner::with_builder(tasks, src.clone(), "mock:default", excerpt_builder(src.clone(), solve)), src)
}

#[tokio::test]
async fn test_f11_the_starter_excerpt_should_pass_when_the_reference_is_applied() {
    if !have("python3") {
        eprintln!("skipping: python3 not installed");
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let (r, _) = excerpt_runner(true);

    let run = r.run(&EvalOptions { out_dir: out.path().to_path_buf(), ..Default::default() }).await.unwrap();

    for task in ["c01-py-fix-median", "m02-mem-staging-port"] {
        let outcome = read_json(&run_dir(&run, task).join("outcome.json"));
        assert_eq!(outcome["status"], "pass", "{task}: {outcome}\n{}", std::fs::read_to_string(run_dir(&run, task).join("check.txt")).unwrap_or_default());
    }
}

#[tokio::test]
async fn test_f11_the_starter_excerpt_should_fail_on_the_untouched_tasks() {
    if !have("python3") {
        eprintln!("skipping: python3 not installed");
        return;
    }
    let out = tempfile::tempdir().unwrap();
    let (r, _) = excerpt_runner(false);

    let run = r.run(&EvalOptions { out_dir: out.path().to_path_buf(), ..Default::default() }).await.unwrap();

    for task in ["c01-py-fix-median", "m02-mem-staging-port"] {
        assert_eq!(read_json(&run_dir(&run, task).join("outcome.json"))["status"], "fail", "{task}");
    }
}

#[tokio::test]
async fn test_f11_the_starter_memory_fixture_should_load_into_the_run_store() {
    let out = tempfile::tempdir().unwrap();
    let seen = Seen::default();
    let src = excerpt();
    let raw = std::fs::read_to_string(src.join("tasks.toml")).unwrap();
    let tasks = TasksFile::from_toml_str("tasks.toml", &raw).unwrap();
    let r = EvalRunner::with_builder(tasks, src, "mock:default", recording_builder(&seen));

    r.run(&EvalOptions { out_dir: out.path().to_path_buf(), ..Default::default() }).await.unwrap();

    let stores = seen.memories.lock().unwrap().clone();
    let mut counts = Vec::new();
    for s in &stores {
        counts.push(all_memories(s).await.len());
    }
    counts.sort_unstable();
    assert_eq!(counts, [0, 5], "the memory task's store holds its five fixture memories, the coding task's is empty");
}

#[test]
fn test_f11_check_should_accept_the_starter_excerpt() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config/hone")).unwrap();
    std::fs::write(home.join(".config/hone/config.toml"), "[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n").unwrap();
    let suite = tmp.path().join("suite");
    copy_dir(&excerpt(), &suite);

    let out = Command::new(env!("CARGO_BIN_EXE_hone"))
        .arg("eval")
        .arg(suite.join("tasks.toml"))
        .arg("--check")
        .args(["--model", "ollama:llama3.2"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&suite)
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(0), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    assert!(String::from_utf8_lossy(&out.stdout).contains("2 runs"), "{}", String::from_utf8_lossy(&out.stdout));
}

#[test]
fn test_f11_check_should_reject_a_malformed_memory_fixture_naming_file_and_line() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(home.join(".config/hone")).unwrap();
    std::fs::write(home.join(".config/hone/config.toml"), "[providers.ollama]\nbase_url = \"http://127.0.0.1:9\"\n").unwrap();
    let suite = tmp.path().join("suite");
    copy_dir(&excerpt(), &suite);
    let fixture = suite.join("fixtures/m02-mem-staging-port.jsonl");
    let good = std::fs::read_to_string(&fixture).unwrap();
    std::fs::write(&fixture, format!("{good}{{broken\n")).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_hone"))
        .arg("eval")
        .arg(suite.join("tasks.toml"))
        .arg("--check")
        .args(["--model", "ollama:llama3.2"])
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin")
        .current_dir(&suite)
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(2), "stdout: {}", String::from_utf8_lossy(&out.stdout));
    let err = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(err.contains("m02-mem-staging-port.jsonl") && err.contains("line 6"), "{err}");
}
