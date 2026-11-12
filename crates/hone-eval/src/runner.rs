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
use hone_memory::MemoryStore;
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
    /// Unfinished when `hone eval` was interrupted by a signal.
    Interrupted,
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
    /// Set to `true` to interrupt the run: running agents are stopped, their copies
    /// removed, unfinished runs marked `interrupted`, and the reports written for
    /// what finished.
    pub interrupt: Option<tokio::sync::watch::Receiver<bool>>,
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

pub struct EvalRunner {
    tasks: Arc<TasksFile>,
    base_dir: PathBuf,
    default_model: String,
    builder: AgentBuilder,
    user: Option<Arc<ConfigLayer>>,
    copy_warn_bytes: u64,
}

/// The size above which a task's copy is reported (F21).
const DEFAULT_COPY_WARN_BYTES: u64 = 200 * 1024 * 1024;

/// What one run produced, for the report and the history.
struct RunResult {
    planned: PlannedRun,
    stat: RunStat,
    outcome: TaskOutcome,
}

/// Everything a spawned run needs, cloned out of the runner.
#[derive(Clone)]
struct Ctx {
    tasks: Arc<TasksFile>,
    base_dir: PathBuf,
    model: String,
    builder: AgentBuilder,
    user: Option<Arc<ConfigLayer>>,
    run_dir: PathBuf,
    keep_workdirs: bool,
    copy_warn_bytes: u64,
    /// Tasks whose copy was already reported, and the warnings for the manifest.
    warned: Arc<std::sync::Mutex<(std::collections::HashSet<String>, Vec<String>)>>,
    interrupt: Option<tokio::sync::watch::Receiver<bool>>,
    integrity: Arc<Integrity>,
}

/// The suite directories that must not change during a run (F31).
struct Integrity {
    dirs: Vec<PathBuf>,
    baseline: String,
    violated: std::sync::atomic::AtomicBool,
}

const INTEGRITY_MESSAGE: &str = "suite modified during the run";

impl Integrity {
    fn new(dirs: Vec<PathBuf>) -> Self {
        let baseline = hash_dirs(&dirs);
        Self {
            dirs,
            baseline,
            violated: false.into(),
        }
    }

    fn is_violated(&self) -> bool {
        self.violated.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Re-hash the directories (after a check); true when anything changed, now or before.
    fn check(&self) -> bool {
        if !self.dirs.is_empty() && hash_dirs(&self.dirs) != self.baseline {
            self.violated
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.is_violated()
    }
}

/// SHA-256 over every file's relative path and content (and symlink targets).
fn hash_dirs(dirs: &[PathBuf]) -> String {
    use sha2::{Digest, Sha256};
    fn walk(root: &Path, dir: &Path, hasher: &mut Sha256) {
        let Ok(read) = std::fs::read_dir(dir) else {
            hasher.update(b"unreadable:");
            hasher.update(dir.to_string_lossy().as_bytes());
            return;
        };
        let mut entries: Vec<_> = read.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let rel = path.strip_prefix(root).unwrap_or(&path);
            hasher.update(rel.to_string_lossy().as_bytes());
            hasher.update([0]);
            match entry.file_type() {
                Ok(t) if t.is_dir() => {
                    hasher.update(b"d");
                    walk(root, &path, hasher);
                }
                Ok(t) if t.is_symlink() => {
                    hasher.update(b"l");
                    if let Ok(target) = std::fs::read_link(&path) {
                        hasher.update(target.to_string_lossy().as_bytes());
                    }
                }
                Ok(t) if t.is_file() => {
                    hasher.update(b"f");
                    hasher.update(std::fs::read(&path).unwrap_or_default());
                }
                _ => {}
            }
            hasher.update([0]);
        }
    }
    let mut hasher = Sha256::new();
    for dir in dirs {
        hasher.update(dir.to_string_lossy().as_bytes());
        hasher.update([1]);
        walk(dir, dir, &mut hasher);
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl EvalRunner {
    pub fn with_builder(
        tasks: TasksFile,
        base_dir: PathBuf,
        default_model: impl Into<String>,
        builder: AgentBuilder,
    ) -> Self {
        Self {
            tasks: Arc::new(tasks),
            base_dir,
            default_model: default_model.into(),
            builder,
            user: None,
            copy_warn_bytes: DEFAULT_COPY_WARN_BYTES,
        }
    }

    /// Report a task's copy in the manifest's `warnings` when it exceeds `bytes`.
    pub fn with_copy_warn_bytes(mut self, bytes: u64) -> Self {
        self.copy_warn_bytes = bytes;
        self
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
        let suite_sha = git_sha(&base_dir);
        let mut manifest = json!({
            "run_id": run_id,
            "hone_version": env!("CARGO_PKG_VERSION"),
            "suite_git_sha": suite_sha,
            "hone_git_sha": hone_git_sha(),
            "env": run_env(),
            "interrupted": false,
            "warnings": [],
            "model": model,
            "profiles": self.manifest_profiles(&profile_names),
            "tasks_sha256": hash,
            "repeats": repeats,
            "started_at": started.to_rfc3339(),
            "ended_at": Value::Null,
        });
        write_json(&run_dir.join("manifest.json"), &manifest)?;

        let ctx = Ctx {
            tasks: self.tasks.clone(),
            base_dir: base_dir.clone(),
            model: model.clone(),
            builder: self.builder.clone(),
            user: self.user.clone(),
            run_dir: run_dir.clone(),
            keep_workdirs: opts.keep_workdirs,
            copy_warn_bytes: self.copy_warn_bytes,
            warned: Arc::default(),
            interrupt: opts.interrupt.clone(),
            integrity: Arc::new(Integrity::new(
                self.tasks
                    .integrity()
                    .iter()
                    .map(|d| base_dir.join(d))
                    .collect(),
            )),
        };
        let planned_total = planned.len();
        let results = run_all(&ctx, planned, opts.jobs).await;
        let interrupted = ctx.interrupt.as_ref().is_some_and(|rx| *rx.borrow());

        manifest["ended_at"] = json!(Utc::now().to_rfc3339());
        manifest["interrupted"] = json!(interrupted);
        manifest["warnings"] = json!(
            ctx.warned
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .1
                .clone()
        );
        manifest["outcomes"] = status_counts(&results, planned_total);
        manifest["integrity_violated"] = json!(ctx.integrity.is_violated());
        write_json(&run_dir.join("manifest.json"), &manifest)?;

        // Unfinished runs are marked in their outcome.json; the report covers the rest.
        let finished: Vec<RunResult> = results
            .into_iter()
            .filter(|r| r.stat.status != TaskStatus::Interrupted)
            .collect();
        let results = finished;
        let mut report = self.report(&profile_names, &results);
        report.json["integrity_violated"] = json!(ctx.integrity.is_violated());
        if ctx.integrity.is_violated() {
            report
                .markdown
                .push_str("\n**Untrustworthy run: the suite was modified during the run.**\n");
        }
        write_json(&run_dir.join("report.json"), &report.json)?;
        std::fs::write(run_dir.join("report.md"), &report.markdown)?;

        if let Some(store) = opts.history.as_ref().filter(|_| !interrupted) {
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
                    manifest["suite_git_sha"].as_str().map(str::to_owned),
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

/// `git rev-parse HEAD` in `dir`, or null outside git.
fn git_sha(dir: &Path) -> Value {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .map_or(Value::Null, Value::String)
}

/// The git SHA hone was built from (build metadata), or null.
fn hone_git_sha() -> Value {
    match option_env!("HONE_GIT_SHA") {
        Some(sha) if !sha.is_empty() => Value::String(sha.to_owned()),
        _ => Value::Null,
    }
}

/// The environment variables that change runs, as strings or null.
fn run_env() -> Value {
    let mut vars = serde_json::Map::new();
    for name in [
        "HONE_AUDIT_LOG",
        "HONE_CONTEXT_WINDOW",
        "HONE_POLICY_BUILTINS",
        "HONE_PHASE_COMPACTION",
    ] {
        vars.insert(
            name.to_owned(),
            std::env::var(name).map_or(Value::Null, Value::String),
        );
    }
    Value::Object(vars)
}

/// Statuses of the planned runs; those that never started (an interrupt) count as
/// `interrupted`.
fn status_counts(results: &[RunResult], planned_total: usize) -> Value {
    let mut counts = serde_json::Map::new();
    for r in results {
        let key = serde_json::to_value(r.stat.status)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let n = counts.get(&key).and_then(Value::as_u64).unwrap_or(0);
        counts.insert(key, json!(n + 1));
    }
    let missing = planned_total.saturating_sub(results.len());
    if missing > 0 {
        let n = counts
            .get("interrupted")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        counts.insert("interrupted".to_owned(), json!(n + missing as u64));
    }
    Value::Object(counts)
}

fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
    std::fs::write(path, text)
}

fn interrupted(ctx: &Ctx) -> bool {
    ctx.interrupt.as_ref().is_some_and(|rx| *rx.borrow())
}

/// Resolves when the interrupt flag is set; never when there is none.
async fn wait_interrupt(rx: Option<tokio::sync::watch::Receiver<bool>>) {
    let Some(mut rx) = rx else {
        std::future::pending::<()>().await;
        return;
    };
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

async fn run_all(ctx: &Ctx, planned: Vec<PlannedRun>, jobs: usize) -> Vec<RunResult> {
    if jobs <= 1 {
        let mut out = Vec::with_capacity(planned.len());
        for p in planned {
            if interrupted(ctx) {
                break;
            }
            if let Some(r) = run_one(ctx, p).await {
                out.push(r);
            }
        }
        return out;
    }
    let gate = Arc::new(Semaphore::new(jobs));
    let mut handles = Vec::with_capacity(planned.len());
    for p in planned {
        let (ctx, gate) = (ctx.clone(), gate.clone());
        handles.push(tokio::spawn(async move {
            let _permit = gate.acquire().await;
            if interrupted(&ctx) {
                return None;
            }
            run_one(&ctx, p).await
        }));
    }
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(Some(r)) = h.await {
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
    /// Notes for `check.txt` (skipped special files).
    notes: Vec<String>,
}

struct CheckOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

impl Finished {
    fn without_agent(status: TaskStatus, message: String, duration_ms: u64) -> Self {
        Self {
            status,
            message: Some(message),
            duration_ms,
            usage: hone_proto::TokenUsage::default(),
            cost_usd: 0.0,
            check: None,
            receipts: Vec::new(),
            transcript: json!([]),
            notes: Vec::new(),
        }
    }

    fn infra(message: String, duration_ms: u64) -> Self {
        Self::without_agent(TaskStatus::InfraError, message, duration_ms)
    }

    fn context_tokens(&self) -> u64 {
        self.receipts
            .iter()
            .flat_map(|r| &r.blocks)
            .map(|b| u64::from(b.tokens))
            .sum()
    }
}

/// The message of a panic payload.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// Run one planned run in its own task, so a panic is a recorded `agent_error`
/// and an interrupt can drop it (stopping its commands and removing its copy).
/// `None`: interrupted before it started.
async fn run_one(ctx: &Ctx, planned: PlannedRun) -> Option<RunResult> {
    if interrupted(ctx) {
        return None;
    }
    let dir = ctx
        .run_dir
        .join(&planned.profile)
        .join(&planned.task)
        .join(format!("r{}", planned.repeat));
    let started = Instant::now();
    let finished = match std::fs::create_dir_all(&dir) {
        Err(e) => Finished::infra(format!("cannot create {}: {e}", dir.display()), 0),
        Ok(()) => {
            let (c, p, d) = (ctx.clone(), planned.clone(), dir.clone());
            let mut handle = tokio::spawn(async move { execute(&c, &p, &d, started).await });
            tokio::select! {
                joined = &mut handle => match joined {
                    Ok(f) => f,
                    Err(e) if e.is_panic() => Finished::without_agent(
                        TaskStatus::AgentError,
                        format!("panic: {}", panic_message(e.into_panic())),
                        started.elapsed().as_millis() as u64,
                    ),
                    Err(e) => Finished::without_agent(
                        TaskStatus::AgentError,
                        format!("the run was cancelled: {e}"),
                        started.elapsed().as_millis() as u64,
                    ),
                },
                () = wait_interrupt(ctx.interrupt.clone()) => {
                    // Dropping the task's future stops its commands and removes its copy.
                    handle.abort();
                    let _ = (&mut handle).await;
                    Finished::without_agent(
                        TaskStatus::Interrupted,
                        "interrupted by a signal".to_owned(),
                        started.elapsed().as_millis() as u64,
                    )
                }
            }
        }
    };
    persist(&dir, &finished);

    let check = finished.check.as_ref();
    Some(RunResult {
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
    })
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
    let mut check_txt = String::new();
    for note in &f.notes {
        check_txt.push_str(&format!("note: {note}\n"));
    }
    match &f.check {
        Some(c) => check_txt.push_str(&format!(
            "exit code: {}\n--- stdout ---\n{}\n--- stderr ---\n{}\n",
            c.exit_code.map_or("none".to_owned(), |n| n.to_string()),
            c.stdout,
            c.stderr
        )),
        None => check_txt.push_str("check not run\n"),
    }
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

/// Removes files and directories when dropped (also when a run's future is dropped).
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_dir_all(path);
            let _ = std::fs::remove_file(path);
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

    if ctx.integrity.is_violated() {
        return Finished::infra(INTEGRITY_MESSAGE.to_owned(), 0);
    }
    // The fixture is read first, so a malformed one stops the run cheaply.
    let fixture = match &task.memory_fixture {
        Some(f) => match crate::fixture::load(&ctx.base_dir.join(f)) {
            Ok(m) => m,
            Err(e) => return Finished::infra(e, ms(started)),
        },
        None => Vec::new(),
    };
    let selection = match ctx
        .tasks
        .profile_selection(&planned.profile, ctx.user.as_deref())
    {
        Ok(s) => s,
        Err(e) => return Finished::infra(e.to_string(), ms(started)),
    };

    // A fresh store, as the CLI has: a SQLite file in the temp dir, deleted with the run.
    let id = uuid::Uuid::now_v7();
    let db = std::env::temp_dir().join(format!("hone-eval-mem-{id}.db"));
    let mut cleanup = Cleanup(vec![
        db.clone(),
        PathBuf::from(format!("{}-wal", db.display())),
        PathBuf::from(format!("{}-shm", db.display())),
        PathBuf::from(format!("{}-journal", db.display())),
    ]);
    let memory = match hone_memory::SqliteStore::open_with_decay_interval(&db, u64::MAX).await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            return Finished::infra(format!("cannot open the memory store: {e}"), ms(started));
        }
    };
    for m in fixture {
        if let Err(e) = memory.record(m).await {
            return Finished::infra(
                format!(
                    "memory fixture {}: {e}",
                    task.memory_fixture.as_deref().unwrap_or("")
                ),
                ms(started),
            );
        }
    }

    // A fresh copy of the task directory, outside the repository.
    let copy = std::env::temp_dir().join(format!("hone-eval-{id}"));
    cleanup.0.push(copy.clone());
    let source = ctx.base_dir.join(&task.working_dir);
    let report = match crate::copy::copy_task_dir(&source, &copy) {
        Ok(r) => r,
        Err(e) => return Finished::infra(e, ms(started)),
    };
    if report.bytes > ctx.copy_warn_bytes {
        let mut warned = ctx.warned.lock().unwrap_or_else(|e| e.into_inner());
        if warned.0.insert(task.id.clone()) {
            let msg = format!(
                "task `{}`: its working-directory copy is {} MB; add large directories to \
                 .gitignore to keep copies lean",
                task.id,
                report.bytes / (1024 * 1024)
            );
            tracing::warn!("eval: {msg}");
            warned.1.push(msg);
        }
    }

    let mut result = run_in_copy(ctx, planned, &task, &copy, memory, selection, started).await;
    result.notes = report.notes;

    if ctx.keep_workdirs {
        if let Err(e) = crate::copy::copy_task_dir(&copy, &dir.join("workdir")) {
            tracing::warn!("eval: cannot keep the working directory: {e}");
        }
    }
    drop(cleanup);
    result
}

async fn run_in_copy(
    ctx: &Ctx,
    planned: &PlannedRun,
    task: &TaskSpec,
    workdir: &Path,
    memory: Arc<hone_memory::SqliteStore>,
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
        notes: Vec::new(),
    };
    match turn {
        Err(_) => finish(
            TaskStatus::Timeout,
            Some(format!("timed out after {}s", task.timeout_secs)),
            agent.session_usage(),
            agent.session_cost(),
            None,
        ),
        Ok(Err(e)) => finish(
            TaskStatus::AgentError,
            Some(e.to_string()),
            agent.session_usage(),
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
            if ctx.integrity.check() {
                let mut out = finish(
                    TaskStatus::InfraError,
                    Some(INTEGRITY_MESSAGE.to_owned()),
                    turn.usage,
                    turn.cost_usd,
                    Some(check),
                );
                out.status = TaskStatus::InfraError;
                return out;
            }
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

/// Kill a process group (and so every process the check left behind).
#[cfg(unix)]
fn kill_group(pgid: u32) {
    if pgid > 1 {
        // SAFETY: `killpg` only sends a signal; the group id is that of a child
        // this function started with `process_group(0)`.
        unsafe {
            libc::killpg(pgid as libc::pid_t, libc::SIGKILL);
        }
    }
}

/// Run the success check in the copy, in its own process group that is killed
/// when the check times out and after it finishes (F25). `EVAL_SUITE_DIR` is set
/// on the check's process only, never on this process: the agent runs in-process
/// and its shell would inherit it. Output goes to temp files, not pipes, so a
/// background process holding them open cannot keep this waiting.
async fn run_check(cmd: &str, workdir: &Path, suite_dir: &Path, timeout_secs: u64) -> CheckOutput {
    let limit = Duration::from_secs(timeout_secs.max(60));
    let id = uuid::Uuid::now_v7();
    let (out_path, err_path) = (
        std::env::temp_dir().join(format!("hone-eval-check-{id}.out")),
        std::env::temp_dir().join(format!("hone-eval-check-{id}.err")),
    );
    let _cleanup = Cleanup(vec![out_path.clone(), err_path.clone()]);
    let fail = |msg: String| CheckOutput {
        exit_code: None,
        stdout: String::new(),
        stderr: msg,
    };
    let (Ok(out_file), Ok(err_file)) = (
        std::fs::File::create(&out_path),
        std::fs::File::create(&err_path),
    ) else {
        return fail("cannot create the check's output files".to_owned());
    };

    let mut command = tokio::process::Command::new("sh");
    command
        .arg("-c")
        .arg(cmd)
        .current_dir(workdir)
        .env("EVAL_SUITE_DIR", suite_dir)
        .stdin(std::process::Stdio::null())
        .stdout(out_file)
        .stderr(err_file)
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("cannot run the check: {e}")),
    };
    let pgid = child.id();

    let waited = tokio::time::timeout(limit, child.wait()).await;
    #[cfg(unix)]
    if let Some(pgid) = pgid {
        kill_group(pgid);
    }
    #[cfg(not(unix))]
    let _ = pgid;
    let status = match waited {
        Ok(Ok(status)) => status,
        Ok(Err(e)) => return fail(format!("cannot run the check: {e}")),
        Err(_) => {
            let _ = child.kill().await;
            let mut out = fail(format!("the check timed out after {}s", limit.as_secs()));
            out.stdout = std::fs::read_to_string(&out_path).unwrap_or_default();
            return out;
        }
    };
    CheckOutput {
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&std::fs::read(&out_path).unwrap_or_default()).into_owned(),
        stderr: String::from_utf8_lossy(&std::fs::read(&err_path).unwrap_or_default()).into_owned(),
    }
}
