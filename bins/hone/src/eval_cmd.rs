//! `hone eval`: run a tasks file with real agents, in isolated copies, and compare
//! strategy profiles. See `docs/plans/eval-profiles-spec.md`.
//!
//! The agent of each run is built like a one-shot `hone` run builds its own: the
//! built-in tools, the sandbox, the permission rules of the run's directory, a
//! symbol index of it, the strategy selection of the run's profile, receipts. Two
//! deliberate differences keep runs hermetic: no MCP servers are started, and no
//! language server is spawned.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use hone_config::HoneMd;
use hone_config::resolve::{Effective, ModelChoice, ModelSource};
use hone_core::Agent;
use hone_core::context_injector::SkeletonStrategy;
use hone_eval::tasks::{ExpandOptions, TasksFile};
use hone_eval::{AgentBuild, AgentBuilder, EvalOptions, EvalRunStore, EvalRunner};
use hone_index::SqliteIndex;
use hone_proto::ModelId;
use hone_tools::{LspBridge, MockLspBridge};

/// Arguments of `hone eval`.
#[derive(clap::Args, Clone, Debug)]
pub struct EvalArgs {
    /// Path to the tasks.toml file.
    pub tasks: PathBuf,
    /// Directory for run directories (default: ./eval-runs).
    #[arg(short, long)]
    pub output: Option<PathBuf>,
    /// Validate the tasks file, profiles, files and provider, then exit (no model call).
    #[arg(long)]
    pub check: bool,
    /// Runs per (profile, task); overrides `[eval] repeats`.
    #[arg(long)]
    pub repeats: Option<u32>,
    /// Run only this profile (repeatable). With --ablate: the base profile.
    #[arg(long = "profile")]
    pub profile: Vec<String>,
    /// Runs in parallel.
    #[arg(long, default_value_t = 1)]
    pub jobs: usize,
    /// Keep each run's working directory in its run directory.
    #[arg(long)]
    pub keep_workdirs: bool,
    /// Vary one slot of the base profile: context.skeleton, context.compactor,
    /// context.graduated.phase, memory.recaller or context.sources:<name>.
    #[arg(long)]
    pub ablate: Option<String>,
    /// Model to run (default: the tasks file's `model`, then the configured default).
    #[arg(short, long)]
    pub model: Option<String>,
    /// Label for the history record.
    #[arg(long)]
    pub tag: Option<String>,
}

fn fail(errors: &[String]) -> ! {
    for e in errors {
        eprintln!("hone eval: {e}");
    }
    std::process::exit(2);
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("{n} {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The provider of `model` must be buildable from the configuration (no call).
fn check_provider(
    model: &str,
    source: ModelSource,
    eff: &Effective,
) -> std::result::Result<(), String> {
    const PREFIXES: [&str; 5] = [
        "anthropic:",
        "openai:",
        "ollama:",
        "claude-cli:",
        "gemini-cli:",
    ];
    if model.starts_with("claude-cli:") || model.starts_with("gemini-cli:") {
        let prefix = model.split(':').next().unwrap_or(model);
        return Err(format!(
            "model '{model}': {prefix} models run their own tools outside Hone's sandbox, \
             permission rules and working directory, so `hone eval` does not support them"
        ));
    }
    if !PREFIXES.iter().any(|p| model.starts_with(p)) {
        return Err(format!(
            "invalid model ID '{model}': it must start with a provider prefix ({})",
            PREFIXES.join(" ")
        ));
    }
    let choice = ModelChoice {
        model: ModelId::new(model),
        source,
    };
    crate::build_explicit_provider(&choice, eff).map(|_| ())
}

pub async fn run(args: EvalArgs, eff: Effective, model_flag: Option<String>) -> Result<()> {
    // Every agent of the eval is priced per model, as a one-shot run is: the
    // built-in table with the user's `[pricing]` entries on top.
    hone_core::set_default_price_lookup(crate::price_lookup(&eff.pricing));
    let label = args.tasks.display().to_string();
    let raw = match std::fs::read_to_string(&args.tasks) {
        Ok(r) => r,
        Err(e) => fail(&[format!("cannot read {label}: {e}")]),
    };
    let mut tasks = match TasksFile::from_toml_str(&label, &raw) {
        Ok(t) => t,
        Err(e) => fail(&[e.to_string()]),
    };
    let user = match crate::settings::load_user_layer() {
        Ok(u) => u,
        Err(e) => fail(&[crate::settings::format_config_error(&e)]),
    };
    let mut profiles = args.profile.clone();
    if let Some(slot) = &args.ablate {
        // Combined with --profile only to pick the base.
        if profiles.len() > 1 {
            fail(&["--ablate takes at most one --profile (the base profile)".to_owned()]);
        }
        tasks = match tasks.with_ablation_over(
            slot,
            profiles.first().map(String::as_str),
            user.as_ref(),
        ) {
            Ok(t) => t,
            Err(e) => fail(&[e.to_string()]),
        };
        profiles.clear();
    }
    let planned = match tasks.expand(&ExpandOptions {
        profiles: profiles.clone(),
        repeats: args.repeats,
    }) {
        Ok(p) => p,
        Err(e) => fail(&[e.to_string()]),
    };
    let base_dir = args
        .tasks
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let base_dir = std::fs::canonicalize(base_dir).unwrap_or_else(|_| base_dir.to_path_buf());

    // Everything that can be wrong before a model is called, reported together.
    let mut errors = tasks.check_files(&base_dir);
    // An integrity directory must not contain the output directory (F33).
    let out_dir = args
        .output
        .clone()
        .unwrap_or_else(|| PathBuf::from("eval-runs"));
    let out_abs = absolute_lossy(&out_dir);
    for dir in tasks.integrity() {
        let integrity_dir = std::fs::canonicalize(base_dir.join(dir));
        if integrity_dir.is_ok_and(|d| out_abs.starts_with(&d)) {
            errors.push(format!(
                "[eval] integrity: `{dir}` contains the output directory {}; pass a different --output",
                out_abs.display()
            ));
        }
    }
    // Every profile resolved over the user's config, as a run would use it.
    for name in tasks.profile_names() {
        if let Err(e) = tasks.profile_selection(&name, user.as_ref()) {
            errors.push(e.to_string());
        }
    }
    let configured = model_flag
        .clone()
        .map(|m| (m, ModelSource::Cli))
        .or_else(|| {
            tasks.model.clone().map(|m| {
                (
                    m,
                    ModelSource::DefaultModel {
                        path: args.tasks.clone(),
                    },
                )
            })
        })
        .or_else(|| {
            eff.model
                .as_ref()
                .map(|c| (c.model.to_string(), c.source.clone()))
        });
    if let Some((model, source)) = &configured {
        if let Err(e) = check_provider(model, source.clone(), &eff) {
            errors.push(e);
        }
    }
    if configured.is_none() && args.check {
        errors.push(
            "no model: set `model` in the tasks file, pass --model, or set default_model in config.toml"
                .to_owned(),
        );
    }
    if !errors.is_empty() {
        fail(&errors);
    }

    for w in tasks.integrity_warnings(&base_dir) {
        eprintln!("hone eval: warning: {w}");
    }
    let repeats = args.repeats.unwrap_or_else(|| tasks.repeats());
    let profile_count = {
        let mut names: Vec<&str> = planned.iter().map(|p| p.profile.as_str()).collect();
        names.dedup();
        names.sort_unstable();
        names.dedup();
        names.len()
    };
    if args.check {
        println!(
            "ok: {} x {} x {} = {}",
            plural(tasks.tasks.len(), "task"),
            plural(profile_count, "profile"),
            plural(repeats as usize, "repeat"),
            plural(planned.len(), "run"),
        );
        return Ok(());
    }

    let default_model = match &configured {
        Some((m, _)) => m.clone(),
        None => {
            let m = crate::detect_provider(&eff).await.model_id.to_string();
            if let Err(e) = check_provider(&m, ModelSource::Cli, &eff) {
                fail(&[e]);
            }
            m
        }
    };
    let eff = Arc::new(eff);
    let mut runner =
        EvalRunner::with_builder(tasks, base_dir, default_model, cli_builder(eff.clone()));
    if let Some(layer) = user {
        runner = runner.with_user_layer(layer);
    }
    let history = open_history().await;
    let opts = EvalOptions {
        out_dir: args
            .output
            .clone()
            .unwrap_or_else(|| PathBuf::from("eval-runs")),
        profiles,
        repeats: args.repeats,
        jobs: args.jobs,
        keep_workdirs: args.keep_workdirs,
        model: model_flag,
        tag: args.tag.clone(),
        history,
        interrupt: None,
    };
    println!("running {}", plural(planned.len(), "run"));
    let (interrupt_tx, interrupt_rx) = tokio::sync::watch::channel(false);
    let opts = EvalOptions {
        interrupt: Some(interrupt_rx),
        ..opts
    };
    // A spawned task runs on a worker thread, where the builder may block on the
    // runtime (see `build_agent`).
    let mut task = tokio::spawn(async move { runner.run(&opts).await });
    let mut signals = Signals::new()?;
    let mut got: Option<i32> = None;
    let joined = loop {
        let signal = tokio::select! {
            r = &mut task => break r,
            code = signals.next() => code,
        };
        if got.is_some() {
            // A second signal: stop at once, after killing the commands.
            crate::exit_now(signal);
        }
        got = Some(signal);
        eprintln!("hone eval: interrupted; stopping the running agents (signal again to exit now)");
        let _ = interrupt_tx.send(true);
    };
    let run_dir = joined
        .map_err(|e| anyhow::anyhow!("eval task failed: {e}"))?
        .map_err(|e| anyhow::anyhow!("eval failed: {e}"))?;
    print_summary(&run_dir);
    if let Some(code) = got {
        crate::exit_now(code);
    }
    // A violated eval fails visibly, whatever the pass rate (F35).
    if let Some(manifest) = std::fs::read_to_string(run_dir.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .filter(|m| m["integrity_violated"] == true)
    {
        let path = manifest["integrity_first_change"]
            .as_str()
            .unwrap_or("an integrity directory");
        eprintln!(
            "hone eval: the suite changed during the run (first change: {path}); the results can't be trusted"
        );
        crate::exit_now(1);
    }
    // A run in which every outcome is an infrastructure error measured nothing.
    if all_infra_errors(&run_dir) {
        eprintln!("hone eval: every run ended in infra_error; see the outcome.json files");
        crate::exit_now(1);
    }
    Ok(())
}

fn all_infra_errors(run_dir: &Path) -> bool {
    let manifest = std::fs::read_to_string(run_dir.join("manifest.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    let Some(counts) = manifest.as_ref().and_then(|m| m["outcomes"].as_object()) else {
        return false;
    };
    !counts.is_empty() && counts.keys().all(|k| k == "infra_error")
}

/// An absolute path for `p`, resolving symlinks in its longest existing prefix.
fn absolute_lossy(p: &Path) -> PathBuf {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let mut tail = Vec::new();
    let mut head = abs.as_path();
    loop {
        if let Ok(real) = std::fs::canonicalize(head) {
            return tail.iter().rev().fold(real, |acc, c| acc.join(c));
        }
        match (head.parent(), head.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_owned());
                head = parent;
            }
            _ => return abs,
        }
    }
}

/// SIGINT and SIGTERM (Unix); Ctrl+C elsewhere. `next` resolves to the exit code
/// for the signal: 130 or 143.
#[cfg(unix)]
struct Signals {
    int: tokio::signal::unix::Signal,
    term: tokio::signal::unix::Signal,
}

#[cfg(unix)]
impl Signals {
    fn new() -> std::io::Result<Self> {
        use tokio::signal::unix::{SignalKind, signal};
        Ok(Self {
            int: signal(SignalKind::interrupt())?,
            term: signal(SignalKind::terminate())?,
        })
    }

    async fn next(&mut self) -> i32 {
        tokio::select! {
            _ = self.int.recv() => 130,
            _ = self.term.recv() => 143,
        }
    }
}

#[cfg(not(unix))]
struct Signals;

#[cfg(not(unix))]
impl Signals {
    fn new() -> std::io::Result<Self> {
        Ok(Self)
    }

    async fn next(&mut self) -> i32 {
        let _ = tokio::signal::ctrl_c().await;
        130
    }
}

async fn open_history() -> Option<Arc<EvalRunStore>> {
    if std::env::var("HONE_DISABLE_PERSISTENT_STORES").is_ok_and(|v| v == "1") {
        return None;
    }
    let dir = crate::data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("hone eval: warning: no eval history ({e})");
        return None;
    }
    match EvalRunStore::open(&dir.join("eval-history.db")).await {
        Ok(s) => Some(Arc::new(s)),
        Err(e) => {
            eprintln!("hone eval: warning: no eval history ({e})");
            None
        }
    }
}

fn print_summary(run_dir: &Path) {
    println!("run directory: {}", run_dir.display());
    let report = std::fs::read_to_string(run_dir.join("report.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok());
    for p in report
        .as_ref()
        .and_then(|r| r["profiles"].as_array())
        .into_iter()
        .flatten()
    {
        println!(
            "  {}: {}/{} passed ({:.0}%, 95% CI {:.0}%-{:.0}%), mean cost ${:.4}",
            p["profile"].as_str().unwrap_or("?"),
            p["passes"],
            p["runs"],
            p["pass_rate"].as_f64().unwrap_or(0.0) * 100.0,
            p["wilson_low"].as_f64().unwrap_or(0.0) * 100.0,
            p["wilson_high"].as_f64().unwrap_or(0.0) * 100.0,
            p["mean_cost_usd"].as_f64().unwrap_or(0.0),
        );
    }
    println!("report: {}", run_dir.join("report.md").display());
}

/// The builder `hone eval` plugs into the runner.
pub fn cli_builder(eff: Arc<Effective>) -> AgentBuilder {
    Arc::new(move |b: AgentBuild| build_agent(&eff, b))
}

fn build_agent(eff: &Effective, b: AgentBuild) -> std::result::Result<Agent, String> {
    let choice = ModelChoice {
        model: ModelId::new(&b.model),
        source: ModelSource::Cli,
    };
    let detected = crate::build_explicit_provider(&choice, eff)?;
    let workdir = b.workdir.clone();

    // The async parts: the symbol index of the copy and the memory index. Called
    // from a runtime worker thread, so blocking in place is allowed.
    let memory_index_on = b
        .selection
        .sources
        .value
        .iter()
        .any(|n| n == "memory-index");
    let memory = b.memory.clone();
    let handle = tokio::runtime::Handle::current();
    let (index, memory_index) = tokio::task::block_in_place(|| {
        handle.block_on(async {
            let index = match SqliteIndex::in_memory().await {
                Ok(idx) => {
                    if let Err(e) = crate::index_directory_bg(&idx, &workdir).await {
                        tracing::warn!("eval: indexing failed: {e}");
                    }
                    Some(Arc::new(idx))
                }
                Err(e) => {
                    tracing::warn!("eval: no symbol index: {e}");
                    None
                }
            };
            let memory_index = if memory_index_on {
                hone_core::memory_index::render_index(
                    memory.as_ref(),
                    hone_core::memory_index::IndexBudget::default(),
                )
                .await
                .ok()
            } else {
                None
            };
            (index, memory_index)
        })
    });

    let hone_md = HoneMd::load_hierarchical(&workdir);
    let mut sections: Vec<(String, String)> = hone_md
        .sections
        .iter()
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    sections.sort_by(|a, c| a.0.cmp(&c.0));
    let system_prompt =
        crate::system_prompt::build_system_prompt(&sections, memory_index.as_deref());

    let resolved = hone_core::strategy::StrategyRegistry::with_builtins()
        .build(
            &b.selection,
            &hone_core::strategy::StrategyDeps {
                memory_store: Some(b.memory.clone()),
                index: index.clone(),
                working_dir: Some(workdir.clone()),
            },
        )
        .map_err(|e| format!("strategy error: {e}"))?;
    let skeleton = SkeletonStrategy::from_str(&b.selection.skeleton.value).unwrap_or_default();

    // The permission rules of the run's directory: `deny` rules still apply.
    let builtins_env = std::env::var(hone_tools::policy_gate::POLICY_BUILTINS_ENV).ok();
    let policy = crate::policy_load::resolve_permission_policy(
        &workdir,
        hone_tools::policy_gate::default_policy_home(),
        hone_tools::policy_gate::builtins_enabled(builtins_env.as_deref()),
    )
    .map_err(|e| format!("permission policy error: {e}"))?;

    let lsp: Arc<dyn LspBridge> = Arc::new(MockLspBridge::builder().build());
    let config = crate::build_agent_config_at(
        &detected,
        detected.model_id.clone(),
        system_prompt,
        lsp,
        index,
        Some(b.memory.clone()),
        None,
        Vec::new(),
        resolved.has_source("skeleton"),
        skeleton,
        false,
        b.task.budget_usd.or(eff.session_limit_usd),
        workdir,
        policy,
    );
    Ok(Agent::new(config).with_strategies(Arc::new(resolved)))
}
