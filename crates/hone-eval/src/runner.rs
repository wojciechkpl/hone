//! Running a tasks file: a fresh copy of each task's directory, a fresh memory
//! store, a real agent from a pluggable builder, the success check, and the run
//! directory with its report (F1, F2, F4, F7, F8, F9, F10).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use hone_config::resolve::ConfigLayer;
use hone_config::strategy::StrategySelection;
use hone_core::Agent;
use hone_memory::{InMemoryStore, MemoryStore};
use hone_proto::{AgentEvent, ContextReceipt, canonical_json_sha256};
use serde_json::{Value, json};
use tokio::sync::{Semaphore, mpsc};

use crate::config::TaskSpec;
use crate::history::{EvalRunStore, ProfileRunSummary};
use crate::stats::{
    BOOTSTRAP_RESAMPLES, BOOTSTRAP_SEED, RunStat, paired_comparison, pareto_flags, summarize,
};
use crate::tasks::{ExpandOptions, PlannedRun, TasksFile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    #[serde(alias = "Pass")]
    Pass,
    #[serde(alias = "Fail")]
    Fail,
    #[serde(alias = "Timeout")]
    Timeout,
    #[serde(alias = "AgentError")]
    AgentError,
    /// The run could not start: a failed copy, fixture or agent build.
    #[serde(alias = "InfraError")]
    InfraError,
}

/// One task outcome as the history store and the regression bisector see it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskOutcome {
    pub id: String,
    pub status: TaskStatus,
    pub duration_ms: u64,
    pub tokens_input: u32,
    pub tokens_output: u32,
    pub cost_usd: f64,
    pub success_check_stdout: String,
    pub success_check_stderr: String,
    pub success_check_exit_code: Option<i32>,
}

/// What the runner hands to the agent builder for one run.
pub struct AgentBuild {
    pub task: TaskSpec,
    pub profile: String,
    pub repeat: u32,
    /// The fresh copy of the task directory.
    pub workdir: PathBuf,
    /// A fresh store, loaded from the task's fixture.
    pub memory: Arc<dyn MemoryStore>,
    /// `--model`, else the tasks file's `model`, else the default.
    pub model: String,
    /// The profile, resolved.
    pub selection: StrategySelection,
}

pub type AgentBuilder = Arc<dyn Fn(AgentBuild) -> Result<Agent, String> + Send + Sync>;

#[derive(Default)]
pub struct EvalOptions {
    pub out_dir: PathBuf,
    pub profiles: Vec<String>,
    pub repeats: Option<u32>,
    /// 0 or 1 = sequential.
    pub jobs: usize,
    pub keep_workdirs: bool,
    pub model: Option<String>,
    pub tag: Option<String>,
    pub history: Option<Arc<EvalRunStore>>,
}

#[derive(Debug, thiserror::Error)]
pub enum EvalRunnerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Config(#[from] crate::config::EvalConfigError),
    #[error("history: {0}")]
    History(#[from] crate::history::EvalHistoryError),
}

/// Working-directory copies of finished runs. A copy is removed when the next
/// run finishes (so at most one finished copy lingers, for inspection), and
/// whatever is left is removed when the runner is dropped.
#[derive(Default)]
struct Graveyard(std::sync::Mutex<std::collections::VecDeque<PathBuf>>);

impl Graveyard {
    fn retire(&self, copy: PathBuf) {
        let mut queue = self.0.lock().unwrap_or_else(|e| e.into_inner());
        queue.push_back(copy);
        while queue.len() > 1 {
            if let Some(old) = queue.pop_front() {
                let _ = std::fs::remove_dir_all(old);
            }
        }
    }
}

impl Drop for Graveyard {
    fn drop(&mut self) {
        let queue = self.0.get_mut().unwrap_or_else(|e| e.into_inner());
        for copy in queue.drain(..) {
            let _ = std::fs::remove_dir_all(copy);
        }
    }
}

pub struct EvalRunner {
    graveyard: Arc<Graveyard>,
    tasks: Arc<TasksFile>,
    base_dir: PathBuf,
    default_model: String,
    builder: AgentBuilder,
    user: Option<Arc<ConfigLayer>>,
}

/// What one run produced, for the report and the history.
struct RunResult {
    planned: PlannedRun,
    stat: RunStat,
    outcome: TaskOutcome,
}

/// Everything a spawned run needs, cloned out of the runner.
#[derive(Clone)]
struct Ctx {
    graveyard: Arc<Graveyard>,
    tasks: Arc<TasksFile>,
    base_dir: PathBuf,
    model: String,
    builder: AgentBuilder,
    user: Option<Arc<ConfigLayer>>,
    run_dir: PathBuf,
    keep_workdirs: bool,
}

impl EvalRunner {
    pub fn with_builder(
        tasks: TasksFile,
        base_dir: PathBuf,
        default_model: impl Into<String>,
        builder: AgentBuilder,
    ) -> Self {
        Self {
            graveyard: Arc::new(Graveyard::default()),
            tasks: Arc::new(tasks),
            base_dir,
            default_model: default_model.into(),
            builder,
            user: None,
        }
    }

    /// Resolve each profile over this user config layer (`config.toml`).
    pub fn with_user_layer(mut self, layer: ConfigLayer) -> Self {
        self.user = Some(Arc::new(layer));
        self
    }

    /// Run every planned run and return `<out_dir>/<run-id>`.
    pub async fn run(&self, opts: &EvalOptions) -> Result<PathBuf, EvalRunnerError> {
        let planned = self.tasks.expand(&ExpandOptions {
            profiles: opts.profiles.clone(),
            repeats: opts.repeats,
        })?;
        let model = opts
            .model
            .clone()
            .or_else(|| self.tasks.model.clone())
            .unwrap_or_else(|| self.default_model.clone());
        let base_dir = std::fs::canonicalize(&self.base_dir)
            .or_else(|_| std::path::absolute(&self.base_dir))?;

        let started = Utc::now();
        let hash = self.tasks.source_sha256().to_owned();
        let (run_id, run_dir) = unique_run_dir(&opts.out_dir, started, &hash)?;
        let repeats = opts.repeats.unwrap_or_else(|| self.tasks.repeats());

        let profile_names: Vec<String> = self
            .tasks
            .profile_names()
            .into_iter()
            .filter(|n| opts.profiles.is_empty() || opts.profiles.contains(n))
            .collect();
        let mut manifest = json!({
            "run_id": run_id,
            "hone_version": env!("CARGO_PKG_VERSION"),
            "git_sha": git_sha(&base_dir),
            "model": model,
            "profiles": self.manifest_profiles(&profile_names),
            "tasks_sha256": hash,
            "repeats": repeats,
            "started_at": started.to_rfc3339(),
            "ended_at": Value::Null,
        });
        write_json(&run_dir.join("manifest.json"), &manifest)?;

        let ctx = Ctx {
            graveyard: self.graveyard.clone(),
            tasks: self.tasks.clone(),
            base_dir,
            model: model.clone(),
            builder: self.builder.clone(),
            user: self.user.clone(),
            run_dir: run_dir.clone(),
            keep_workdirs: opts.keep_workdirs,
        };
        let results = run_all(&ctx, planned, opts.jobs).await;

        manifest["ended_at"] = json!(Utc::now().to_rfc3339());
        write_json(&run_dir.join("manifest.json"), &manifest)?;

        let report = self.report(&profile_names, &results);
        write_json(&run_dir.join("report.json"), &report.json)?;
        std::fs::write(run_dir.join("report.md"), &report.markdown)?;

        if let Some(store) = &opts.history {
            let base = self.tasks.base_profile();
            let outcomes: Vec<TaskOutcome> = results
                .iter()
                .filter(|r| r.planned.profile == base && r.planned.repeat == 1)
                .map(|r| r.outcome.clone())
                .collect();
            let profiles: Vec<ProfileRunSummary> = report
                .summaries
                .iter()
                .map(|s| ProfileRunSummary {
                    profile: s.profile.clone(),
                    runs: s.runs,
                    passes: s.passes,
                })
                .collect();
            store
                .record_with_profiles(
                    manifest["git_sha"].as_str().map(str::to_owned),
                    model,
                    manifest["tasks_sha256"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    opts.tag.clone(),
                    outcomes,
                    profiles,
                )
                .await?;
        }
        Ok(run_dir)
    }

    /// `[{ "name", "strategies": [{ "slot", "name", "params_sha256" }] }]`.
    fn manifest_profiles(&self, names: &[String]) -> Value {
        let entries: Vec<Value> = names
            .iter()
            .map(|name| {
                let strategies = match self.tasks.profile_selection(name, self.user.as_deref()) {
                    Ok(sel) => strategy_ids(&sel),
                    Err(_) => Vec::new(),
                };
                json!({ "name": name, "strategies": strategies })
            })
            .collect();
        Value::Array(entries)
    }

    fn report(&self, profile_names: &[String], results: &[RunResult]) -> Report {
        let stats_of = |profile: &str| -> Vec<RunStat> {
            results
                .iter()
                .filter(|r| r.planned.profile == profile)
                .map(|r| r.stat.clone())
                .collect()
        };
        let ran: Vec<&String> = profile_names
            .iter()
            .filter(|n| results.iter().any(|r| &r.planned.profile == *n))
            .collect();
        let summaries: Vec<_> = ran.iter().map(|n| summarize(n, &stats_of(n))).collect();
        let by_cost: Vec<(f64, f64)> = summaries
            .iter()
            .map(|s| (s.pass_rate, s.mean_cost_usd))
            .collect();
        let by_context: Vec<(f64, f64)> = summaries
            .iter()
            .map(|s| (s.pass_rate, s.mean_context_tokens))
            .collect();
        let (pareto_cost, pareto_context) = (pareto_flags(&by_cost), pareto_flags(&by_context));

        let base = self.tasks.base_profile();
        let base_stats = ran.iter().any(|n| **n == base).then(|| stats_of(&base));
        let mut profiles = Vec::new();
        let mut md = String::from("# Hone eval report\n\n");
        md.push_str(
            "| profile | runs | pass rate | 95% CI | timeouts | errors | mean cost $ | median cost $ | mean context tokens | mean calls | pareto (cost) | pareto (context) |\n",
        );
        md.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        let mut compare = String::new();
        for (i, s) in summaries.iter().enumerate() {
            let vs_base = match (&base_stats, s.profile != base) {
                (Some(b), true) => {
                    let p = paired_comparison(
                        b,
                        &stats_of(&s.profile),
                        BOOTSTRAP_RESAMPLES,
                        BOOTSTRAP_SEED,
                    );
                    compare.push_str(&format!(
                        "| {} | {} | {:+.3} | {} / {} / {} | [{:+.3}, {:+.3}] |\n",
                        s.profile,
                        p.tasks,
                        p.mean_diff,
                        p.better,
                        p.worse,
                        p.tied,
                        p.ci_low,
                        p.ci_high
                    ));
                    json!({
                        "tasks": p.tasks, "mean_diff": p.mean_diff, "better": p.better,
                        "worse": p.worse, "tied": p.tied, "ci_low": p.ci_low, "ci_high": p.ci_high,
                    })
                }
                _ => Value::Null,
            };
            md.push_str(&format!(
                "| {} | {} | {:.0}% | {:.0}%-{:.0}% | {} | {} | {:.4} | {:.4} | {:.0} | {:.1} | {} | {} |\n",
                s.profile,
                s.runs,
                s.pass_rate * 100.0,
                s.wilson_low * 100.0,
                s.wilson_high * 100.0,
                s.timeouts,
                s.errors,
                s.mean_cost_usd,
                s.median_cost_usd,
                s.mean_context_tokens,
                s.mean_model_calls,
                if pareto_cost[i] { "yes" } else { "" },
                if pareto_context[i] { "yes" } else { "" },
            ));
            let mut entry = serde_json::to_value(s).unwrap_or(Value::Null);
            entry["pareto_cost"] = json!(pareto_cost[i]);
            entry["pareto_context"] = json!(pareto_context[i]);
            entry["vs_base"] = vs_base;
            profiles.push(entry);
        }
        if !compare.is_empty() {
            md.push_str(&format!(
                "\n## Paired comparison with `{base}`\n\n| profile | tasks | mean diff | better / worse / tied | 95% bootstrap CI |\n|---|---|---|---|---|\n{compare}"
            ));
        }
        Report {
            json: json!({ "base_profile": base, "profiles": profiles }),
            markdown: md,
            summaries,
        }
    }
}

struct Report {
    json: Value,
    markdown: String,
    summaries: Vec<crate::stats::ProfileSummary>,
}

/// The resolved strategies of a selection, as `{ slot, name, params_sha256 }`.
fn strategy_ids(sel: &StrategySelection) -> Vec<Value> {
    let id = |slot: &str, name: &str, params: Value| json!({ "slot": slot, "name": name, "params_sha256": canonical_json_sha256(&params) });
    let mut out: Vec<Value> = sel
        .sources
        .value
        .iter()
        .map(|s| id("source", s, json!({})))
        .collect();
    out.push(id(
        "skeleton",
        &sel.skeleton.value,
        json!({
            "max_tokens": sel.skeleton_max_tokens.value,
            "gap_threshold": sel.skeleton_gap_threshold.value,
        }),
    ));
    out.push(id(
        "compactor",
        &sel.compactor.value,
        json!({ "phase": sel.graduated_phase.value, "budget_ratio": sel.budget_ratio.value }),
    ));
    out.push(id(
        "recaller",
        &sel.recaller.value,
        json!({ "top_k": sel.top_k.value, "exclude_kinds": sel.exclude_kinds.value }),
    ));
    out.push(id("writer", &sel.writer.value, json!({})));
    out
}

/// `<out>/<%Y%m%dT%H%M%SZ>-<8 hex of the file hash>`, one second later while taken.
fn unique_run_dir(
    out: &Path,
    mut at: DateTime<Utc>,
    hash: &str,
) -> std::io::Result<(String, PathBuf)> {
    let short = &hash[..8.min(hash.len())];
    std::fs::create_dir_all(out)?;
    loop {
        let id = format!("{}-{short}", at.format("%Y%m%dT%H%M%SZ"));
        let dir = out.join(&id);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                at += chrono::Duration::seconds(1);
            }
            Err(e) => return Err(e),
        }
    }
}

fn git_sha(dir: &Path) -> Value {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .map_or(Value::Null, Value::String)
}

fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}

async fn run_all(ctx: &Ctx, planned: Vec<PlannedRun>, jobs: usize) -> Vec<RunResult> {
    if jobs <= 1 {
        let mut out = Vec::with_capacity(planned.len());
        for p in planned {
            out.push(run_one(ctx, p).await);
        }
        return out;
    }
    let gate = Arc::new(Semaphore::new(jobs));
    let mut handles = Vec::with_capacity(planned.len());
    for p in planned {
        let (ctx, gate) = (ctx.clone(), gate.clone());
        handles.push(tokio::spawn(async move {
            let _permit = gate.acquire().await;
            run_one(&ctx, p).await
        }));
    }
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(r) = h.await {
            out.push(r);
        }
    }
    out
}

/// How a run ended, with what it produced.
struct Finished {
    status: TaskStatus,
    message: Option<String>,
    duration_ms: u64,
    usage: hone_proto::TokenUsage,
    cost_usd: f64,
    check: Option<CheckOutput>,
    receipts: Vec<ContextReceipt>,
    transcript: Value,
}

struct CheckOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Finished {
    fn infra(message: String, duration_ms: u64) -> Self {
        Self {
            status: TaskStatus::InfraError,
            message: Some(message),
            duration_ms,
            usage: hone_proto::TokenUsage::default(),
            cost_usd: 0.0,
            check: None,
            receipts: Vec::new(),
            transcript: json!([]),
        }
    }

    fn context_tokens(&self) -> u64 {
        self.receipts
            .iter()
            .flat_map(|r| &r.blocks)
            .map(|b| u64::from(b.tokens))
            .sum()
    }
}

async fn run_one(ctx: &Ctx, planned: PlannedRun) -> RunResult {
    let dir = ctx
        .run_dir
        .join(&planned.profile)
        .join(&planned.task)
        .join(format!("r{}", planned.repeat));
    let started = Instant::now();
    let finished = match std::fs::create_dir_all(&dir) {
        Ok(()) => execute(ctx, &planned, &dir, started).await,
        Err(e) => Finished::infra(format!("cannot create {}: {e}", dir.display()), 0),
    };
    persist(&dir, &finished);

    let check = finished.check.as_ref();
    RunResult {
        stat: RunStat {
            task: planned.task.clone(),
            status: finished.status,
            cost_usd: finished.cost_usd,
            context_tokens: finished.context_tokens(),
            model_calls: finished.receipts.len() as u32,
        },
        outcome: TaskOutcome {
            id: planned.task.clone(),
            status: finished.status,
            duration_ms: finished.duration_ms,
            tokens_input: finished.usage.input,
            tokens_output: finished.usage.output,
            cost_usd: finished.cost_usd,
            success_check_stdout: check.map(|c| c.stdout.clone()).unwrap_or_default(),
            success_check_stderr: check.map(|c| c.stderr.clone()).unwrap_or_default(),
            success_check_exit_code: check.and_then(|c| c.exit_code),
        },
        planned,
    }
}

/// Write the per-run files.
fn persist(dir: &Path, f: &Finished) {
    let mut outcome = json!({
        "status": f.status,
        "duration_ms": f.duration_ms,
        "tokens_input": f.usage.input,
        "tokens_output": f.usage.output,
        "tokens_cache": u64::from(f.usage.cache_read) + u64::from(f.usage.cache_creation),
        "cost_usd": f.cost_usd,
        "model_calls": f.receipts.len(),
        "context_tokens": f.context_tokens(),
        "check_exit_code": f.check.as_ref().and_then(|c| c.exit_code),
    });
    if let Some(m) = &f.message {
        outcome["message"] = json!(m);
    }
    let check_txt = match &f.check {
        Some(c) => format!(
            "exit code: {}\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
            c.exit_code.map_or("none".to_owned(), |n| n.to_string()),
            c.stdout,
            c.stderr
        ),
        None => "check not run\n".to_owned(),
    };
    let receipts = serde_json::to_value(&f.receipts).unwrap_or_else(|_| json!([]));
    for (name, result) in [
        (
            "outcome.json",
            write_json(&dir.join("outcome.json"), &outcome),
        ),
        (
            "receipts.json",
            write_json(&dir.join("receipts.json"), &receipts),
        ),
        (
            "transcript.json",
            write_json(&dir.join("transcript.json"), &f.transcript),
        ),
        (
            "check.txt",
            std::fs::write(dir.join("check.txt"), check_txt),
        ),
    ] {
        if let Err(e) = result {
            tracing::warn!("eval: cannot write {}: {e}", dir.join(name).display());
        }
    }
}

async fn execute(ctx: &Ctx, planned: &PlannedRun, dir: &Path, started: Instant) -> Finished {
    let ms = |s: Instant| s.elapsed().as_millis() as u64;
    let Some(task) = ctx
        .tasks
        .tasks
        .iter()
        .find(|t| t.id == planned.task)
        .cloned()
    else {
        return Finished::infra(format!("unknown task `{}`", planned.task), 0);
    };

    // A fresh memory store from the fixture.
    let memory = Arc::new(InMemoryStore::new());
    if let Some(f) = &task.memory_fixture {
        let memories = match crate::fixture::load(&ctx.base_dir.join(f)) {
            Ok(m) => m,
            Err(e) => return Finished::infra(e, ms(started)),
        };
        for m in memories {
            if let Err(e) = memory.record(m).await {
                return Finished::infra(format!("memory fixture {f}: {e}"), ms(started));
            }
        }
    }
    let selection = match ctx
        .tasks
        .profile_selection(&planned.profile, ctx.user.as_deref())
    {
        Ok(s) => s,
        Err(e) => return Finished::infra(e.to_string(), ms(started)),
    };

    // A fresh copy of the task directory, outside the repository.
    let copy = std::env::temp_dir().join(format!("hone-eval-{}", uuid::Uuid::now_v7()));
    let source = ctx.base_dir.join(&task.working_dir);
    if let Err(e) = copy_dir(&source, &copy) {
        let _ = std::fs::remove_dir_all(&copy);
        return Finished::infra(
            format!(
                "cannot copy {} to {}: {e}",
                source.display(),
                copy.display()
            ),
            ms(started),
        );
    }

    let result = run_in_copy(ctx, planned, &task, &copy, memory, selection, started).await;

    if ctx.keep_workdirs {
        if let Err(e) = copy_dir(&copy, &dir.join("workdir")) {
            tracing::warn!("eval: cannot keep the working directory: {e}");
        }
    }
    ctx.graveyard.retire(copy);
    result
}

async fn run_in_copy(
    ctx: &Ctx,
    planned: &PlannedRun,
    task: &TaskSpec,
    workdir: &Path,
    memory: Arc<InMemoryStore>,
    selection: StrategySelection,
    started: Instant,
) -> Finished {
    let ms = |s: Instant| s.elapsed().as_millis() as u64;
    let build = AgentBuild {
        task: task.clone(),
        profile: planned.profile.clone(),
        repeat: planned.repeat,
        workdir: workdir.to_path_buf(),
        memory,
        model: ctx.model.clone(),
        selection,
    };
    let mut agent = match (ctx.builder)(build) {
        Ok(a) => a,
        Err(e) => return Finished::infra(format!("cannot build the agent: {e}"), ms(started)),
    };

    // Receipts come from the event stream.
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
    let collector = tokio::spawn(async move {
        let mut receipts = Vec::new();
        while let Some(event) = rx.recv().await {
            if let AgentEvent::ContextAssembled { receipt } = event {
                receipts.push(receipt);
            }
        }
        receipts
    });

    // Dropping the turn's future on a timeout stops its commands.
    let turn = tokio::time::timeout(
        Duration::from_secs(task.timeout_secs),
        agent.run_turn(&task.prompt, tx),
    )
    .await;
    let duration_ms = ms(started);
    let receipts = collector.await.unwrap_or_default();
    let transcript = redacted_transcript(&agent);

    let finish = |status, message, usage, cost_usd, check| Finished {
        status,
        message,
        duration_ms,
        usage,
        cost_usd,
        check,
        receipts,
        transcript,
    };
    match turn {
        Err(_) => finish(
            TaskStatus::Timeout,
            Some(format!("timed out after {}s", task.timeout_secs)),
            hone_proto::TokenUsage::default(),
            agent.session_cost(),
            None,
        ),
        Ok(Err(e)) => finish(
            TaskStatus::AgentError,
            Some(e.to_string()),
            hone_proto::TokenUsage::default(),
            agent.session_cost(),
            None,
        ),
        Ok(Ok(turn)) => {
            let check = run_check(
                &task.success_check,
                workdir,
                &ctx.base_dir,
                task.timeout_secs,
            )
            .await;
            let status = if check.exit_code == Some(0) {
                TaskStatus::Pass
            } else {
                TaskStatus::Fail
            };
            finish(status, None, turn.usage, turn.cost_usd, Some(check))
        }
    }
}

fn redacted_transcript(agent: &Agent) -> Value {
    let messages = serde_json::to_value(agent.messages()).unwrap_or_else(|_| json!([]));
    hone_tools::redact::redact_json(&messages).0
}

/// Run the success check in the copy. `EVAL_SUITE_DIR` is set on the check's
/// process only, never on this process: the agent runs in-process and its shell
/// would inherit it.
async fn run_check(cmd: &str, workdir: &Path, suite_dir: &Path, timeout_secs: u64) -> CheckOutput {
    let limit = Duration::from_secs(timeout_secs.max(60));
    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(workdir)
        .env("EVAL_SUITE_DIR", suite_dir)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    match tokio::time::timeout(limit, command.output()).await {
        Ok(Ok(out)) => CheckOutput {
            exit_code: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        },
        Ok(Err(e)) => CheckOutput {
            exit_code: None,
            stdout: String::new(),
            stderr: format!("cannot run the check: {e}"),
        },
        Err(_) => CheckOutput {
            exit_code: None,
            stdout: String::new(),
            stderr: format!("the check timed out after {}s", limit.as_secs()),
        },
    }
}

/// Copy a directory tree, skipping `target/` and `node_modules/` (`.git/` is
/// copied, so git-based checks work).
fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        let (src, dst) = (entry.path(), to.join(&name));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if name == "target" || name == "node_modules" {
                continue;
            }
            copy_dir(&src, &dst)?;
        } else if kind.is_symlink() {
            #[cfg(unix)]
            std::os::unix::fs::symlink(std::fs::read_link(&src)?, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}
