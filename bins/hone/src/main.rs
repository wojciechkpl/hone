mod approve;
mod configure;
mod image_input;
mod memory_cmd;
mod memory_init;
mod policy_load;
mod report;
mod reports_cmd;
mod session;
mod settings;
mod spec_cmd;
mod system_prompt;
mod tools;
mod trace;

use std::io;
use std::io::IsTerminal as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use hone_cache::{SqliteBackend, ToolResultCache};
use hone_config::resolve::{
    Effective, ModelChoice, ModelSource, Tier, check_model_switch, turn_model,
};
use hone_config::{HoneMd, McpConfig};
use hone_core::{
    AdaptiveClassifier, AgentConfig, AgentEnsemble, AgentHandle, AgentMode, AgentRegistry,
    ClarificationDetector, ComplexityClassifier, ComplexityTier, Orchestrator, OutcomeRecord,
    PlanModeStore, PlanStore, SelfRefineLoop, SkeletonStrategy, TestVerificationLoop,
};
use hone_index::{IndexWatcher, SqliteIndex, WatcherHandle};
use hone_mcp::McpManager;
use hone_memory::{MemoryStore, SqliteStore};
use hone_proto::{AgentEvent, Content, Message, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::{
    ClaudeCliProvider, GeminiCliProvider, ModelProvider, adapters::anthropic::AnthropicProvider,
    adapters::ollama::OllamaProvider, adapters::openai::OpenAiProvider,
};
use hone_eval::{EvalConfig, EvalReport, EvalRunner};
use hone_recipes::Recipe;
use hone_sandbox::SandboxBuilder;
use hone_tools::{
    ComputerControlTool, EditFileTool, GitCommitTool, GitDiffTool, GitStatusTool, GlobTool,
    GrepTool, Language, LspBridge, LspCodeActionTool, LspDiagnosticsTool, LspHoverTool,
    MockLspBridge, MultiEditTool, ReadFileTool, ScreenCaptureTool, ShellTool, StdioLspBridge,
    ToolRegistry, WebFetchTool, WebSearchTool, WriteFileTool,
};
use hone_tui::render::root;
use hone_tui::{App, AppState};
use hone_tui::{execute_shell_escape, run_interactive_blocking};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// CLI argument parsing via clap
// ---------------------------------------------------------------------------

/// Subcommands for `hone session <sub>`.
#[derive(Subcommand, Debug)]
enum SessionCmd {
    /// List all saved sessions (name, date, message count).
    List,
    /// Resume a previous session by name.
    Resume {
        /// Name of the session to resume.
        name: String,
    },
    /// Rename a session.
    Rename {
        /// Current session name.
        old: String,
        /// New session name.
        new: String,
    },
    /// Delete a session permanently.
    Delete {
        /// Name of the session to delete.
        name: String,
    },
}

/// Top-level subcommands (optional — the default behaviour is the agent loop).
#[derive(Subcommand, Debug)]
enum HoneCmd {
    /// Manage named sessions.
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },
    /// Interactive setup wizard — writes ~/.config/hone/config.toml.
    Configure,
    /// Inspect hierarchical tracing data from the egress audit log.
    Trace {
        #[command(subcommand)]
        cmd: trace::TraceCmd,
    },
    /// List, show, and diff session and task reports.
    Reports {
        #[command(subcommand)]
        cmd: reports_cmd::ReportsCmd,
    },
    /// Inspect and prune the persistent memory store.
    Memory {
        #[command(subcommand)]
        cmd: memory_cmd::MemoryCmd,
    },
    /// Run evaluation tasks from a TOML file and produce a report.
    Eval {
        /// Path to the tasks.toml file.
        tasks: PathBuf,
        /// Output directory for report.json and report.md (default: parent dir of tasks file).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Generate a Kiro-style spec (requirements + design + tasks) for a goal.
    Spec {
        /// Natural-language goal describing what to build.
        goal: String,
        /// Project root (default: current working directory).
        #[arg(short, long)]
        cwd: Option<PathBuf>,
    },
}

#[derive(Parser)]
#[command(
    name = "hone",
    version,
    about = "The universal terminal AI coding agent"
)]
struct Cli {
    /// Run a single prompt and exit (one-shot mode)
    prompt: Option<String>,

    /// Model ID to use (e.g. anthropic:claude-sonnet-4-5)
    #[arg(short, long)]
    model: Option<String>,

    /// Run a named built-in recipe
    #[arg(long)]
    recipe: Option<String>,

    /// List all available built-in recipes
    #[arg(long)]
    list_recipes: bool,

    /// Run prompt through the multi-agent orchestrator
    #[arg(long)]
    orchestrate: bool,

    /// Use a named agent persona (e.g. code-reviewer, debugger, security-auditor)
    #[arg(long)]
    agent: Option<String>,

    /// Ephemeral session (no memory persistence)
    #[arg(long)]
    ephemeral: bool,

    /// Encrypt the memory store at rest using AES-256-GCM.
    /// Requires the HONE_MEMORY_PASSPHRASE environment variable to be set.
    #[arg(long)]
    encrypted_memory: bool,

    /// Enable vim-style keybindings in the TUI (i/a=insert, Esc=normal, j/k=scroll)
    #[arg(long)]
    vim: bool,

    /// Run N parallel candidates with varied temperatures and pick the best
    /// response using the MASAI Ranker pattern (requires N >= 2)
    #[arg(long, value_name = "N")]
    ensemble: Option<u32>,

    /// Attach a name to this session (auto-saved after each turn).
    #[arg(long, value_name = "NAME")]
    session: Option<String>,

    /// Disable proactive context injection (enabled by default; set HONE_NO_INJECTION=1 to disable).
    /// Context injection pre-loads a repo skeleton and relevant code signatures
    /// into the agent's context before the first model call.
    #[arg(long)]
    no_inject: bool,

    /// Skeleton generation strategy for context injection.
    ///
    /// Choices: adaptive (default), call-graph, hybrid, dependency-graph, flat.
    /// Also controlled via the HONE_SKELETON_STRATEGY env var.
    #[arg(long, value_name = "STRATEGY")]
    skeleton_strategy: Option<String>,

    /// Output structured JSON instead of streaming text (for CI/CD and scripting).
    /// Includes response, tool calls, usage, cost, and exit status.
    #[arg(long)]
    json: bool,

    /// Substitute a variable in a recipe prompt. Repeatable.
    /// Format: KEY=VALUE (e.g. --var feature_name=my_feature --var test_command="cargo test")
    #[arg(long = "var", value_name = "KEY=VALUE")]
    vars: Vec<String>,

    /// Pre-approve a permission rule by id so its `ask` decisions run without
    /// a prompt (repeatable). Never overrides a `deny`.
    #[arg(long, value_name = "RULE_ID")]
    approve: Vec<String>,

    /// Enable the egress audit log (writes to .hone/audit.jsonl).
    /// Also enabled by setting HONE_AUDIT_LOG=1.
    #[arg(long)]
    audit: bool,

    /// Skip scope clarification entirely (useful for scripting/CI).
    /// By default, ambiguous one-shot prompts may trigger clarifying questions
    /// before the agent begins executing.
    #[arg(long)]
    no_clarify: bool,

    /// Enable step-through debugger: pause before each sequential tool call.
    /// Press Space/Enter to proceed, s to skip, Esc to proceed.
    #[arg(long, default_value_t = false)]
    step: bool,

    /// Execute a saved plan file (plan_body is loaded as the prompt and run in
    /// Build mode). The file must be a SavedPlan written by plan mode.
    #[arg(long, value_name = "FILE")]
    plan: Option<PathBuf>,

    /// Attach one or more image files to the initial prompt. Supported formats:
    /// png, jpg/jpeg, gif, webp. Repeatable for multi-image prompts.
    #[arg(long, value_name = "FILE")]
    image: Vec<PathBuf>,

    /// Coordinator URL for distributed orchestration (e.g. http://10.0.0.5:3117).
    /// When set, --orchestrate dispatches per-worker tasks through a remote
    /// honed coordinator instead of running them in-process. Requires
    /// HONE_AUTH_TOKEN to be set to the coordinator's bearer token.
    #[arg(long, value_name = "URL")]
    distributed: Option<String>,

    /// Subcommand (session management, configure wizard, etc.)
    #[command(subcommand)]
    command: Option<HoneCmd>,
}

// ---------------------------------------------------------------------------
// Built-in recipe loading
// ---------------------------------------------------------------------------

fn load_builtin_recipe(name: &str) -> Option<&'static str> {
    match name {
        "tdd-feature" => Some(include_str!("../recipes/tdd-feature.yaml")),
        "bug-fix" => Some(include_str!("../recipes/bug-fix.yaml")),
        "code-review" => Some(include_str!("../recipes/code-review.yaml")),
        "refactor" => Some(include_str!("../recipes/refactor.yaml")),
        "security-audit" => Some(include_str!("../recipes/security-audit.yaml")),
        "performance" => Some(include_str!("../recipes/performance-optimization.yaml")),
        "migration" => Some(include_str!("../recipes/migration.yaml")),
        "documentation" => Some(include_str!("../recipes/documentation.yaml")),
        "onboarding" => Some(include_str!("../recipes/onboarding.yaml")),
        _ => None,
    }
}

fn list_builtin_recipes() {
    println!("Available recipes:");
    println!(
        "  {:<20} Implement a feature using Test-Driven Development",
        "tdd-feature"
    );
    println!("  {:<20} Fix a bug with regression test", "bug-fix");
    println!(
        "  {:<20} Comprehensive code review (correctness, security, performance)",
        "code-review"
    );
    println!(
        "  {:<20} Safely refactor with test verification at each step",
        "refactor"
    );
    println!(
        "  {:<20} Full OWASP Top 10 security audit",
        "security-audit"
    );
    println!(
        "  {:<20} Profile, analyze, and optimize performance",
        "performance"
    );
    println!(
        "  {:<20} Migrate between frameworks, libraries, or API versions",
        "migration"
    );
    println!(
        "  {:<20} Generate or update project documentation",
        "documentation"
    );
    println!(
        "  {:<20} Understand a new codebase (architecture, patterns, entry points)",
        "onboarding"
    );
}

// ---------------------------------------------------------------------------
// Recipe execution mode
// ---------------------------------------------------------------------------

async fn run_recipe(
    agent: AgentHandle,
    recipe_name: &str,
    vars: &std::collections::HashMap<String, String>,
) -> Result<()> {
    let yaml = load_builtin_recipe(recipe_name)
        .ok_or_else(|| anyhow::anyhow!("unknown recipe: {recipe_name}"))?;

    let recipe = Recipe::from_yaml(yaml)?.with_variables(vars);

    println!(
        "Running recipe: {} ({} steps)",
        recipe.name,
        recipe.steps.len()
    );
    if let Some(desc) = &recipe.description {
        println!("  {desc}");
    }
    println!();

    for (i, step) in recipe.steps.iter().enumerate() {
        if step.when == Some(false) {
            println!(
                "[{}/{}] Skipping: {} (condition false)",
                i + 1,
                recipe.steps.len(),
                step.name
            );
            continue;
        }

        println!("[{}/{}] {}", i + 1, recipe.steps.len(), step.name);

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        // Spawn a printer that drains events and writes to stdout.
        let printer = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    AgentEvent::Token { text } => {
                        print!("{text}");
                        use io::Write;
                        let _ = io::stdout().flush();
                    }
                    AgentEvent::ToolCallStart { name, .. } => {
                        println!("\n  [tool: {name}]");
                    }
                    AgentEvent::Done { .. } => break,
                    AgentEvent::Error { message } => {
                        eprintln!("\n  error: {message}");
                        break;
                    }
                    _ => {}
                }
            }
        });

        let prompt = step.prompt.clone();
        if let Err(e) = agent.run_turn(&prompt, tx).await {
            eprintln!("  Step failed: {e}");
        }
        let _ = printer.await;
        println!("\n");

        // Run test command if configured.
        if let Some(test_cmd) = &recipe.test_cmd {
            println!("  Running tests: {test_cmd}");
            let output = tokio::process::Command::new("sh")
                .arg("-c")
                .arg(test_cmd)
                .output()
                .await?;

            if output.status.success() {
                println!("  Tests passed.");
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                println!("  Tests FAILED:\n{stderr}");
            }
            println!();
        }
    }

    println!("Recipe '{}' complete.", recipe.name);
    Ok(())
}

// ---------------------------------------------------------------------------
// Color output helpers
// ---------------------------------------------------------------------------

/// Returns `true` when ANSI color escape codes should be emitted to stderr.
///
/// Respects:
/// - `NO_COLOR` env var (https://no-color.org) — any value disables color
/// - Whether stderr is an interactive terminal (disabled when piped/redirected)
fn should_color_stderr() -> bool {
    std::env::var("NO_COLOR").is_err() && io::stderr().is_terminal()
}

/// Wrap `text` with ANSI bold-yellow escape codes when color is enabled.
fn warn_style(text: &str) -> String {
    if should_color_stderr() {
        format!("\x1b[1;33m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Wrap `text` with ANSI bold-red escape codes when color is enabled.
fn error_style(text: &str) -> String {
    if should_color_stderr() {
        format!("\x1b[1;31m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

/// Wrap `text` with ANSI bold escape codes when color is enabled.
fn bold_style(text: &str) -> String {
    if should_color_stderr() {
        format!("\x1b[1m{text}\x1b[0m")
    } else {
        text.to_owned()
    }
}

// ---------------------------------------------------------------------------
// Provider detection
// ---------------------------------------------------------------------------

struct DetectedProvider {
    provider: Arc<dyn ModelProvider>,
    model_id: ModelId,
    /// Whether this provider supports tool calling.
    supports_tools: bool,
}

async fn binary_on_path(name: &str) -> bool {
    tokio::process::Command::new("which")
        .arg(name)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn anthropic_provider(eff: &Effective, key: String) -> AnthropicProvider {
    match &eff.anthropic.base_url {
        Some(url) => AnthropicProvider::with_base_url(key, url.clone()),
        None => AnthropicProvider::new(key),
    }
}

fn openai_provider(eff: &Effective, key: String) -> OpenAiProvider {
    match &eff.openai.base_url {
        Some(url) => OpenAiProvider::with_base_url(key, url.clone()),
        None => OpenAiProvider::new(key),
    }
}

fn ollama_base_url(eff: &Effective) -> String {
    eff.ollama
        .base_url
        .as_deref()
        .unwrap_or("http://localhost:11434")
        .trim_end_matches('/')
        .to_owned()
}

/// Build the provider an explicit model choice names (all five prefixes).
///
/// A choice that cannot be built (no key) is an error naming where the choice
/// came from and how to supply the key; it never falls back to detection.
fn build_explicit_provider(
    choice: &ModelChoice,
    eff: &Effective,
) -> Result<DetectedProvider, String> {
    let m = choice.model.as_str();
    let source = match &choice.source {
        ModelSource::Cli => "--model".to_owned(),
        ModelSource::DefaultModel { path } => format!("default_model in {}", path.display()),
        ModelSource::Wizard { path } => format!("[provider] in {}", path.display()),
    };
    let missing_key = |settings: &hone_config::resolve::ProviderSettings| {
        let var = settings.api_key_env.as_deref().unwrap_or("the API key variable");
        format!(
            "model '{m}' (from {source}) needs an API key: set {var} \
             (or run `hone configure`)"
        )
    };
    let detected = |provider: Arc<dyn ModelProvider>, supports_tools: bool| DetectedProvider {
        provider,
        model_id: choice.model.clone(),
        supports_tools,
    };

    if m.starts_with("ollama:") {
        tracing::info!("explicit ollama: model — using the Ollama provider directly");
        Ok(detected(
            Arc::new(OllamaProvider::new(ollama_base_url(eff))),
            true,
        ))
    } else if m.starts_with("anthropic:") {
        let key = eff
            .anthropic
            .api_key
            .clone()
            .ok_or_else(|| missing_key(&eff.anthropic))?;
        Ok(detected(Arc::new(anthropic_provider(eff, key)), true))
    } else if m.starts_with("openai:") {
        let key = eff
            .openai
            .api_key
            .clone()
            .ok_or_else(|| missing_key(&eff.openai))?;
        Ok(detected(Arc::new(openai_provider(eff, key)), true))
    } else if m.starts_with("claude-cli:") {
        Ok(detected(Arc::new(ClaudeCliProvider::new()), false))
    } else if m.starts_with("gemini-cli:") {
        Ok(detected(Arc::new(GeminiCliProvider::new()), false))
    } else {
        Err(format!("invalid model ID '{m}' (from {source})"))
    }
}

/// The API provider `hone eval` / `hone spec` use: Anthropic, else OpenAI, from
/// the resolved settings.
fn api_provider_from_settings(eff: &Effective, command: &str) -> Result<Arc<dyn ModelProvider>> {
    if let Some(key) = eff.anthropic.api_key.clone() {
        Ok(Arc::new(anthropic_provider(eff, key)))
    } else if let Some(key) = eff.openai.api_key.clone() {
        Ok(Arc::new(openai_provider(eff, key)))
    } else {
        anyhow::bail!(
            "{command} requires a provider: set {} or {}",
            eff.anthropic.api_key_env.as_deref().unwrap_or("ANTHROPIC_API_KEY"),
            eff.openai.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY"),
        )
    }
}

fn is_api_model(model: &ModelId) -> bool {
    matches!(model.provider(), Some("anthropic" | "openai" | "ollama"))
}

fn tier_of(t: ComplexityTier) -> Tier {
    match t {
        ComplexityTier::Simple => Tier::Simple,
        ComplexityTier::Medium => Tier::Medium,
        ComplexityTier::Complex => Tier::Complex,
    }
}

/// Everything the TUI needs to route turns and validate `/model`.
struct TuiRouting {
    eff: Effective,
    dispatch: Arc<hone_providers::ModelDispatch>,
    /// `--model`: exactly that model, no routing.
    pinned: Option<ModelId>,
}

/// Build the provider set for a session on an API model: Anthropic and OpenAI
/// when they have a key, Ollama always, with `fallback_models` (not after
/// `--model`). The session's own provider is reused as is.
///
/// Routed or fallback models whose provider is missing get one warning each and
/// are dropped.
fn build_model_dispatch(
    eff: &Effective,
    detected: &DetectedProvider,
    pinned: bool,
) -> Arc<hone_providers::ModelDispatch> {
    let session_prefix = detected.model_id.provider().unwrap_or_default().to_owned();
    let mut dispatch = hone_providers::ModelDispatch::new(detected.model_id.clone())
        .with_provider(&session_prefix, Arc::clone(&detected.provider));
    if session_prefix != "anthropic" {
        if let Some(key) = eff.anthropic.api_key.clone() {
            dispatch = dispatch.with_provider("anthropic", Arc::new(anthropic_provider(eff, key)));
        }
    }
    if session_prefix != "openai" {
        if let Some(key) = eff.openai.api_key.clone() {
            dispatch = dispatch.with_provider("openai", Arc::new(openai_provider(eff, key)));
        }
    }
    if session_prefix != "ollama" {
        dispatch = dispatch.with_provider(
            "ollama",
            Arc::new(OllamaProvider::new(ollama_base_url(eff))),
        );
    }
    if pinned {
        return Arc::new(dispatch);
    }

    let missing = |model: &ModelId| -> String {
        let prefix = model.provider().unwrap_or_default();
        let var = match prefix {
            "anthropic" => eff
                .anthropic
                .api_key_env
                .as_deref()
                .unwrap_or("ANTHROPIC_API_KEY"),
            "openai" => eff
                .openai
                .api_key_env
                .as_deref()
                .unwrap_or("OPENAI_API_KEY"),
            _ => "the provider's settings",
        };
        format!("no {prefix} provider is configured (set {var})")
    };
    for tier in [Tier::Simple, Tier::Medium, Tier::Complex] {
        if let Some(r) = eff.routing.get(tier) {
            if dispatch.has_provider_for(&r.model) {
                // A repository chose this model: say so, as for its default_model.
                if settings::is_project_config_path(&r.path) {
                    eprintln!(
                        "hone: routing {} requests to {} (from {})",
                        tier.as_str(),
                        r.model,
                        r.path.display()
                    );
                }
            } else {
                eprintln!(
                    "hone: warning: {}: [[routing]] {} -> {} is not used: {}; using the default model",
                    r.path.display(),
                    tier.as_str(),
                    r.model,
                    missing(&r.model)
                );
            }
        }
    }
    let mut fallback = Vec::new();
    for m in &eff.fallback_models {
        if dispatch.has_provider_for(m) {
            fallback.push(m.clone());
        } else if let Some(path) = &eff.fallback_path {
            eprintln!(
                "hone: warning: {}: fallback_models {m} is dropped: {}",
                path.display(),
                missing(m)
            );
        }
    }
    Arc::new(dispatch.with_fallback(fallback))
}

/// `[[routing]]` and `fallback_models` do nothing with an agent CLI model.
fn warn_routing_not_applied(eff: &Effective) {
    if !eff.routing.is_empty() {
        eprintln!("hone: warning: [[routing]] is not applied with an agent CLI model");
    }
    if !eff.fallback_models.is_empty() {
        eprintln!("hone: warning: fallback_models is not applied with an agent CLI model");
    }
}

async fn detect_provider(eff: &Effective) -> DetectedProvider {
    // Prefer direct Anthropic API — supports tool calling.
    if let Some(api_key) = eff.anthropic.api_key.clone() {
        // Empty keys never get here: the resolver drops them.
        let var = eff.anthropic.api_key_env.as_deref().unwrap_or("ANTHROPIC_API_KEY");
        if !api_key.starts_with("sk-ant-") {
            eprintln!(
                "{} {var} doesn't start with 'sk-ant-'. Is this correct?",
                warn_style("Warning:")
            );
        }
        tracing::info!(
            provider = "anthropic",
            api = "http",
            tools = true,
            "provider detected"
        );
        return DetectedProvider {
            provider: Arc::new(anthropic_provider(eff, api_key)),
            model_id: ModelId::new(format!("anthropic:{}", eff.anthropic.default_model)),
            supports_tools: true,
        };
    }

    // OpenAI API (or any OpenAI-compatible endpoint) — supports tool calling.
    // Set HONE_OPENAI_BASE_URL (or [providers.openai].base_url in the user
    // config) to point at DeepSeek, Qwen, Kimi, OpenRouter, etc.
    if let Some(api_key) = eff.openai.api_key.clone() {
        tracing::info!(
            provider = "openai",
            base_url = ?eff.openai.base_url,
            tools = true,
            "provider detected"
        );
        return DetectedProvider {
            provider: Arc::new(openai_provider(eff, api_key)),
            model_id: ModelId::new(format!("openai:{}", eff.openai.default_model)),
            supports_tools: true,
        };
    }

    // Fall back to CLI pass-through providers (no tool calling).
    if binary_on_path("claude").await {
        tracing::info!(
            provider = "claude-cli",
            api = "cli",
            tools = false,
            "provider detected"
        );
        eprintln!(
            "{} Using Claude CLI (no tool calling). \
             For full agent capabilities, set ANTHROPIC_API_KEY instead.",
            warn_style("Note:")
        );
        return DetectedProvider {
            provider: Arc::new(ClaudeCliProvider::new()),
            model_id: ModelId::new("claude-cli:claude-sonnet-4-5"),
            supports_tools: false,
        };
    }

    if binary_on_path("gemini").await {
        tracing::info!(
            provider = "gemini-cli",
            api = "cli",
            tools = false,
            "provider detected"
        );
        eprintln!(
            "{} Using Gemini CLI (no tool calling). \
             For full agent capabilities, set OPENAI_API_KEY or ANTHROPIC_API_KEY instead.",
            warn_style("Note:")
        );
        return DetectedProvider {
            provider: Arc::new(GeminiCliProvider::new()),
            model_id: ModelId::new("gemini-cli:gemini-2.5-pro"),
            supports_tools: false,
        };
    }

    // Check if Ollama is running locally (no tool use, but real local inference).
    let ollama_url = ollama_base_url(eff);
    if reqwest::Client::new()
        .get(format!("{ollama_url}/api/tags"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok()
    {
        let ollama_model = eff.ollama.default_model.as_str();

        // Verify that the default model is actually pulled before committing
        // to Ollama as the provider. A 200 from /api/show means the model
        // exists; any other response (404, connection error) means it is absent.
        let model_present = reqwest::Client::new()
            .post(format!("{ollama_url}/api/show"))
            .json(&serde_json::json!({ "name": ollama_model }))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);

        if !model_present {
            eprintln!(
                "{} Ollama is running but model '{}' is not installed.",
                warn_style("Note:"),
                ollama_model
            );
            eprintln!("Run:  ollama pull {ollama_model}");
            eprintln!("Then re-run hone.\n");
            // Fall through to the "no provider" error below.
        } else {
            tracing::info!(
                provider = "ollama",
                api = "http",
                tools = true,
                "provider detected (tool calling enabled since Ollama v0.3.0)"
            );
            return DetectedProvider {
                provider: Arc::new(OllamaProvider::new(ollama_url.clone())),
                model_id: ModelId::new(format!("ollama:{ollama_model}")),
                supports_tools: true,
            };
        }
    }

    tracing::warn!("no AI provider found");
    eprintln!(
        "\n{} Hone checked the following (in order):\n",
        warn_style("No AI provider detected.")
    );
    eprintln!(
        "  1. {}    — ANTHROPIC_API_KEY not set",
        bold_style("Anthropic API")
    );
    eprintln!("                         export ANTHROPIC_API_KEY=sk-ant-...");
    eprintln!(
        "  2. {}        — OPENAI_API_KEY not set",
        bold_style("OpenAI API")
    );
    eprintln!("                         export OPENAI_API_KEY=sk-...");
    eprintln!(
        "                         Tip: set HONE_OPENAI_BASE_URL for DeepSeek/Qwen/Kimi/OpenRouter"
    );
    eprintln!(
        "  3. {}        — 'claude' not found on PATH",
        bold_style("Claude CLI")
    );
    eprintln!("                         Install: https://claude.ai/download");
    eprintln!(
        "  4. {}        — 'gemini' not found on PATH",
        bold_style("Gemini CLI")
    );
    eprintln!("                         Install: https://github.com/google-gemini/gemini-cli");
    eprintln!(
        "  5. {}            — localhost:11434 not responding",
        bold_style("Ollama")
    );
    eprintln!("                         Install: https://ollama.com  then: ollama serve");
    eprintln!();
    eprintln!("Set one of the above and re-run hone.\n");
    std::process::exit(1);
}

// ---------------------------------------------------------------------------
// Tool registry
// ---------------------------------------------------------------------------

/// Detect the primary language of the current project from well-known manifest
/// files in the working directory.
fn detect_project_language() -> Language {
    if std::path::Path::new("Cargo.toml").exists() {
        Language::Rust
    } else if std::path::Path::new("pyproject.toml").exists()
        || std::path::Path::new("requirements.txt").exists()
    {
        Language::Python
    } else if std::path::Path::new("package.json").exists() {
        Language::TypeScript
    } else if std::path::Path::new("go.mod").exists() {
        Language::Go
    } else {
        Language::Unknown
    }
}

/// Try to spawn a real LSP bridge for the project language.  Falls back to
/// `MockLspBridge` when the language server binary is not installed, so tools
/// are still registered but return empty results.
async fn detect_lsp_bridge() -> Arc<dyn LspBridge> {
    let lang = detect_project_language();
    if lang != Language::Unknown {
        match StdioLspBridge::for_language(lang).await {
            Ok(bridge) => {
                tracing::info!("LSP: spawned {:?} server", lang);
                return Arc::new(bridge);
            }
            Err(e) => {
                tracing::warn!("LSP: failed to spawn {:?} server: {e} — using mock", lang);
            }
        }
    }
    Arc::new(MockLspBridge::builder().build())
}

/// Build a `ToolRegistry` with all available native tools plus the three LSP
/// tools backed by `lsp`, and optionally the symbol index tools.
///
/// MCP tools are registered first (lower priority) so that native tools
/// overwrite any name collisions — native tools always take precedence.
///
/// When `cache` is `Some`, read-only tools are wrapped with
/// [`CachingToolWrapper`] so that repeated identical calls are served from the
/// SQLite-backed tool-result cache.  Write tools, shell, and git_commit are
/// registered unwrapped — the wrapper's `is_cacheable` guard would bypass the
/// cache anyway, but wrapping them is unnecessary overhead.
fn build_tool_registry(
    lsp: Arc<dyn LspBridge>,
    index: Option<Arc<SqliteIndex>>,
    memory: Option<Arc<dyn MemoryStore>>,
    cache: Option<Arc<ToolResultCache>>,
    mcp_tools: Vec<Box<dyn hone_tools::Tool>>,
) -> ToolRegistry {
    // Wrap a tool with the cache layer when caching is enabled.
    // The wrapper is a transparent pass-through for non-cacheable tools, but
    // we only wrap tools that are in the cacheable set to keep the hot path
    // clean for write tools.
    let wrap = |tool: Box<dyn hone_tools::Tool>| -> Box<dyn hone_tools::Tool> {
        match &cache {
            Some(c) => Box::new(tools::CachingToolWrapper::new(
                Arc::from(tool),
                Arc::clone(c),
            )),
            None => tool,
        }
    };

    // Register MCP tools first so that native tools can overwrite any name
    // collisions (last registration wins in ToolRegistryBuilder).
    let mut builder = ToolRegistry::builder();
    for mcp_tool in mcp_tools {
        builder = builder.register(mcp_tool);
    }

    builder = builder
        // Cacheable read-only tools — wrapped when cache is enabled.
        .register(wrap(Box::new(ReadFileTool)))
        .register(wrap(Box::new(GlobTool)))
        .register(wrap(Box::new(GrepTool)))
        .register(wrap(Box::new(GitStatusTool)))
        .register(wrap(Box::new(GitDiffTool)))
        .register(wrap(Box::new(LspDiagnosticsTool::new(Arc::clone(&lsp)))))
        .register(wrap(Box::new(LspHoverTool::new(Arc::clone(&lsp)))))
        .register(wrap(Box::new(LspCodeActionTool::new(Arc::clone(&lsp)))))
        // Web tools — read-only network egress. WebSearch is Disabled unless
        // HONE_WEBSEARCH_PROVIDER + HONE_WEBSEARCH_KEY are set; WebFetch is
        // always live (uses reqwest with 15s timeout).
        .register(wrap(Box::new(WebSearchTool::from_env())))
        .register(wrap(Box::new(WebFetchTool::new())))
        // Write / mutating tools — never cached.
        .register(Box::new(WriteFileTool))
        .register(Box::new(EditFileTool))
        .register(Box::new(MultiEditTool))
        .register(Box::new(ShellTool))
        .register(Box::new(GitCommitTool))
        .register(Box::new(ScreenCaptureTool))
        .register(Box::new(ComputerControlTool));

    if let Some(idx) = index {
        builder = builder
            .register(wrap(Box::new(tools::SymbolSearchTool::new(Arc::clone(
                &idx,
            )))))
            .register(wrap(Box::new(tools::SymbolAtTool::new(idx))));
    }

    if let Some(store) = memory {
        builder = builder.register(Box::new(tools::MemoryReadTool::new(store)));
    }

    builder.build()
}

// ---------------------------------------------------------------------------
// Config and HONE.md loading
// ---------------------------------------------------------------------------

/// Load HONE.md sections from the current directory upwards.
///
/// `config.toml` is read separately by [`settings::load_effective_or_exit`].
fn load_hone_md() -> Option<HoneMd> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let merged = HoneMd::load_hierarchical(&cwd);
    if merged.sections.is_empty() {
        None
    } else {
        Some(merged)
    }
}

// ---------------------------------------------------------------------------
// Agent construction
// ---------------------------------------------------------------------------

/// Load the permission policy (user + project layers) for agent runs.
///
/// A broken policy file is fatal: print the (already sanitised) error and
/// exit with code 2 rather than run without the rules the user wrote.
fn load_permission_policy(working_dir: &std::path::Path) -> Option<hone_core::PolicyConfig> {
    let home = hone_tools::policy_gate::default_policy_home();
    // Read once at startup: the built-in rules are opt-in.
    let env_value = std::env::var(hone_tools::policy_gate::POLICY_BUILTINS_ENV).ok();
    if let Some(w) = hone_tools::policy_gate::builtins_env_warning(env_value.as_deref()) {
        eprintln!("hone: warning: {w}");
    }
    let builtins = hone_tools::policy_gate::builtins_enabled(env_value.as_deref());
    match policy_load::resolve_permission_policy(working_dir, home, builtins) {
        Ok(policy) => policy,
        Err(e) => {
            eprintln!("hone: permission policy error: {e}");
            std::process::exit(2);
        }
    }
}

/// Apply `--approve` ids and the provider-gating check to the loaded policy.
///
/// Exits with code 2 on an unknown id, `--approve` without a policy, or a
/// provider that runs its own tools under a non-empty policy.
fn apply_policy_flags(config: &mut AgentConfig, approve: &[String]) {
    let fail = |msg: String| -> ! {
        eprintln!("hone: {msg}");
        std::process::exit(2);
    };
    match approve::validate_approvals(config.policy.as_ref(), approve) {
        Ok(ids) => {
            if let Some(policy) = config.policy.as_mut() {
                policy.pre_approved = ids;
            }
        }
        Err(msg) => fail(msg),
    }
    let allow = std::env::var("HONE_ALLOW_UNGATED_PROVIDER").ok();
    match approve::check_provider_gating(
        config.policy.as_ref(),
        config.provider.runs_own_tools(),
        allow.as_deref(),
    ) {
        Ok(Some(warning)) => eprintln!("hone: warning: {warning}"),
        Ok(None) => {}
        Err(msg) => {
            let id = config.provider.id();
            fail(msg.replacen("this provider ", &format!("this provider ({id}) "), 1));
        }
    }
}

/// Build an `AgentConfig` from the detected provider + model + system prompt.
///
/// The returned config is `Clone`; callers may clone it freely to spawn
/// orchestrators, ensembles, or additional agents without rebuilding.
fn build_agent_config(
    detected: &DetectedProvider,
    model_id: ModelId,
    system_prompt: String,
    lsp: Arc<dyn LspBridge>,
    index: Option<Arc<SqliteIndex>>,
    memory_store: Option<Arc<dyn MemoryStore>>,
    tool_cache: Option<Arc<ToolResultCache>>,
    mcp_tools: Vec<Box<dyn hone_tools::Tool>>,
    no_inject_flag: bool,
    skeleton_strategy_arg: Option<&str>,
    audit_flag: bool,
    session_limit_usd: Option<f64>,
) -> AgentConfig {
    // Only register tools when the provider can actually use them.
    let tools = if detected.supports_tools {
        build_tool_registry(
            lsp,
            index.clone(),
            memory_store.clone(),
            tool_cache,
            mcp_tools,
        )
    } else {
        ToolRegistry::new()
    };

    // Proactive context injection is ENABLED by default. Its effect on resolve
    // rate is being measured by the pre-registered ablation in
    // benchmarks/harness-compare. Disable with --no-inject or HONE_NO_INJECTION=1.
    let inject_context = !no_inject_flag
        && std::env::var("HONE_NO_INJECTION")
            .map(|v| v != "1")
            .unwrap_or(true);

    // Skeleton strategy: CLI arg wins over env var; env var is read inside
    // SkeletonStrategy::from_str fallback via ContextInjector::new(). We
    // resolve explicitly here so the value is stored in AgentConfig.
    let skeleton_strategy = skeleton_strategy_arg
        .and_then(SkeletonStrategy::from_str)
        .or_else(|| {
            std::env::var("HONE_SKELETON_STRATEGY")
                .ok()
                .and_then(|v| SkeletonStrategy::from_str(&v))
        })
        .unwrap_or_default();

    // Egress audit log: enabled via --audit or HONE_AUDIT_LOG=1.
    let enable_audit = audit_flag
        || std::env::var("HONE_AUDIT_LOG")
            .map(|v| v == "1")
            .unwrap_or(false);
    let egress_log = if enable_audit {
        let working_dir = std::env::current_dir().unwrap_or_default();
        let log_path = working_dir.join(".hone").join("audit.jsonl");
        Some(Arc::new(tokio::sync::Mutex::new(
            hone_core::EgressLog::new(log_path),
        )))
    } else {
        None
    };

    // One source of truth for the directory: tools and policy share it.
    let agent_working_dir = std::env::current_dir().unwrap_or_default();
    let policy = load_permission_policy(&agent_working_dir);

    AgentConfig {
        provider: Arc::clone(&detected.provider),
        tools,
        sandbox: Arc::from(SandboxBuilder::detect()),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt,
        model: model_id.clone(),
        max_tokens: None,
        temperature: None,
        budget_limit: session_limit_usd,
        pricing: PricingEntry {
            model: model_id.clone(),
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: agent_working_dir,
        memory_store,
        retry_config: hone_core::RetryConfig::default(),
        index: index.map(|idx| Arc::clone(&idx)),
        inject_context,
        skeleton_strategy,
        egress_log,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy,
    }
}

// ---------------------------------------------------------------------------
// Test-command detection
// ---------------------------------------------------------------------------

/// Return the test command appropriate for the current working directory, or
/// `None` when no recognisable build system is found.
fn detect_test_cmd() -> Option<String> {
    if std::path::Path::new("Cargo.toml").exists() {
        return Some("cargo test".to_string());
    }
    if std::path::Path::new("package.json").exists() {
        return Some("npm test".to_string());
    }
    if std::path::Path::new("Makefile").exists() {
        return Some("make test".to_string());
    }
    if std::path::Path::new("pyproject.toml").exists() {
        return Some("pytest".to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// Test verification helper — runs after any code-modifying turn
// ---------------------------------------------------------------------------

/// Run the test-verification loop if a test command is detectable and the
/// current turn actually called a code-modifying tool.
///
/// Only `write_file`, `edit_file`, and `multi_edit` are considered
/// code-modifying.  Pure Q&A turns (no tool calls, or only read/shell calls)
/// are intentionally skipped so tests don't run after every message.
async fn maybe_run_test_verification(
    handle: &AgentHandle,
    tx: &mpsc::Sender<AgentEvent>,
    tools_used: &[String],
) {
    const CODE_MODIFYING: &[&str] = &["write_file", "edit_file", "multi_edit"];

    let touched_code = tools_used
        .iter()
        .any(|name| CODE_MODIFYING.contains(&name.as_str()));

    if !touched_code {
        tracing::debug!(
            tools = ?tools_used,
            "test verification skipped (no code-modifying tool used)"
        );
        return;
    }

    let Some(test_cmd) = detect_test_cmd() else {
        tracing::debug!("test verification skipped (no test command detected)");
        return;
    };

    tracing::info!(
        tools = ?tools_used,
        cmd = %test_cmd,
        "test verification triggered"
    );

    let cwd = std::env::current_dir().unwrap_or_default();
    let test_loop = TestVerificationLoop::new(test_cmd, 3, cwd);

    match test_loop.run_with_handle(handle, tx.clone()).await {
        Ok(result) => match result.final_status {
            hone_core::TestStatus::Passed => {
                tracing::info!(
                    iterations = result.iterations,
                    "tests passed after {} iteration(s)",
                    result.iterations
                );
            }
            hone_core::TestStatus::FailedAfterMax { .. } => {
                tracing::warn!(
                    iterations = result.iterations,
                    "test verification gave up after {} iteration(s)",
                    result.iterations
                );
            }
            hone_core::TestStatus::NoTestCommand => {}
        },
        Err(e) => {
            tracing::warn!("test verification failed: {e}");
        }
    }
}

/// Names (in order of completion) of code-modifying calls that actually ran:
/// a non-error `ToolCallEnd` whose result is not `{"skipped": true}`, matched
/// to its `ToolCallStart` by id. Denied, skipped and failed calls do not count,
/// so a refused write never triggers the automatic test run.
#[cfg(test)]
fn code_modifying_tools_that_ran(events: &[AgentEvent]) -> Vec<String> {
    let mut tracker = RanTracker::default();
    for e in events {
        tracker.observe(e);
    }
    tracker.ran
}

/// Incremental form of [`code_modifying_tools_that_ran`] for the turn
/// forwarders: it keeps only `(id, name)` of code-modifying starts and the
/// names that ran, never whole events or tool results.
#[derive(Default)]
struct RanTracker {
    started: std::collections::HashMap<String, String>,
    ran: Vec<String>,
}

impl RanTracker {
    fn observe(&mut self, event: &AgentEvent) {
        const CODE_MODIFYING: &[&str] = &["write_file", "edit_file", "multi_edit"];
        match event {
            AgentEvent::ToolCallStart { id, name, .. }
                if CODE_MODIFYING.contains(&name.as_str()) =>
            {
                self.started.insert(id.clone(), name.clone());
            }
            AgentEvent::ToolCallEnd {
                id,
                result,
                is_error,
                ..
            } => {
                let skipped = result.get("skipped").and_then(|v| v.as_bool()) == Some(true);
                if let Some(name) = self.started.remove(id) {
                    if !*is_error && !skipped {
                        self.ran.push(name);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Aborts a task when dropped (also when the owning task is aborted).
struct AbortOnDrop(tokio::task::AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Forward `inner_rx` to `outer_tx` for an inline `@agent` turn, then run
/// `after` (prompt/tool restore) when the inner stream ends normally.
///
/// If the wrapper is aborted or dropped, or the outer receiver goes away, the
/// `inner` turn task is aborted (drop guard) so a pending approval prompt
/// cannot outlive the UI that could answer it. `after` does not run then.
fn spawn_inline_turn_wrapper<F>(
    inner: tokio::task::JoinHandle<()>,
    mut inner_rx: mpsc::Receiver<AgentEvent>,
    outer_tx: mpsc::Sender<AgentEvent>,
    after: F,
) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let _guard = AbortOnDrop(inner.abort_handle());
        loop {
            tokio::select! {
                biased;
                () = outer_tx.closed() => return,
                ev = inner_rx.recv() => match ev {
                    Some(ev) => {
                        if outer_tx.send(ev).await.is_err() {
                            return;
                        }
                    }
                    None => break,
                },
            }
        }
        after.await;
    })
}

// ---------------------------------------------------------------------------
// Agent turn dispatch — simple (direct)
// ---------------------------------------------------------------------------

fn spawn_agent_turn(
    agent: AgentHandle,
    user_text: String,
    tx: mpsc::Sender<AgentEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Use an intermediate channel so we can observe events before
        // forwarding them to the caller and track which tools ran.
        let (inner_tx, mut inner_rx) = mpsc::channel::<AgentEvent>(256);

        let agent_clone = agent.clone();
        let turn_task =
            tokio::spawn(async move { agent_clone.run_turn(&user_text, inner_tx).await });
        // Aborting this task (Esc) must release the actor: dropping `inner_rx`
        // makes the turn cancel, and the guard aborts the request task too.
        let _guard = AbortOnDrop(turn_task.abort_handle());

        let mut tracker = RanTracker::default();
        let mut terminal_forwarded = false;

        loop {
            tokio::select! {
                biased;
                // The UI went away: stop forwarding so the actor is released.
                () = tx.closed() => return,
                ev = inner_rx.recv() => {
                    let Some(event) = ev else { break };
                    tracker.observe(&event);
                    let is_terminal =
                        matches!(event, AgentEvent::Done { .. } | AgentEvent::Error { .. });
                    if tx.send(event).await.is_err() {
                        return;
                    }
                    if is_terminal {
                        terminal_forwarded = true;
                        break;
                    }
                }
            }
        }

        // Errors the turn forwarded are already visible. If the stream ended
        // without a terminal event (turn error, dead actor, panicked task) the
        // UI must still hear about it, or its spinner never stops.
        let joined = turn_task.await;
        if !terminal_forwarded {
            let reason = match joined {
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
                Ok(Ok(_)) => "the turn ended without a result".to_owned(),
            };
            let _ = tx
                .send(AgentEvent::Error {
                    message: format!("agent stopped unexpectedly: {reason}"),
                })
                .await;
            return;
        }

        // After the turn completes, run test verification if applicable.
        maybe_run_test_verification(&agent, &tx, &tracker.ran).await;
    })
}

// ---------------------------------------------------------------------------
// Agent turn dispatch — medium (agent + self-refine)
// ---------------------------------------------------------------------------

fn spawn_medium_turn(
    agent: AgentHandle,
    user_text: String,
    tx: mpsc::Sender<AgentEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        // Use an intermediate channel to observe events and track tool usage.
        let (inner_tx, mut inner_rx) = mpsc::channel::<AgentEvent>(256);

        let agent_clone = agent.clone();
        let user_text_clone = user_text.clone();
        let refine_task = tokio::spawn(async move {
            let refine = SelfRefineLoop::new(2);
            refine
                .run_with_handle(&agent_clone, &user_text_clone, inner_tx)
                .await
        });
        let _guard = AbortOnDrop(refine_task.abort_handle());

        let mut tracker = RanTracker::default();
        let mut had_error = false;
        // The refine loop runs several turns on this channel, each ending in
        // a `Done`: round 0 is the answer, then critique and refine rounds
        // alternate. Forward tool, pause, approval and error events of every
        // round, but drop critique `Token`s (they are the model talking to
        // itself) and mark where the revised answer starts. Intermediate
        // `Done`s are swallowed; exactly one terminal `Done` (usage and cost
        // summed over all rounds) is sent at the end.
        const REVISED_SEPARATOR: &str = "\n\n---\n\n*Revised answer:*\n\n";
        let mut round: usize = 0;
        let mut separator_sent = false;
        let mut total_usage = hone_proto::TokenUsage::default();
        let mut total_cost = 0.0_f64;
        let mut last_plan = None;
        let mut saw_done = false;

        loop {
            tokio::select! {
                biased;
                // The UI went away: stop forwarding so the actor is released.
                () = tx.closed() => return,
                ev = inner_rx.recv() => {
                    let Some(event) = ev else { break };
                    match event {
                        AgentEvent::Done { usage, cost_usd, plan } => {
                            total_usage = total_usage + usage;
                            total_cost += cost_usd;
                            if plan.is_some() {
                                last_plan = plan;
                            }
                            saw_done = true;
                            round += 1;
                            separator_sent = false;
                        }
                        AgentEvent::Token { text } => {
                            let is_critique = round % 2 == 1;
                            let is_refine = round >= 2 && round % 2 == 0;
                            if is_critique {
                                continue;
                            }
                            if is_refine && !separator_sent {
                                separator_sent = true;
                                let sep = AgentEvent::Token { text: REVISED_SEPARATOR.to_owned() };
                                if tx.send(sep).await.is_err() {
                                    return;
                                }
                            }
                            if tx.send(AgentEvent::Token { text }).await.is_err() {
                                return;
                            }
                        }
                        other => {
                            tracker.observe(&other);
                            if matches!(other, AgentEvent::Error { .. }) {
                                had_error = true;
                            }
                            if tx.send(other).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        }

        // Surface any error that the refine task returned but wasn't forwarded.
        let refine_result = refine_task.await;
        let failure = match refine_result {
            Ok(Ok(_)) => None,
            // No Error event was forwarded, so the actor died under this turn.
            Ok(Err(e)) => Some(format!("agent stopped unexpectedly: {e}")),
            // The task panicked: the actor is gone and later turns will be
            // Cancelled, so say so instead of leaving the spinner running.
            Err(e) => Some(format!("agent stopped unexpectedly: {e}")),
        };
        if let Some(message) = failure {
            if !had_error {
                let _ = tx.send(AgentEvent::Error { message }).await;
            }
            return;
        }

        if had_error {
            return;
        }

        if saw_done {
            let done = AgentEvent::Done {
                usage: total_usage,
                cost_usd: total_cost,
                plan: last_plan,
            };
            if tx.send(done).await.is_err() {
                return;
            }
        }

        // After the self-refine turn completes, run test verification.
        // This replaces the previous unconditional detect_test_cmd call and
        // respects the code-modifying tool guard, so Q&A turns don't trigger
        // an unnecessary test run. Only calls that actually ran count.
        maybe_run_test_verification(&agent, &tx, &tracker.ran).await;
    })
}

// ---------------------------------------------------------------------------
// Agent turn dispatch — complex (orchestrator as spawned task)
// ---------------------------------------------------------------------------

/// The run config of an orchestrated turn under an inline `@agent` persona:
/// `config` with its tools narrowed to the persona's `tools:` list, the same
/// narrowing the persona's own turn gets. An empty list leaves it unchanged.
fn narrow_run_config_for_persona(mut config: AgentConfig, allowed: &[String]) -> AgentConfig {
    if !allowed.is_empty() {
        config.tools = hone_core::filtered_registry(&config.tools, allowed);
    }
    config
}

fn spawn_orchestrated_turn(
    lead_config: AgentConfig,
    user_text: String,
    tx: mpsc::Sender<AgentEvent>,
    agent: AgentHandle,
    plan: Option<hone_core::TaskPlan>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut lead_config = lead_config;
        // A stopped agent fails closed: no lead or worker call is made.
        let Some(settings) = agent.run_settings().await else {
            let _ = tx
                .send(AgentEvent::Error {
                    message: "orchestration failed: the agent has stopped".to_owned(),
                })
                .await;
            return;
        };
        // Workers run under the agent's current approval policy and mode
        // ("always" answers, /permit, /mode). Read here, not in the UI loop,
        // so the UI never waits on the actor. One round trip through the actor
        // (the run waits if it is busy) gives a consistent snapshot.
        lead_config.approval_policy = settings.approval_policy;
        lead_config.mode = settings.mode;

        let registry = AgentRegistry::load_hierarchical(&lead_config.working_dir);

        let plan_store = PlanStore::default_for(&lead_config.working_dir);
        // No per-worker timeout: a worker may wait on an approval or a long
        // build. Esc (or `x` in the approval menu) stops the run.
        // The session's "always" answers cover every worker, and the
        // workers' answers cover the session agent in turn.
        let mut orchestrator = Orchestrator::new(lead_config, registry, 4)
            .with_plan_store(plan_store)
            .without_worker_timeout()
            .with_session_approvals(settings.session_approvals);

        // The orchestrator's lead and worker turns each end in a `Done` (or an
        // `Error`) on this channel. Those belong to the inner turns: swallow
        // them, sum the `Done`s, and send exactly one terminal event at the end
        // so the TUI stays busy until the whole run is over.
        let (inner_tx, mut inner_rx) = mpsc::channel::<AgentEvent>(256);
        let run_text = user_text.clone();
        let run_task = tokio::spawn(async move {
            match plan {
                Some(plan) => orchestrator.run_plan(plan, inner_tx).await,
                None => orchestrator.run(&run_text, inner_tx).await,
            }
        });
        // Dropped when this task is aborted (Esc), which stops the run too.
        let _guard = AbortOnDrop(run_task.abort_handle());

        let mut total_usage = hone_proto::TokenUsage::default();
        let mut total_cost = 0.0_f64;
        loop {
            tokio::select! {
                biased;
                // The UI went away: stop forwarding.
                () = tx.closed() => return,
                ev = inner_rx.recv() => {
                    let Some(event) = ev else { break };
                    match event {
                        AgentEvent::Done { usage, cost_usd, .. } => {
                            total_usage = total_usage + usage;
                            total_cost += cost_usd;
                        }
                        AgentEvent::Error { .. } => {}
                        other => {
                            if tx.send(other).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        }

        let outcome = match run_task.await {
            Ok(outcome) => outcome,
            Err(e) => {
                let _ = tx
                    .send(AgentEvent::Error {
                        message: format!("orchestration failed: {e}"),
                    })
                    .await;
                return;
            }
        };
        match outcome {
            Ok(result) => {
                if result.merged_response.is_empty()
                    || result.merged_response.starts_with("Merge failed:")
                {
                    tracing::warn!(
                        "skipping orchestrated history writeback: merged_response is empty or merge-failure"
                    );
                } else {
                    // Note: load_history queues an actor command via mpsc::Sender; if the spawning
                    // task is aborted before the actor processes the command, the write-back may
                    // be lost. This is acceptable today because abort during orchestration is rare
                    // and the bounded channel (capacity 256) almost always has room. Upgrading
                    // load_history to use a oneshot ack would close the race fully.
                    agent
                        .load_history(vec![
                            Message::user(user_text.as_str()),
                            Message::assistant(result.merged_response.as_str()),
                        ])
                        .await;
                }
                let summary = format!(
                    "\n\n--- Orchestration complete ---\n{}\n\nTotal cost: ${:.6} | workers: {}",
                    result.merged_response,
                    result.total_cost_usd,
                    result.workers.len()
                );
                let _ = tx.send(AgentEvent::Token { text: summary }).await;
                let _ = tx
                    .send(AgentEvent::Done {
                        usage: total_usage,
                        cost_usd: total_cost,
                        plan: None,
                    })
                    .await;
            }
            Err(e) => {
                let _ = tx
                    .send(AgentEvent::Error {
                        message: format!("orchestration failed: {e}"),
                    })
                    .await;
            }
        }
    })
}

// ---------------------------------------------------------------------------
// /plan dispatch helper — calls Orchestrator::decompose once, returns the
// TaskPlan + lead-LLM cost + token usage so the TUI can render the
// plan-preview overlay without spawning any workers.
// ---------------------------------------------------------------------------

async fn dispatch_plan_decompose(
    lead_config: AgentConfig,
    working_dir: std::path::PathBuf,
    prompt: &str,
) -> Result<(hone_core::TaskPlan, f64, hone_proto::TokenUsage)> {
    let registry = AgentRegistry::load_hierarchical(&working_dir);
    let mut orchestrator = Orchestrator::new(lead_config, registry, 4);
    // The decompose path emits its own AgentEvents on this channel; we
    // discard them since the overlay only needs the final triple.
    let (events_tx, mut events_rx) = mpsc::channel::<AgentEvent>(64);
    let drain = tokio::spawn(async move {
        while events_rx.recv().await.is_some() {
            // intentionally swallowed
        }
    });
    let result = orchestrator.decompose(prompt, events_tx).await;
    drop(orchestrator); // release events_tx so the drainer can exit
    drain.await.ok();
    result.map_err(|e| anyhow::anyhow!("decompose: {e}"))
}

// ---------------------------------------------------------------------------
// Orchestrate mode (blocking, for the --orchestrate one-shot path)
// ---------------------------------------------------------------------------

/// Print `--orchestrate` progress until the channel closes.
///
/// Must keep draining after an `Error`: dropping the receiver would make every
/// later worker and the merge turn fail with `Cancelled`.
async fn print_orchestrate_events<O: io::Write, E: io::Write>(
    mut rx: mpsc::Receiver<AgentEvent>,
    out: &mut O,
    err: &mut E,
) {
    let mut fallbacks = FallbackPrinter::default();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Token { text } => {
                fallbacks.wrote(&text);
                let _ = write!(out, "{text}");
                let _ = out.flush();
            }
            AgentEvent::ModelFallback {
                requested,
                fallback,
                reason,
            } => fallbacks.event(err, &requested, &fallback, &reason),
            AgentEvent::PlanCreated {
                goal,
                subtask_count,
            } => {
                let _ = writeln!(
                    out,
                    "\n[orchestrator] plan: {goal} ({subtask_count} subtasks)"
                );
                fallbacks.wrote("\n");
            }
            AgentEvent::WorkerStarted {
                task_id,
                agent,
                description,
            } => {
                let _ = writeln!(out, "[{task_id}] starting {agent}: {description}");
                fallbacks.wrote("\n");
            }
            AgentEvent::WorkerCompleted {
                task_id,
                agent,
                status,
                cost_usd,
            } => {
                let _ = writeln!(out, "[{task_id}] {agent}: {status} (${cost_usd:.4})");
                fallbacks.wrote("\n");
            }
            AgentEvent::Error { message } => {
                let _ = writeln!(err, "\nerror: {message}");
            }
            _ => {}
        }
    }
}

async fn run_orchestrate(
    lead_config: AgentConfig,
    prompt: &str,
    distributed: Option<&str>,
) -> Result<()> {
    let registry = AgentRegistry::load_hierarchical(&lead_config.working_dir);

    let plan_store = PlanStore::default_for(&lead_config.working_dir);
    let mut orchestrator = Orchestrator::new(lead_config, registry, 4).with_plan_store(plan_store);

    // P2-4 Phase 2b/2c: when --distributed COORDINATOR_URL is set, route
    // per-task dispatch through a remote honed coordinator's /distributed/*
    // endpoints instead of running every worker in-process.  Auth token
    // comes from HONE_AUTH_TOKEN.
    if let Some(coordinator_url) = distributed {
        let token = std::env::var("HONE_AUTH_TOKEN").unwrap_or_default();
        if token.is_empty() {
            tracing::warn!(
                "--distributed set but HONE_AUTH_TOKEN env is empty; coordinator may reject requests"
            );
        }
        let client = std::sync::Arc::new(hone_core::queue_client::QueueClient::new(
            coordinator_url.to_string(),
            token,
        ));
        orchestrator = orchestrator.with_queue_client(client);
        tracing::info!("distributed orchestrator enabled, coordinator at {coordinator_url}");
    }

    let (tx, rx) = mpsc::channel::<AgentEvent>(256);

    let printer = tokio::spawn(async move {
        let mut out = io::stdout();
        let mut err = io::stderr();
        print_orchestrate_events(rx, &mut out, &mut err).await;
    });

    match orchestrator.run(prompt, tx).await {
        Ok(result) => {
            let _ = printer.await;
            println!("\n\n--- Final Response ---");
            println!("{}", result.merged_response);
            println!(
                "\nTotal cost: ${:.6} | workers: {}",
                result.total_cost_usd,
                result.workers.len()
            );
        }
        Err(e) => {
            let _ = printer.await;
            eprintln!("Orchestration failed: {e}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Ensemble mode
// ---------------------------------------------------------------------------

/// Print `--ensemble` progress until the channel closes.
///
/// Tokens and the first `Error` are printed only until the first `Done` or
/// `Error`; after that the channel is still read to its end, ignoring tokens
/// and errors, so later candidates' fallback notices are printed.
async fn print_ensemble_events<O: io::Write, E: io::Write>(
    mut rx: mpsc::Receiver<AgentEvent>,
    out: &mut O,
    err: &mut E,
) {
    let mut fallbacks = FallbackPrinter::default();
    let mut printing = true;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Token { text } if printing => {
                fallbacks.wrote(&text);
                let _ = write!(out, "{text}");
                let _ = out.flush();
            }
            AgentEvent::ModelFallback {
                requested,
                fallback,
                reason,
            } => fallbacks.event(err, &requested, &fallback, &reason),
            AgentEvent::Done { .. } => printing = false,
            AgentEvent::Error { message } if printing => {
                let _ = writeln!(err, "\nerror: {message}");
                printing = false;
            }
            _ => {}
        }
    }
}

/// Run a prompt through N parallel candidates with varied temperatures, then
/// let a ranker agent pick the best response (MASAI Ranker pattern).
async fn run_ensemble(base_config: &AgentConfig, prompt: &str, n: u32) -> Result<()> {
    // Temperature ladder: diverse but bounded.  We only use as many slots as
    // requested, so `--ensemble 2` picks the first two entries.
    const TEMPS: [f32; 5] = [0.0, 0.3, 0.5, 0.7, 0.9];

    let n = (n as usize).min(TEMPS.len());
    let temperatures: Vec<f32> = TEMPS[..n].to_vec();

    // AgentEnsemble clones the prototype once per candidate (applying the
    // per-slot temperature) and once more for the ranker (at 0.0).  All share
    // the same system prompt; the ranker prompt is assembled by the ensemble
    // from the candidate responses, so the system prompt here just sets the
    // overall assistant persona.
    let ensemble = AgentEnsemble::new(base_config.clone(), temperatures);

    let (tx, rx) = mpsc::channel::<AgentEvent>(256);

    let printer = tokio::spawn(async move {
        print_ensemble_events(rx, &mut io::stdout(), &mut io::stderr()).await;
    });

    let result = ensemble.run(prompt, tx).await?;
    let _ = printer.await;

    println!(
        "\n\n[Ensemble: picked candidate {}/{} (${:.4} total)]\n\n{}",
        result.winner_index + 1,
        result.all_candidates.len(),
        result.total_cost_usd,
        result.winner.response,
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// One-shot mode
// ---------------------------------------------------------------------------

/// Finish a one-shot turn. A failed turn has already printed its `Error`
/// event (the printer returns `true`), so exit non-zero without letting
/// `main` print the same error a second time.
async fn finish_one_shot(
    result: Result<hone_core::TurnResult, hone_core::AgentError>,
    printer: tokio::task::JoinHandle<bool>,
) -> Result<()> {
    let error_printed = printer.await.unwrap_or(false);
    match result {
        Ok(_) => {
            println!();
            Ok(())
        }
        Err(_) if error_printed => {
            println!();
            Err(AlreadyReported.into())
        }
        Err(e) => Err(e.into()),
    }
}

async fn run_one_shot(agent: AgentHandle, prompt: &str) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

    // Drain events and print tokens while the agent runs inline.
    let printer = tokio::spawn(async move {
        let mut fallbacks = FallbackPrinter::default();
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Token { text } => {
                    fallbacks.wrote(&text);
                    print!("{text}");
                    use io::Write;
                    let _ = io::stdout().flush();
                }
                AgentEvent::ModelFallback {
                    requested,
                    fallback,
                    reason,
                } => fallbacks.event(&mut io::stderr(), &requested, &fallback, &reason),
                AgentEvent::Done { .. } => break,
                AgentEvent::Error { message } => {
                    eprintln!("\nerror: {message}");
                    return true;
                }
                _ => {}
            }
        }
        false
    });

    let result = agent.run_turn(prompt, tx).await;
    finish_one_shot(result, printer).await
}

/// Run a one-shot multimodal turn (text + images) and stream tokens to stdout.
async fn run_one_shot_content(agent: AgentHandle, content: Content) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

    let printer = tokio::spawn(async move {
        let mut fallbacks = FallbackPrinter::default();
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Token { text } => {
                    fallbacks.wrote(&text);
                    print!("{text}");
                    use io::Write;
                    let _ = io::stdout().flush();
                }
                AgentEvent::ModelFallback {
                    requested,
                    fallback,
                    reason,
                } => fallbacks.event(&mut io::stderr(), &requested, &fallback, &reason),
                AgentEvent::Done { .. } => break,
                AgentEvent::Error { message } => {
                    eprintln!("\nerror: {message}");
                    return true;
                }
                _ => {}
            }
        }
        false
    });

    let result = agent.run_turn_content(content, tx).await;
    finish_one_shot(result, printer).await
}

/// Run a one-shot prompt and output structured JSON.
///
/// Output format:
/// ```json
/// {
///   "response": "the agent's final text response",
///   "tool_calls": [{"name": "read_file", "result": "..."}],
///   "usage": {"input": 1234, "output": 567, "cache_read": 0},
///   "cost_usd": 0.0034,
///   "model": "openai:deepseek/deepseek-chat",
///   "success": true,
///   "error": null,
///   "fallbacks": [{"requested": "...", "fallback": "...", "reason": "...", "calls": 1}]
/// }
/// ```
async fn run_json_mode(agent: AgentHandle, prompt: &str, model_id: &str) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

    let collector = tokio::spawn(async move {
        let mut response = String::new();
        let mut tool_calls: Vec<serde_json::Value> = Vec::new();
        let mut error: Option<String> = None;
        let mut current_tool: Option<String> = None;
        let mut fallbacks = FallbackPrinter::default();

        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Token { text } => {
                    response.push_str(&text);
                }
                AgentEvent::ModelFallback {
                    requested,
                    fallback,
                    reason,
                } => fallbacks.event(&mut io::stderr(), &requested, &fallback, &reason),
                AgentEvent::ToolCallStart { name, .. } => {
                    current_tool = Some(name);
                }
                AgentEvent::ToolCallEnd {
                    result, is_error, ..
                } => {
                    let tool_name = current_tool.take().unwrap_or_else(|| "unknown".to_string());
                    tool_calls.push(serde_json::json!({
                        "name": tool_name,
                        "result": result.to_string(),
                        "is_error": is_error,
                    }));
                }
                AgentEvent::Done { .. } => break,
                AgentEvent::Error { message } => {
                    error = Some(message);
                    break;
                }
                _ => {}
            }
        }

        (response, tool_calls, error, fallbacks.log)
    });

    let turn_result = agent.run_turn(prompt, tx).await;
    let (response, tool_calls, error, fallbacks) = collector.await.unwrap_or_default();
    // One fresh agent per run, so the session's spend is this run's spend,
    // including model calls of a turn that failed or was cancelled.
    let spend = agent.session_spend().await;

    let output = json_mode_output(
        &turn_result,
        response,
        tool_calls,
        error,
        spend,
        model_id,
        &fallbacks,
    );
    println!("{}", serde_json::to_string_pretty(&output)?);

    if output["success"] != true {
        return Err(AlreadyReported.into());
    }

    Ok(())
}

/// Build the `--json` report. `usage` and `cost_usd` come from the session
/// spend whatever the outcome, so a failed run still reports what it cost.
fn json_mode_output(
    turn: &Result<hone_core::TurnResult, hone_core::AgentError>,
    response: String,
    tool_calls: Vec<serde_json::Value>,
    error: Option<String>,
    spend: (hone_proto::TokenUsage, f64),
    model_id: &str,
    fallbacks: &FallbackLog,
) -> serde_json::Value {
    let (usage, cost_usd) = spend;
    serde_json::json!({
        "response": response,
        "tool_calls": tool_calls,
        "usage": {
            "input": usage.input,
            "output": usage.output,
            "cache_read": usage.cache_read,
        },
        "cost_usd": cost_usd,
        "model": model_id,
        "success": turn.is_ok() && error.is_none(),
        "error": error,
        "fallbacks": fallbacks.to_json(),
    })
}

/// The failure was already reported to the user (printed error event or JSON
/// report). `main` maps it to exit code 1 after MCP servers shut down.
#[derive(Debug)]
struct AlreadyReported;

impl std::fmt::Display for AlreadyReported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("failure already reported")
    }
}

impl std::error::Error for AlreadyReported {}

// ---------------------------------------------------------------------------
// Session auto-save
// ---------------------------------------------------------------------------

/// Snapshot the agent's context and persist it to the session store.
///
/// When `name` is `None` a default name like `"session-YYYY-MM-DD-HHMM"` is
/// generated.  Errors are logged as warnings and never propagate to the caller.
async fn auto_save_session(agent: &AgentHandle, name: Option<&str>) {
    let messages = agent.get_messages().await;
    if messages.is_empty() {
        return;
    }

    let Some(store) = open_session_store().await else {
        return;
    };

    let resolved_name = name
        .map(str::to_owned)
        .unwrap_or_else(session::default_session_name);

    if let Err(e) = store.save_session(&resolved_name, &messages).await {
        tracing::warn!("auto-save session failed: {e}");
    } else {
        tracing::info!(name = %resolved_name, messages = messages.len(), "session auto-saved");
    }
}

// ---------------------------------------------------------------------------
// Session management helpers
// ---------------------------------------------------------------------------

/// Return the path to the sessions database (`~/.local/share/hone/sessions.db`).
fn sessions_db_path() -> PathBuf {
    data_dir().join("sessions.db")
}

/// Open the session store, returning `None` (with a warning) on failure.
async fn open_session_store() -> Option<session::SessionStore> {
    let path = sessions_db_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("{} could not create data dir: {e}", warn_style("Warning:"));
            return None;
        }
    }
    match session::SessionStore::open(&path).await {
        Ok(store) => Some(store),
        Err(e) => {
            eprintln!("{} could not open sessions db: {e}", warn_style("Warning:"));
            None
        }
    }
}

/// Handle `hone session list`.
async fn cmd_session_list() -> Result<()> {
    let Some(store) = open_session_store().await else {
        return Ok(());
    };
    let sessions = store.list_sessions().await?;
    if sessions.is_empty() {
        println!("No saved sessions.  Start one with: hone --session my-session 'prompt'");
        return Ok(());
    }

    let name_width = sessions
        .iter()
        .map(|s| s.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    println!(
        "{:<name_width$}  {:<20}  {:>5}",
        "NAME",
        "UPDATED",
        "MSGS",
        name_width = name_width
    );
    println!("{}", "-".repeat(name_width + 2 + 20 + 2 + 5));
    for s in &sessions {
        println!(
            "{:<name_width$}  {:<20}  {:>5}",
            s.name,
            session::format_timestamp(s.updated_at),
            s.message_count,
            name_width = name_width,
        );
    }
    Ok(())
}

/// Handle `hone session rename <old> <new>`.
async fn cmd_session_rename(old: &str, new: &str) -> Result<()> {
    let Some(store) = open_session_store().await else {
        return Ok(());
    };
    store.rename_session(old, new).await?;
    println!("Session '{old}' renamed to '{new}'.");
    Ok(())
}

/// Handle `hone session delete <name>`.
async fn cmd_session_delete(name: &str) -> Result<()> {
    let Some(store) = open_session_store().await else {
        return Ok(());
    };
    store.delete_session(name).await?;
    println!("Session '{name}' deleted.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Eval subcommand
// ---------------------------------------------------------------------------

/// Run evaluation tasks from a TOML file and write report.json + report.md.
///
/// Provider support in v1: Anthropic and OpenAI(-compatible), built from the
/// resolved settings (environment, user `config.toml`, wizard key). Other
/// providers (Ollama, CLI pass-throughs) are a follow-up.
async fn cmd_eval(
    tasks_path: &std::path::Path,
    output_dir: Option<PathBuf>,
    eff: &Effective,
) -> Result<()> {
    use std::path::Path;

    let config = EvalConfig::from_toml_file(tasks_path)
        .map_err(|e| anyhow::anyhow!("failed to load tasks file: {e}"))?;

    let base_dir = tasks_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();

    let out_dir = output_dir.unwrap_or_else(|| base_dir.clone());

    std::fs::create_dir_all(&out_dir)
        .map_err(|e| anyhow::anyhow!("cannot create output dir {}: {e}", out_dir.display()))?;

    // The factory must be 'static: build one provider from the resolved
    // settings and hand out clones of the Arc.
    let provider = api_provider_from_settings(eff, "eval")?;
    let provider_factory: Box<dyn Fn() -> Arc<dyn ModelProvider> + Send + Sync> =
        Box::new(move || Arc::clone(&provider));

    let outcomes = EvalRunner::new(config, base_dir, provider_factory)
        .run_all()
        .await;

    let report = EvalReport::from_outcomes(outcomes);

    hone_eval::report::write_json(&report, &out_dir.join("report.json"))
        .map_err(|e| anyhow::anyhow!("failed to write report.json: {e}"))?;
    hone_eval::report::write_markdown(&report, &out_dir.join("report.md"))
        .map_err(|e| anyhow::anyhow!("failed to write report.md: {e}"))?;

    let pass_pct = report.pass_rate() * 100.0;
    println!(
        "{}/{} passed ({:.0}%) — total cost ${:.4} — see {}/report.md for details",
        report.passed,
        report.total,
        pass_pct,
        report.total_cost_usd,
        out_dir.display(),
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Spec subcommand
// ---------------------------------------------------------------------------

async fn cmd_spec_run(
    goal: &str,
    cwd: Option<PathBuf>,
    eff: &Effective,
) -> Result<()> {
    use spec_cmd::{SpecCommandError, SpecGeneratorImpl, cmd_spec};

    let project_root = match cwd {
        Some(p) => p,
        None => std::env::current_dir()?,
    };

    let provider = api_provider_from_settings(eff, "spec")?;

    let model = hone_proto::ModelId::new("anthropic:claude-3-5-sonnet-20241022");
    let generator = SpecGeneratorImpl::new(provider, model);

    match cmd_spec(goal, &project_root, &generator).await {
        Ok(out) => {
            println!("Spec created: {}", out.spec_dir.display());
            println!("  slug:   {}", out.slug);
            println!("  goal:   {}", out.goal);
            println!("  tasks:  {}", out.task_count);
            Ok(())
        }
        Err(SpecCommandError::AlreadyExists(path)) => {
            anyhow::bail!(
                "spec already exists at {}; remove it or use a different goal",
                path.display()
            );
        }
        Err(e) => Err(anyhow::anyhow!("{}", e)),
    }
}

// ---------------------------------------------------------------------------
// SQLite persistence initialisation
// ---------------------------------------------------------------------------

/// Return `~/.local/share/hone` (or `/tmp/hone` as a fallback).
fn data_dir() -> PathBuf {
    std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".local/share/hone"))
        .unwrap_or_else(|| PathBuf::from("/tmp/hone"))
}

/// Return type for `init_persistence`.
struct PersistenceHandles {
    /// The opened memory store.
    memory: Option<Arc<dyn MemoryStore>>,
    /// The opened symbol index, shared with the tool registry.
    index: Option<Arc<SqliteIndex>>,
    /// Keeps the file-watcher task alive.  Must be held for the process lifetime.
    _watcher: Option<WatcherHandle>,
}

/// Initialise the memory and index SQLite stores.
///
/// Background indexing of the current directory is kicked off as a detached
/// tokio task so it never blocks startup.  A file watcher is also started so
/// that subsequent edits are incrementally re-indexed.
///
/// Returns `PersistenceHandles` with whatever stores opened successfully.
/// Any errors are printed to stderr (as non-fatal warnings) so that a
/// persistence failure never prevents the binary from starting.
async fn init_persistence(
    ephemeral: bool,
    memory_passphrase: Option<String>,
) -> PersistenceHandles {
    if ephemeral {
        tracing::info!("ephemeral mode: skipping persistence init");
        return PersistenceHandles {
            memory: None,
            index: None,
            _watcher: None,
        };
    }

    let dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("failed to create data dir {}: {e}", dir.display());
        return PersistenceHandles {
            memory: None,
            index: None,
            _watcher: None,
        };
    }

    // Memory store
    let memory_path = dir.join("memory.db");
    let memory: Option<Arc<dyn MemoryStore>> = match SqliteStore::open(&memory_path).await {
        Ok(store) => {
            tracing::info!(path = %memory_path.display(), "memory store initialised");
            let passphrase_ref = memory_passphrase.as_deref();
            match memory_init::build_memory_store(store, passphrase_ref).await {
                Ok(wrapped) => Some(wrapped),
                Err(e) => {
                    tracing::warn!("failed to initialise memory store: {e}");
                    if passphrase_ref.is_some() {
                        eprintln!(
                            "error: failed to open encrypted memory store: {e}\n\
                             Check that HONE_MEMORY_PASSPHRASE is correct."
                        );
                        std::process::exit(1);
                    } else {
                        eprintln!(
                            "Failed to initialise memory store: {e}\n\
                             Hone will run without memory persistence."
                        );
                        None
                    }
                }
            }
        }
        Err(e) => {
            tracing::warn!("failed to open memory store: {e}");
            eprintln!(
                "Failed to open memory store at {}: {e}\n\
                 Hone will run without persistence. Use --ephemeral to suppress this warning.",
                memory_path.display()
            );
            None
        }
    };

    // Index store — open, run an initial full-directory index in a background
    // task, then start the file watcher for incremental updates.
    let index_path = dir.join("index.db");
    let (index_arc, watcher_handle) = match SqliteIndex::open(&index_path).await {
        Ok(index) => {
            tracing::info!(path = %index_path.display(), "symbol index initialised");
            let index = Arc::new(index);
            let cwd = std::env::current_dir().unwrap_or_default();

            // Initial full scan (detached — does not block startup).
            {
                let index_bg = Arc::clone(&index);
                let cwd_bg = cwd.clone();
                tokio::spawn(async move {
                    if let Err(e) = index_directory_bg(&index_bg, &cwd_bg).await {
                        tracing::warn!("background indexing failed: {e}");
                    } else {
                        tracing::info!("background indexing complete");
                    }
                });
            }

            // File watcher for incremental re-indexing.
            let watcher = IndexWatcher::new(Arc::clone(&index), cwd);
            let handle = match watcher.spawn().await {
                Ok(handle) => {
                    tracing::info!("file watcher started");
                    Some(handle)
                }
                Err(e) => {
                    tracing::warn!("file watcher failed to start: {e}");
                    None
                }
            };
            (Some(index), handle)
        }
        Err(e) => {
            tracing::warn!("failed to open symbol index: {e}");
            (None, None)
        }
    };

    PersistenceHandles {
        memory,
        index: index_arc,
        _watcher: watcher_handle,
    }
}

/// Walk `root` for `.rs` files and index them via `SqliteIndex::index_file`.
///
/// Hidden directories, `target/`, and non-UTF-8 paths are skipped silently.
async fn index_directory_bg(index: &SqliteIndex, root: &std::path::Path) -> Result<()> {
    fn is_skipped(entry: &walkdir::DirEntry) -> bool {
        let name = entry.file_name().to_string_lossy();
        if name.starts_with('.') {
            return true;
        }
        if entry.file_type().is_dir() && (name == "target" || name == "node_modules") {
            return true;
        }
        false
    }

    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_skipped(e))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        if !entry.file_type().is_file() {
            continue;
        }

        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if ext != "rs" && ext != "py" {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if let Err(e) = index.index_file(path, &content).await {
            tracing::debug!(path = %path.display(), "index_file error: {e}");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// TUI event loop
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();

    // Dispatch subcommands that don't need the full agent stack.
    if let Some(ref cmd) = cli.command {
        match cmd {
            HoneCmd::Configure => {
                configure::run_wizard()?;
                return Ok(());
            }
            HoneCmd::Session { cmd: session_cmd } => {
                match session_cmd {
                    SessionCmd::List => {
                        cmd_session_list().await?;
                    }
                    SessionCmd::Resume { name } => {
                        // Fall through — resume is handled later after the
                        // agent is constructed, so we can load history into it.
                        // Nothing to do here.
                        let _ = name; // suppress unused warning
                    }
                    SessionCmd::Rename { old, new } => {
                        cmd_session_rename(old, new).await?;
                    }
                    SessionCmd::Delete { name } => {
                        cmd_session_delete(name).await?;
                    }
                }
                // For List/Rename/Delete we exit immediately; Resume falls through.
                if !matches!(session_cmd, SessionCmd::Resume { .. }) {
                    return Ok(());
                }
            }
            HoneCmd::Trace { cmd: trace_cmd } => {
                let audit_path = std::env::current_dir()
                    .unwrap_or_default()
                    .join(".hone")
                    .join("audit.jsonl");
                match trace_cmd {
                    trace::TraceCmd::List { limit } => {
                        trace::cmd_list(&audit_path, *limit)?;
                    }
                    trace::TraceCmd::Show { id } => {
                        trace::cmd_show(&audit_path, id)?;
                    }
                    trace::TraceCmd::Stats => {
                        trace::cmd_stats(&audit_path)?;
                    }
                }
                return Ok(());
            }
            HoneCmd::Reports { cmd: reports_sub } => {
                let reports_dir = std::env::current_dir()
                    .unwrap_or_default()
                    .join(".hone")
                    .join("reports");
                match reports_sub {
                    reports_cmd::ReportsCmd::List { limit, keep_all } => {
                        let effective_limit = if *keep_all { 0 } else { *limit };
                        reports_cmd::cmd_list(&reports_dir, effective_limit)?;
                    }
                    reports_cmd::ReportsCmd::Show { id } => {
                        reports_cmd::cmd_show(&reports_dir, id)?;
                    }
                    reports_cmd::ReportsCmd::Diff { a, b } => {
                        reports_cmd::cmd_diff(&reports_dir, a, b)?;
                    }
                }
                return Ok(());
            }
            HoneCmd::Memory { .. } => {
                // Handled below, outside the `Some(ref cmd)` borrow scope,
                // so we can take ownership of the MemoryCmd variant.
            }
            HoneCmd::Eval { tasks, output } => {
                let eff = settings::load_effective_or_exit(cli.model.as_deref());
                cmd_eval(tasks, output.clone(), &eff).await?;
                return Ok(());
            }
            HoneCmd::Spec { goal, cwd } => {
                let eff = settings::load_effective_or_exit(cli.model.as_deref());
                cmd_spec_run(goal, cwd.clone(), &eff).await?;
                return Ok(());
            }
        }
    }

    // Handle `hone memory` after releasing the `Some(ref cmd)` borrow so we
    // can move the MemoryCmd value into `memory_cmd::run`.
    if let Some(HoneCmd::Memory { cmd: memory_sub }) = cli.command {
        let memory_path = data_dir().join("memory.db");
        if let Err(e) = std::fs::create_dir_all(data_dir()) {
            eprintln!("warning: could not create data dir: {e}");
        }
        let store: Arc<dyn MemoryStore> = match SqliteStore::open(&memory_path).await {
            Ok(s) => Arc::new(s),
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "failed to open memory store at {}: {e}",
                    memory_path.display()
                ));
            }
        };
        memory_cmd::run(memory_sub, store, &mut std::io::stdout()).await?;
        return Ok(());
    }

    // Read config.toml (user + project) once. A broken file stops here with
    // exit 2; warnings go to stderr once.
    let eff = settings::load_effective_or_exit(cli.model.as_deref());
    let hone_md = load_hone_md();

    // Initialise SQLite persistence (non-blocking; errors are warnings only).
    // The returned handles keep the memory store and file watcher alive for
    // the process lifetime.
    //
    // Persistence is skipped when `--ephemeral` is passed OR when the env var
    // `HONE_DISABLE_PERSISTENT_STORES=1` is set (useful for automated tests).
    let skip_persistence = cli.ephemeral
        || std::env::var("HONE_DISABLE_PERSISTENT_STORES")
            .map(|v| v == "1")
            .unwrap_or(false);

    // Resolve the memory passphrase: only read the env var when
    // `--encrypted-memory` is explicitly set; otherwise ignore it entirely.
    let memory_passphrase: Option<String> = if cli.encrypted_memory {
        match std::env::var("HONE_MEMORY_PASSPHRASE") {
            Ok(pp) if !pp.is_empty() => Some(pp),
            _ => {
                eprintln!(
                    "error: --encrypted-memory requires the HONE_MEMORY_PASSPHRASE \
                     environment variable to be set to a non-empty value.\n\
                     Example: export HONE_MEMORY_PASSPHRASE=\"your-strong-passphrase\""
                );
                std::process::exit(1);
            }
        }
    } else {
        None
    };

    let persistence = init_persistence(skip_persistence, memory_passphrase).await;
    let memory_store = persistence.memory.clone();
    let index = persistence.index.clone();

    // Tool-result cache: ON by default, disable with HONE_TOOL_CACHE=0.
    //
    // Stored at ~/.local/share/hone/tool-cache.db alongside the other SQLite
    // databases.  A failure to open the cache is non-fatal — the agent runs
    // uncached and logs a warning.
    let tool_cache: Option<Arc<ToolResultCache>> = if std::env::var("HONE_TOOL_CACHE")
        .map(|v| v != "0")
        .unwrap_or(true)
    {
        let cache_path = data_dir().join("tool-cache.db");
        match SqliteBackend::open(&cache_path).await {
            Ok(backend) => {
                tracing::info!(path = %cache_path.display(), "tool cache initialised");
                Some(Arc::new(ToolResultCache::new(Arc::new(backend))))
            }
            Err(e) => {
                tracing::warn!(
                    path = %cache_path.display(),
                    "failed to open tool cache: {e} — running without tool cache"
                );
                None
            }
        }
    } else {
        tracing::info!("tool cache disabled (HONE_TOOL_CACHE=0)");
        None
    };

    // Build the system prompt, injecting any HONE.md sections.
    let hone_md_sections: Vec<(String, String)> = hone_md
        .as_ref()
        .map(|md| {
            let mut pairs: Vec<(String, String)> = md
                .sections
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0)); // deterministic ordering
            pairs
        })
        .unwrap_or_default();
    if let Some(ref md) = hone_md {
        tracing::info!(
            sections = md.sections.len(),
            "HONE.md loaded and injected into system prompt"
        );
    }
    // One-shot full-corpus scan at session start. Bounded by the recall-time
    // 5000-row cap on non-empty queries, but uncapped here. Tens of ms at
    // most for typical corpora; consider caching if startup latency becomes
    // a concern.
    let memory_index_str = if let Some(store) = memory_store.as_ref() {
        match hone_core::memory_index::render_index(
            store.as_ref(),
            hone_core::memory_index::IndexBudget::default(),
        )
        .await
        {
            Ok(s) => Some(s),
            Err(e) => {
                tracing::warn!(error = %e, "failed to render memory index for system prompt");
                None
            }
        }
    } else {
        None
    };
    let system_prompt =
        system_prompt::build_system_prompt(&hone_md_sections, memory_index_str.as_deref());

    tracing::debug!(
        system_prompt_len = system_prompt.len(),
        "system prompt ready"
    );

    // An explicit model (--model, default_model, or the wizard's implied model)
    // builds the provider its prefix names; it never falls back to
    // auto-detection with a mismatched id.
    if let Some(choice) = &eff.model {
        let m = choice.model.as_str();
        let known_prefixes = [
            "anthropic:",
            "openai:",
            "ollama:",
            "claude-cli:",
            "gemini-cli:",
        ];
        // A known prefix with a bad name gets the name's reason; only an unknown
        // prefix gets the prefix help.
        if let Some(name) = known_prefixes.iter().find_map(|p| m.strip_prefix(p)) {
            if let Err(why) = hone_config::resolve::validate_model_name(name) {
                eprintln!("{} invalid model ID '{m}': {why}", error_style("Error:"));
                std::process::exit(1);
            }
        }
        if !known_prefixes.iter().any(|p| m.starts_with(p)) {
            eprintln!("{} invalid model ID '{m}'", error_style("Error:"));
            eprintln!();
            eprintln!("Model IDs must start with a provider prefix:");
            eprintln!("  anthropic:claude-sonnet-4-5");
            eprintln!("  openai:gpt-4o");
            eprintln!("  openai:deepseek-coder      (with HONE_OPENAI_BASE_URL)");
            eprintln!("  ollama:llama3.2");
            eprintln!("  ollama:qwen2.5-coder:14b");
            eprintln!("  claude-cli:claude-sonnet-4-5");
            eprintln!("  gemini-cli:gemini-2.5-pro");
            std::process::exit(1);
        }
    }
    let mut detected = if let Some(choice) = &eff.model {
        match build_explicit_provider(choice, &eff) {
            Ok(d) => {
                settings::announce_project_model(choice);
                d
            }
            // A repository's choice that can't be built (no key) must not stop every
            // contributor without that key. It falls through like an ignored choice:
            // the user's default_model, then the wizard model, then auto-detection
            // (with detection's own model id).
            Err(msg) if settings::is_project_sourced(choice) => {
                let next = settings::load_effective_without_project_model();
                match &next.model {
                    Some(own) => {
                        eprintln!(
                            "hone: warning: {msg}; falling back to your own model choice"
                        );
                        match build_explicit_provider(own, &next) {
                            Ok(d) => d,
                            Err(msg) => {
                                eprintln!("{} {msg}", error_style("Error:"));
                                std::process::exit(1);
                            }
                        }
                    }
                    None => {
                        eprintln!("hone: warning: {msg}; falling back to auto-detection");
                        detect_provider(&next).await
                    }
                }
            }
            Err(msg) => {
                eprintln!("{} {msg}", error_style("Error:"));
                std::process::exit(1);
            }
        }
    } else {
        detect_provider(&eff).await
    };

    // A session on an API model gets a provider that serves every configured
    // API provider (routing, fallback models, `/model` across providers). With an
    // agent CLI model nothing is routed.
    let pinned_choice: Option<ModelId> = match &eff.model {
        Some(ModelChoice {
            model,
            source: ModelSource::Cli,
        }) => Some(model.clone()),
        _ => None,
    };
    let dispatch: Option<Arc<hone_providers::ModelDispatch>> = if is_api_model(&detected.model_id) {
        let d = build_model_dispatch(&eff, &detected, pinned_choice.is_some());
        detected.provider = d.clone();
        Some(d)
    } else {
        warn_routing_not_applied(&eff);
        None
    };

    // Opt-in local response cache: set HONE_CACHE=1 (or any value) to wrap
    // the inner provider with an LRU cache. Useful for development/testing
    // and retry scenarios where the exact same request may repeat.
    if std::env::var("HONE_CACHE").is_ok() {
        tracing::info!("local response cache enabled (HONE_CACHE set)");
        detected.provider = Arc::new(hone_providers::CachingProvider::new(detected.provider, 100));
    }

    let session_default: ModelId = detected.model_id.clone();
    let mut model_id: ModelId = session_default.clone();

    // One-shot runs (plain, --json, --orchestrate, --ensemble) pick their model by
    // the prompt's complexity tier; the TUI does it per turn.
    if let Some(d) = &dispatch {
        let one_shot_prompt = if cli.recipe.is_none() && cli.plan.is_none() && !cli.list_recipes {
            cli.prompt.as_deref()
        } else {
            None
        };
        if let Some(prompt) = one_shot_prompt {
            let tier = tier_of(ComplexityClassifier::classify(prompt));
            let turn = turn_model(&eff, tier, pinned_choice.as_ref(), &session_default, &|m| {
                d.has_provider_for(m)
            });
            if let Some(path) = &turn.routed_from {
                eprintln!(
                    "hone: using {} for this {} request (from [[routing]] in {})",
                    turn.model,
                    tier.as_str(),
                    path.display()
                );
            }
            model_id = turn.model;
        }
    }

    // Provider capability warnings: --orchestrate and --ensemble require tool
    // calling. Warn (but do not abort) when used with a CLI/Ollama provider
    // that cannot invoke tools.
    if !detected.supports_tools {
        let provider_name = detected.model_id.to_string();
        let provider_name = provider_name.split(':').next().unwrap_or(&provider_name);

        if cli.orchestrate {
            eprintln!(
                "{} --orchestrate requires tool calling. \
                 Current provider ({provider_name}) does not support tools. \
                 Results may be degraded.",
                warn_style("Warning:")
            );
        }
        if cli.ensemble.is_some() {
            eprintln!(
                "{} --ensemble requires tool calling. \
                 Current provider ({provider_name}) does not support tools. \
                 Results may be degraded.",
                warn_style("Warning:")
            );
        }
    }

    // Try to spawn an LSP bridge for the project language (falls back to mock
    // if the language server binary is not installed).
    let lsp = detect_lsp_bridge().await;

    // Load MCP config (global + project), then spawn and handshake all enabled
    // MCP servers.  A broken MCP server is logged and skipped — it must never
    // prevent the agent from starting.
    let mcp_config = {
        let global = std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".config/hone/mcp.json"));
        let project = PathBuf::from(".hone/mcp.json");
        McpConfig::load(
            global.as_deref(),
            if project.exists() {
                Some(project.as_path())
            } else {
                None
            },
        )
    };
    let mcp_manager = McpManager::from_config(&mcp_config).await;
    let mcp_tools = mcp_manager.tools_as_native();
    tracing::info!(
        servers = mcp_manager.server_count(),
        tools = mcp_tools.len(),
        "MCP manager initialised"
    );

    // Build a single clonable config; share it across all code paths.
    let mut base_config = build_agent_config(
        &detected,
        model_id.clone(),
        system_prompt.clone(),
        Arc::clone(&lsp),
        index,
        memory_store.clone(),
        tool_cache,
        mcp_tools,
        cli.no_inject,
        cli.skeleton_strategy.as_deref(),
        cli.audit,
        eff.session_limit_usd,
    );
    apply_policy_flags(&mut base_config, &cli.approve);

    let agent = AgentHandle::spawn(base_config.clone());

    // Session resume: load history into the freshly spawned agent.
    let is_resume = matches!(
        cli.command,
        Some(HoneCmd::Session {
            cmd: SessionCmd::Resume { .. }
        })
    );
    if is_resume {
        if let Some(HoneCmd::Session {
            cmd: SessionCmd::Resume { ref name },
        }) = cli.command
        {
            if let Some(store) = open_session_store().await {
                match store.load_session(name).await {
                    Ok(Some(messages)) => {
                        let count = messages.len();
                        agent.load_history(messages).await;
                        eprintln!("Resumed session '{}' ({count} messages).", name);
                    }
                    Ok(None) => {
                        eprintln!("{} Session '{}' not found.", error_style("Error:"), name);
                        std::process::exit(1);
                    }
                    Err(e) => {
                        eprintln!("{} Could not load session: {e}", error_style("Error:"));
                        std::process::exit(1);
                    }
                }
            }
        }
    }

    // Determine the active session name for auto-save.
    // Priority: --session NAME > resume name > generated default.
    let session_name: Option<String> = if cli.ephemeral {
        None
    } else {
        cli.session.clone().or_else(|| {
            if let Some(HoneCmd::Session {
                cmd: SessionCmd::Resume { ref name },
            }) = cli.command
            {
                Some(name.clone())
            } else {
                None
            }
        })
    };

    // Run the selected mode and capture the result so we can shut down MCP
    // servers before returning regardless of which path was taken.
    let no_clarify = cli.no_clarify;
    let run_result = run_selected_mode(
        cli,
        agent,
        base_config,
        model_id,
        session_name,
        no_clarify,
        dispatch.map(|dispatch| TuiRouting {
            eff,
            dispatch,
            pinned: pinned_choice,
        }),
    )
    .await;

    // Graceful MCP server shutdown — always runs, even on error.
    mcp_manager.shutdown().await;

    if let Err(e) = &run_result {
        if e.downcast_ref::<AlreadyReported>().is_some() {
            std::process::exit(1);
        }
    }

    run_result
}

/// Wire interactive approvals for the TUI: default policy is AskRisky
/// (AlwaysAsk with `--step`) plus a step channel, applied to both
/// `base_config` and the running `agent` (no respawn, history is kept).
async fn wire_tui_approvals(
    agent: &AgentHandle,
    base_config: &mut AgentConfig,
    step: bool,
) -> tokio::sync::mpsc::Sender<hone_proto::StepAction> {
    let (step_tx, step_rx) = tokio::sync::mpsc::channel::<hone_proto::StepAction>(1);
    let step_rx = std::sync::Arc::new(tokio::sync::Mutex::new(step_rx));
    let policy = if step {
        ToolApprovalPolicy::AlwaysAsk
    } else {
        ToolApprovalPolicy::AskRisky {
            always_allowed: Vec::new(),
        }
    };
    base_config.approval_policy = policy.clone();
    base_config.step_rx = Some(step_rx.clone());
    agent.set_approval_policy(policy).await;
    agent.set_step_channel(Some(step_rx)).await;
    step_tx
}

/// Dispatch to the appropriate execution mode based on the parsed CLI flags.
///
/// Extracted from `main` so that `mcp_manager.shutdown()` can unconditionally
/// run after all execution paths without duplicating the call.
async fn run_selected_mode(
    cli: Cli,
    agent: AgentHandle,
    base_config: AgentConfig,
    model_id: ModelId,
    session_name: Option<String>,
    no_clarify: bool,
    routing: Option<TuiRouting>,
) -> Result<()> {
    // --list-recipes: print available recipes and exit.
    if cli.list_recipes {
        list_builtin_recipes();
        return Ok(());
    }

    // Parse --var KEY=VALUE pairs into a HashMap for recipe substitution.
    let recipe_vars: std::collections::HashMap<String, String> = cli
        .vars
        .iter()
        .filter_map(|kv| {
            let mut parts = kv.splitn(2, '=');
            let key = parts.next()?.to_owned();
            let value = parts.next().unwrap_or("").to_owned();
            Some((key, value))
        })
        .collect();

    // Every non-TUI mode that can run the shell tool kills its running
    // commands when a signal ends it (the TUI does so on its own exit).
    if cli.recipe.is_some()
        || cli.ensemble.is_some()
        || cli.plan.is_some()
        || cli.orchestrate
        || cli.prompt.is_some()
    {
        install_one_shot_signal_handlers()?;
    }

    // --recipe <name>: run a built-in recipe and exit.
    if let Some(ref recipe_name) = cli.recipe {
        return run_recipe(agent.clone(), recipe_name, &recipe_vars).await;
    }

    // --ensemble N: run N parallel candidates and pick the best.
    if let Some(n) = cli.ensemble {
        if n < 2 || n > 10 {
            eprintln!(
                "{} --ensemble value must be between 2 and 10 (got {n})",
                error_style("Error:")
            );
            eprintln!("  2 is the minimum useful value (needs at least 2 candidates to compare)");
            eprintln!("  10 is the maximum (higher values risk runaway API costs)");
            std::process::exit(1);
        }

        let prompt = match cli.prompt.as_deref() {
            Some(p) if !p.is_empty() => p.to_owned(),
            _ => {
                eprintln!("Error: --ensemble requires a prompt (pass it as a positional argument)");
                return Ok(());
            }
        };

        return run_ensemble(&base_config, &prompt, n).await;
    }

    // --plan: load a saved plan file and execute its plan_body as a one-shot prompt.
    if let Some(ref plan_path) = cli.plan {
        let plan = PlanModeStore::load(plan_path).await.map_err(|e| {
            anyhow::anyhow!("failed to load plan from {}: {}", plan_path.display(), e)
        })?;
        println!(
            "Executing plan {} ({})",
            plan.slug, plan.timestamp
        );
        run_one_shot(agent.clone(), &plan.plan_body).await?;
        if !cli.ephemeral {
            auto_save_session(&agent, session_name.as_deref()).await;
        }
        println!("Executed plan {} ({})", plan.slug, plan.timestamp);
        return Ok(());
    }

    // --orchestrate: full orchestrator for the supplied prompt (or a default).
    if cli.orchestrate {
        let prompt = cli
            .prompt
            .clone()
            .unwrap_or_else(|| "Please describe what you need help with.".to_owned());

        return run_orchestrate(base_config, &prompt, cli.distributed.as_deref()).await;
    }

    // One-shot mode: run a single turn, print to stdout, and exit.
    if let Some(ref prompt) = cli.prompt {
        // Clarification step: when the prompt is ambiguous and --no-clarify
        // was not set, print questions to stderr and continue (we do not block
        // execution — just surface the questions so the user is aware).
        if !no_clarify && !cli.json {
            if let Some(questions) = ClarificationDetector::needs_clarification(prompt) {
                let formatted = ClarificationDetector::format_questions(&questions);
                eprintln!("{}", warn_style("[Hone] Ambiguous prompt detected."));
                eprintln!("{formatted}");
                eprintln!();
                eprintln!(
                    "{} Proceeding with the original prompt. \
                     Re-run with a more specific prompt or pass --no-clarify to suppress this.",
                    warn_style("Note:")
                );
                eprintln!();
            }
        }

        if !cli.image.is_empty() {
            let content = image_input::build_content(&cli.image, prompt)
                .map_err(|e| anyhow::anyhow!("image input error: {e}"))?;
            run_one_shot_content(agent.clone(), content).await?;
        } else if cli.json {
            run_json_mode(agent.clone(), prompt, model_id.as_str()).await?;
        } else {
            run_one_shot(agent.clone(), prompt).await?;
        }
        if !cli.ephemeral {
            auto_save_session(&agent, session_name.as_deref()).await;
        }
        return Ok(());
    }

    // Resume with no explicit prompt drops into TUI so the user can continue
    // the conversation interactively.
    // (fall through to TUI below)

    // TUI mode.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Wire approvals on both the config (used for future agents) and the
    // already-running agent, which holds any resumed history.
    let mut base_config = base_config;
    let step_tx = Some(wire_tui_approvals(&agent, &mut base_config, cli.step).await);

    let result = run_tui(
        &mut terminal,
        agent.clone(),
        model_id,
        base_config,
        cli.vim,
        step_tx,
        routing,
    )
    .await;

    // Always restore terminal even on error.
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    if result.is_ok() {
        eprintln!("\nHone session ended.");
    }

    result
}

// ---------------------------------------------------------------------------
// OS-signal helpers
// ---------------------------------------------------------------------------

/// Waits for the next SIGINT delivery on unix, or for the first Ctrl+C
/// future to complete on non-unix platforms.  Using a helper function lets
/// us place a single `_ = os_signal(...)` branch inside `tokio::select!`
/// without needing `#[cfg(...)]` attributes inside the macro invocation,
/// which the macro does not support.
#[cfg(unix)]
async fn os_signal(sigint: &mut tokio::signal::unix::Signal) {
    sigint.recv().await;
}

#[cfg(not(unix))]
async fn os_signal(_sigint: &mut ()) {
    let _ = tokio::signal::ctrl_c().await;
}

/// Kills the commands the shell tool and the test loop still have running when
/// dropped. They run in their own process groups, so neither the terminal's
/// Ctrl+C nor its hangup reaches them; hone's exit has to (W25).
struct KillCommandsOnExit;

impl Drop for KillCommandsOnExit {
    fn drop(&mut self) {
        hone_tools::kill_running_commands();
    }
}

/// Empty the agent event queue after a turn was cancelled, so no menu opens for
/// the dead run. Each dropped `ToolCallPaused` is answered with `Deny` on the
/// step channel, so whoever holds the step lock (e.g. a post-turn fix run that
/// is not the cancelled task) is released instead of waiting forever; a dropped
/// `Done` still counts its cost and tokens.
fn drain_after_cancel(
    app: &mut App,
    rx: &mut mpsc::Receiver<AgentEvent>,
    step_tx: Option<&mpsc::Sender<hone_proto::StepAction>>,
) {
    while let Ok(event) = rx.try_recv() {
        match event {
            AgentEvent::ToolCallPaused { .. } => {
                if let Some(tx) = step_tx {
                    let _ = tx.try_send(hone_proto::StepAction::Deny("turn cancelled".to_owned()));
                }
            }
            AgentEvent::Done {
                usage, cost_usd, ..
            } => app.record_spend(usage.total(), cost_usd),
            _ => {}
        }
    }
}

/// Waits for the next SIGTERM or SIGHUP on unix; never completes elsewhere.
#[cfg(unix)]
async fn os_term_signal(signals: &mut (tokio::signal::unix::Signal, tokio::signal::unix::Signal)) {
    tokio::select! {
        _ = signals.0.recv() => {}
        _ = signals.1.recv() => {}
    }
}

#[cfg(not(unix))]
async fn os_term_signal(_signals: &mut ()) {
    std::future::pending::<()>().await;
}

/// One-shot runs (`-p`, `--plan`, `--orchestrate`, `--recipe`, `--ensemble`): on SIGINT, SIGTERM or
/// SIGHUP kill the running commands and end the process by that signal.
/// Without this, commands in their own process groups would outlive hone.
#[cfg(unix)]
fn install_one_shot_signal_handlers() -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};
    // The signal numbers are the same on every unix we target.
    for (kind, number) in [
        (SignalKind::interrupt(), 2),
        (SignalKind::terminate(), 15),
        (SignalKind::hangup(), 1),
    ] {
        let mut sig =
            signal(kind).map_err(|e| anyhow::anyhow!("failed to install signal handler: {e}"))?;
        tokio::spawn(async move {
            sig.recv().await;
            // Ends the process BY the signal, so a calling script stops too.
            hone_tools::kill_commands_and_reraise(number);
        });
    }
    Ok(())
}

#[cfg(not(unix))]
fn install_one_shot_signal_handlers() -> Result<()> {
    Ok(())
}

/// Return the context window size (in tokens) for a given model identifier.
fn model_context_limit(model: &str) -> u32 {
    let model_lower = model.to_ascii_lowercase();
    // Strip provider prefix (e.g. "anthropic:claude-sonnet-4-5" -> "claude-sonnet-4-5").
    let name = model_lower.split(':').last().unwrap_or(&model_lower);

    if name.contains("claude") {
        // All Claude 3+ models have a 200k context window.
        200_000
    } else if name.contains("gemini") {
        1_000_000
    } else if name.contains("o1") || name.contains("o3") || name.contains("o4") {
        200_000
    } else if name.contains("gpt-4o") || name.contains("gpt-4-turbo") {
        128_000
    } else if name.contains("gpt-4") {
        8_192
    } else if name.contains("gpt-3.5") {
        16_385
    } else if name.contains("deepseek") {
        if name.contains("v3") || name.contains("chat") || name.contains("r1") {
            64_000
        } else {
            16_000
        }
    } else if name.contains("qwen") {
        if name.contains("2.5") { 32_768 } else { 8_192 }
    } else if name.contains("llama") {
        if name.contains("3.2") || name.contains("3.1") {
            128_000
        } else {
            8_192
        }
    } else {
        128_000 // reasonable default
    }
}

/// Remember the fallback model of a `ModelFallback` event for the running turn.
fn note_turn_fallback(event: &AgentEvent, slot: &mut Option<String>) {
    if let AgentEvent::ModelFallback { fallback, .. } = event {
        *slot = Some(fallback.clone());
    }
}

/// Forget the turn's fallback model once the turn has ended.
fn end_turn_fallback(event: &AgentEvent, slot: &mut Option<String>) {
    if matches!(event, AgentEvent::Done { .. } | AgentEvent::Error { .. }) {
        *slot = None;
    }
}

/// The `ModelFallback` events of a run, for the one-shot stderr line and the
/// `--json` `fallbacks` array: one entry per (requested, fallback) pair, in
/// first-seen order.
#[derive(Default)]
struct FallbackLog {
    pairs: Vec<FallbackPair>,
}

/// One (requested, fallback) pair seen in a run.
struct FallbackPair {
    requested: String,
    fallback: String,
    /// The first reason seen for the pair.
    reason: String,
    /// Number of `ModelFallback` events for the pair.
    calls: u32,
}

impl FallbackLog {
    /// Record an event; returns the stderr line if this pair is new.
    fn record(&mut self, requested: &str, fallback: &str, reason: &str) -> Option<String> {
        if let Some(entry) = self
            .pairs
            .iter_mut()
            .find(|p| p.requested == requested && p.fallback == fallback)
        {
            entry.calls += 1;
            return None;
        }
        self.pairs.push(FallbackPair {
            requested: requested.to_owned(),
            fallback: fallback.to_owned(),
            reason: reason.to_owned(),
            calls: 1,
        });
        Some(format!(
            "hone: {requested} failed ({reason}); switched to fallback {fallback}"
        ))
    }

    fn to_json(&self) -> serde_json::Value {
        self.pairs
            .iter()
            .map(|p| {
                serde_json::json!({
                    "requested": p.requested,
                    "fallback": p.fallback,
                    "reason": p.reason,
                    "calls": p.calls,
                })
            })
            .collect()
    }
}

/// Prints the one-shot stderr line for each new fallback pair, on its own
/// line even when stdout tokens ended mid-line.
#[derive(Default)]
struct FallbackPrinter {
    log: FallbackLog,
    mid_line: bool,
}

impl FallbackPrinter {
    /// Note tokens written to stdout.
    fn wrote(&mut self, text: &str) {
        if !text.is_empty() {
            self.mid_line = !text.ends_with('\n');
        }
    }

    fn event<E: io::Write>(&mut self, err: &mut E, requested: &str, fallback: &str, reason: &str) {
        if let Some(line) = self.log.record(requested, fallback, reason) {
            let lead = if self.mid_line { "\n" } else { "" };
            let _ = writeln!(err, "{lead}{line}");
            self.mid_line = false;
        }
    }
}

fn record_outcome_for_event(
    event: &AgentEvent,
    adaptive: &Option<Arc<AdaptiveClassifier>>,
    current_tier: &mut Option<ComplexityTier>,
    model: &str,
    input_per_million: f64,
    output_per_million: f64,
    turn_prompt: &str,
) {
    let Some(adaptive_ref) = adaptive else { return };
    match event {
        AgentEvent::Done { usage, .. } => {
            if let Some(tier) = current_tier.take() {
                let cost_usd = (usage.input as f64 * input_per_million
                    + usage.output as f64 * output_per_million)
                    / 1_000_000.0;
                let record = OutcomeRecord {
                    tier,
                    model: model.to_string(),
                    success: true,
                    cost_usd,
                    total_tokens: u64::from(usage.total()),
                    task_summary: turn_prompt.to_string(),
                };
                let a = Arc::clone(adaptive_ref);
                tokio::spawn(async move {
                    a.record_outcome(record).await;
                });
            }
        }
        AgentEvent::Error { .. } => {
            if let Some(tier) = current_tier.take() {
                let record = OutcomeRecord {
                    tier,
                    model: model.to_string(),
                    success: false,
                    cost_usd: 0.0,
                    total_tokens: 0,
                    task_summary: turn_prompt.to_string(),
                };
                let a = Arc::clone(adaptive_ref);
                tokio::spawn(async move {
                    a.record_outcome(record).await;
                });
            }
        }
        _ => {}
    }
}

async fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    agent: AgentHandle,
    model_id: ModelId,
    base_config: AgentConfig,
    vim: bool,
    step_tx: Option<tokio::sync::mpsc::Sender<hone_proto::StepAction>>,
    routing: Option<TuiRouting>,
) -> Result<()> {
    let mut app = if vim { App::with_vim() } else { App::new() };
    // Wire step-mode sender into app so it can unblock the agent.
    if let Some(tx) = &step_tx {
        app.set_step_tx(tx.clone());
    }

    // Show which model is active in the status bar and set the context limit.
    app.status_mut().model = model_id.to_string();
    app.status_mut().context_limit = model_context_limit(model_id.as_str());

    // Per-turn model routing (API sessions only). `pinned` is set by --model and
    // by a successful `/model`: from then on routing stops for the session.
    let mut pinned_model: Option<ModelId> = routing.as_ref().and_then(|r| r.pinned.clone());
    // The model the agent is currently set to, and the label shown in the status bar.
    let mut applied_model: ModelId = model_id.clone();
    let mut shown_model: String = model_id.to_string();
    // A `/model` accepted while a turn is running waits for the turn to end, so one
    // turn (answer, critique, refine, test-fix follow-ups) never spans two models.
    let mut deferred_model: Option<(String, ModelId)> = None;
    // The most recent turn task, to tell when it has really finished (a medium turn
    // or the automatic test fix keeps running after the first `Done`).
    let mut last_turn_task: Option<tokio::task::AbortHandle> = None;
    // Apply a checked model switch: agent, status bar, confirmation, and the pin
    // (routing stops for the rest of the session; fallback models still apply).
    macro_rules! apply_model_switch {
        ($new_model:expr, $model:expr) => {{
            let new_model: String = $new_model;
            let model: ModelId = $model;
            agent.set_model(model.clone()).await;
            app.status_mut().model = new_model.clone();
            app.status_mut().context_limit = model_context_limit(&new_model);
            app.push_system_message(format!("Model changed to: {new_model}"));
            shown_model = new_model;
            applied_model = model.clone();
            if routing.is_some() {
                pinned_model = Some(model);
            }
        }};
    }

    // The model an orchestrated run (Complex turn, plan run, `/plan` decompose)
    // uses: pinned, else the [[routing]] model for the Complex tier, else the
    // agent's current model (so `/model` reaches orchestrated runs).
    macro_rules! complex_model {
        () => {{
            match &routing {
                Some(r) => {
                    turn_model(
                        &r.eff,
                        Tier::Complex,
                        pinned_model.as_ref(),
                        &model_id,
                        &|m| r.dispatch.has_provider_for(m),
                    )
                    .model
                }
                None => applied_model.clone(),
            }
        }};
    }
    // Drain `/budget` and `/permit`: apply them to the agent here; the caller
    // mirrors the returned overrides onto an orchestrated run's config clone.
    macro_rules! drain_governance {
        () => {{
            let pending_gov = app.take_pending_governance();
            if pending_gov.budget_usd.is_some() {
                agent.set_budget_limit(pending_gov.budget_usd).await;
            }
            if !pending_gov.permits.is_empty() {
                // Fire-and-forget: never wait on the actor from the UI loop.
                agent.approve_tools(pending_gov.permits.clone()).await;
            }
            pending_gov
        }};
    }

    // Load saved sessions into the session-switcher overlay.
    if let Some(store) = open_session_store().await {
        if let Ok(sessions) = store.list_sessions().await {
            app.set_sessions(
                sessions
                    .into_iter()
                    .map(|s| hone_tui::SessionSummary {
                        name: s.name,
                        updated_at: s.updated_at,
                        message_count: s.message_count,
                    })
                    .collect(),
            );
        }
    }

    let (agent_tx, mut agent_rx) = mpsc::channel::<AgentEvent>(256);

    let tick = Duration::from_millis(100);

    let mut events = EventStream::new();
    // Abort handle for the currently running agent task; cleared on completion.
    let mut current_task_abort: Option<tokio::task::AbortHandle> = None;
    // `/plan` decompose runs as a background task and reports here, so the loop
    // keeps drawing and Esc can cancel it. Results carry the id of the planning
    // run; one that isn't `planning_id` (cancelled meanwhile) is dropped.
    type PlanResult = (u64, String, Result<(hone_core::TaskPlan, f64), String>);
    let (plan_tx, mut plan_rx) = mpsc::channel::<PlanResult>(1);
    let mut planning_id: Option<u64> = None;
    let mut next_planning_id: u64 = 0;

    // Adaptive classifier: wired only when a memory store is available.
    let adaptive: Option<Arc<AdaptiveClassifier>> = base_config
        .memory_store
        .as_ref()
        .map(|store| Arc::new(AdaptiveClassifier::new(Arc::clone(store))));
    let mut current_tier: Option<ComplexityTier> = None;
    let mut current_turn_prompt = String::new();
    // The model the running turn was sent to (for outcome records).
    let mut current_turn_model: String = model_id.to_string();
    // Model of the last `ModelFallback` of the running turn: the adaptive
    // classifier records the turn under it, since it gave the answer.
    let mut turn_fallback_model: Option<String> = None;

    // Install a SIGINT handler so the process shuts down cleanly when the
    // terminal sends Ctrl+C at the OS level (e.g. from a wrapper script).
    //
    // On unix we use `signal(SIGINT)` so we get a reusable stream; on other
    // platforms the `os_signal` helper calls `tokio::signal::ctrl_c()` directly
    // and the placeholder `()` value is never actually used.
    #[cfg(unix)]
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .map_err(|e| anyhow::anyhow!("failed to install SIGINT handler: {e}"))?;
    #[cfg(not(unix))]
    let mut sigint = ();
    // SIGTERM and SIGHUP end the TUI like SIGINT (W25).
    #[cfg(unix)]
    let mut term_signals = (
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| anyhow::anyhow!("failed to install SIGTERM handler: {e}"))?,
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .map_err(|e| anyhow::anyhow!("failed to install SIGHUP handler: {e}"))?,
    );
    #[cfg(not(unix))]
    let mut term_signals = ();
    // Whatever way the loop ends, commands still running die with hone.
    let _kill_commands_on_exit = KillCommandsOnExit;

    loop {
        if app.should_quit() {
            break;
        }

        // Tell the app whether the previous turn's task (post-turn test run,
        // fix turns) still runs, so Esc can stop it while the UI is idle.
        app.set_background_task_running(last_turn_task.as_ref().is_some_and(|t| !t.is_finished()));

        terminal.draw(|frame| root::render(&app, frame))?;

        // Mouse text-selection → clipboard: drain the pending flag, extract
        // the selected text from the just-rendered buffer, and push it to the
        // system clipboard. On failure, surface a system message.
        if let Some(sel) = app.take_pending_copy() {
            let buf = terminal.current_buffer_mut();
            let text = hone_tui::selection::extract_text(buf, &sel);
            if !text.is_empty() {
                if let Err(err) = arboard::Clipboard::new().and_then(|mut cb| cb.set_text(text)) {
                    app.push_system_message(format!("clipboard error: {err}"));
                }
            }
        }

        tokio::select! {
            // Async crossterm events
            maybe_event = events.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => {
                        // Ctrl-C always quits cleanly.
                        if key.code == KeyCode::Char('c')
                            && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                        {
                            tracing::info!("Ctrl+C key pressed, shutting down");
                            break;
                        }

                        // Enter on the plan overlay is the overlay's Run, not a submit.
                        if key.code == KeyCode::Enter
                            && !app.plan_preview_open
                            && !app.input().is_empty()
                            && matches!(app.state(), AppState::Idle | AppState::Error(_))
                        {
                            let user_text = app.input().buffer.clone();
                            let is_command = user_text.starts_with('/');
                            let is_shell_escape = user_text.starts_with('!');
                            // @agent-name prefix is also detected by the app's
                            // submit_input; we check here only for the routing decision.
                            let is_inline_agent = user_text.trim_start().starts_with('@');

                            // Let the App update its own state.
                            app.handle_key(key);

                            // Handle pending interactive commands (e.g. nvim).
                            // The app sets `pending_interactive` when it detects
                            // an interactive shell escape.
                            if let Some((cmd, wd)) = app.take_pending_interactive() {
                                // Suspend TUI.
                                disable_raw_mode()?;
                                execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
                                terminal.show_cursor()?;

                                // Run the command synchronously.
                                if let Err(e) = run_interactive_blocking(&cmd, &wd) {
                                    tracing::warn!("interactive command '{cmd}' failed: {e}");
                                }

                                // Restore TUI.
                                enable_raw_mode()?;
                                execute!(terminal.backend_mut(), EnterAlternateScreen)?;
                                terminal.clear()?;

                                app.handle_interactive_command_done();
                            }

                            // Handle pending non-interactive shell escapes.
                            if let Some(cmd) = app.take_pending_shell_cmd() {
                                let working_dir = app.working_dir().to_path_buf();
                                let result = execute_shell_escape(&cmd, &working_dir).await;
                                app.handle_shell_escape_result(result);
                            }

                            // Handle pending agent mode changes from Shift+Tab.
                            if let Some(new_mode) = app.take_pending_mode_change() {
                                agent.set_mode(new_mode).await;
                            }

                            if !is_command && !is_shell_escape && matches!(app.state(), AppState::WaitingForResponse) {
                                // Only spawn an AI turn for regular messages, not commands or
                                // shell escapes.

                                // @agent inline dispatch: swap system prompt for this turn.
                                let maybe_inline_agent = if is_inline_agent {
                                    app.take_inline_agent()
                                } else {
                                    None
                                };

                                // Track whether we applied a filtered tool registry so we
                                // can restore the base registry after the turn completes.
                                let mut applied_tool_filter = false;
                                // The persona's `tools:` list, kept so an orchestrated run
                                // narrows its tools the same way (W18).
                                let mut persona_tools: Vec<String> = Vec::new();
                                if let Some(ref agent_name) = maybe_inline_agent {
                                    // Look up the agent definition and swap the prompt.
                                    let registry = AgentRegistry::load_hierarchical(app.working_dir());
                                    if let Some(def) = registry.get(agent_name) {
                                        let persona_prompt = def.system_prompt.clone();
                                        agent.set_system_prompt(persona_prompt).await;

                                        // Apply tool allowlist when the AgentDef specifies one.
                                        if !def.tools.is_empty() {
                                            let filtered = hone_core::filtered_registry(
                                                &base_config.tools,
                                                &def.tools,
                                            );
                                            agent.set_tools(filtered).await;
                                            applied_tool_filter = true;
                                            persona_tools = def.tools.clone();
                                        }

                                        tracing::info!(agent = %agent_name, "inline @agent persona active");
                                        app.status_mut().mode = format!("@{agent_name}");
                                    } else {
                                        tracing::warn!(agent = %agent_name, "unknown @agent name — using default persona");
                                    }
                                }

                                // Determine the effective prompt for the turn.
                                // When an @agent was parsed, app already stored the
                                // remainder as the last User message; we use that text.
                                let effective_text = if is_inline_agent {
                                    // Extract the prompt portion that was already pushed as a
                                    // User message by submit_input.
                                    app.messages()
                                        .iter()
                                        .rev()
                                        .find(|m| matches!(m.role, hone_tui::message::DisplayRole::User))
                                        .map(|m| m.content.clone())
                                        .unwrap_or(user_text.clone())
                                } else {
                                    user_text.clone()
                                };

                                // Adaptive routing: classify complexity and pick the
                                // appropriate execution strategy.
                                let tier = if let Some(ref a) = adaptive {
                                    a.suggest_tier(&effective_text).await
                                        .unwrap_or_else(|| ComplexityClassifier::classify(&effective_text))
                                } else {
                                    ComplexityClassifier::classify(&effective_text)
                                };
                                current_tier = Some(tier);
                                turn_fallback_model = None;
                                current_turn_prompt = effective_text.chars().take(200).collect::<String>();

                                // Apply per-run governance overrides set via /budget and /permit
                                // BEFORE the turn dispatches. Drains the pending state so each
                                // override fires exactly once. The orchestrated branch picks up
                                // changes through `base_config_clone` below; the Simple/Medium
                                // branches go through the AgentHandle, which has its own setters.
                                let pending_gov = drain_governance!();

                                // Pick this turn's model: pinned, else the [[routing]] model for the
                                // tier, else the session default. Simple/medium turns run on the
                                // agent (set its model); orchestrated turns use the config clone.
                                // A change still waiting for the previous turn must be in
                                // place (and pinned) before this turn's model is chosen.
                                if let Some((new_model, model)) = deferred_model.take() {
                                    apply_model_switch!(new_model, model);
                                }
                                let turn_model_choice = routing.as_ref().map(|r| {
                                    turn_model(
                                        &r.eff,
                                        tier_of(tier),
                                        pinned_model.as_ref(),
                                        &model_id,
                                        &|m| r.dispatch.has_provider_for(m),
                                    )
                                });
                                current_turn_model = turn_model_choice
                                    .as_ref()
                                    .map_or_else(|| shown_model.clone(), |t| t.model.to_string());
                                if let Some(turn) = &turn_model_choice {
                                    if !matches!(tier, ComplexityTier::Complex)
                                        && turn.model != applied_model
                                    {
                                        agent.set_model(turn.model.clone()).await;
                                        applied_model = turn.model.clone();
                                    }
                                    if turn.model.as_str() != shown_model {
                                        shown_model = turn.model.to_string();
                                        app.status_mut().model = shown_model.clone();
                                        app.status_mut().context_limit =
                                            model_context_limit(turn.model.as_str());
                                    }
                                }

                                // Spawn the appropriate turn.
                                let agent_clone = agent.clone();
                                let effective_text_clone = effective_text.clone();
                                let mut base_config_clone = base_config.clone();
                                if let Some(turn) = &turn_model_choice {
                                    base_config_clone.model = turn.model.clone();
                                }
                                if matches!(tier, ComplexityTier::Complex) {
                                    base_config_clone.model = complex_model!();
                                }
                                // Mirror the per-run budget onto the orchestrated path's clone too —
                                // spawn_orchestrated_turn builds a disposable Orchestrator from this
                                // clone. (The approval policy is not mirrored: the spawned task reads
                                // it from the AgentHandle, which got the /permit changes.)
                                pending_gov.clone().apply_to_budget(&mut base_config_clone.budget_limit);
                                // An inline @agent's tool list narrows the run's tools too, so
                                // its workers never have a tool the persona's turn doesn't.
                                let base_config_clone =
                                    narrow_run_config_for_persona(base_config_clone, &persona_tools);
                                let agent_tx_clone = agent_tx.clone();
                                let original_prompt = base_config.system_prompt.clone();
                                let base_tools = base_config.tools.clone();
                                let restore_prompt = maybe_inline_agent.is_some();

                                let task_handle: tokio::task::JoinHandle<()> = match tier {
                                    ComplexityTier::Simple => {
                                        tracing::debug!("complexity: Simple — direct agent turn");
                                        if restore_prompt {
                                            let restore_tools = applied_tool_filter;
                                            {
                                                let (tx2, rx2) = mpsc::channel(256);
                                                let inner = spawn_agent_turn(agent_clone.clone(), effective_text_clone, tx2);
                                                spawn_inline_turn_wrapper(inner, rx2, agent_tx_clone, async move {
                                                    agent_clone.set_system_prompt(original_prompt).await;
                                                    if restore_tools {
                                                        agent_clone.set_tools(base_tools).await;
                                                    }
                                                })
                                            }
                                        } else {
                                            spawn_agent_turn(agent.clone(), effective_text, agent_tx.clone())
                                        }
                                    }
                                    ComplexityTier::Medium => {
                                        tracing::debug!("complexity: Medium — self-refine turn");
                                        if restore_prompt {
                                            let restore_tools = applied_tool_filter;
                                            {
                                                let (tx2, rx2) = mpsc::channel(256);
                                                let inner = spawn_medium_turn(agent_clone.clone(), effective_text_clone, tx2);
                                                spawn_inline_turn_wrapper(inner, rx2, agent_tx_clone, async move {
                                                    agent_clone.set_system_prompt(original_prompt).await;
                                                    if restore_tools {
                                                        agent_clone.set_tools(base_tools).await;
                                                    }
                                                })
                                            }
                                        } else {
                                            spawn_medium_turn(agent.clone(), effective_text, agent_tx.clone())
                                        }
                                    }
                                    ComplexityTier::Complex => {
                                        tracing::debug!("complexity: Complex — orchestrated turn");
                                        if restore_prompt {
                                            let restore_tools = applied_tool_filter;
                                            {
                                                let (tx2, rx2) = mpsc::channel(256);
                                                let inner = spawn_orchestrated_turn(base_config_clone, effective_text_clone, tx2, agent_clone.clone(), None);
                                                spawn_inline_turn_wrapper(inner, rx2, agent_tx_clone, async move {
                                                    agent_clone.set_system_prompt(original_prompt).await;
                                                    if restore_tools {
                                                        agent_clone.set_tools(base_tools).await;
                                                    }
                                                })
                                            }
                                        } else {
                                            spawn_orchestrated_turn(base_config_clone, effective_text, agent_tx.clone(), agent.clone(), None)
                                        }
                                    }
                                };
                                last_turn_task = Some(task_handle.abort_handle());
                                current_task_abort = Some(task_handle.abort_handle());

                                // After the turn is dispatched, clear the mode label
                                // if we had an inline agent (the spawned task restores
                                // the prompt when the turn finishes).
                                if restore_prompt {
                                    // Mode will be reset to default once the turn
                                    // finishes; for now show the agent label.
                                }
                            }
                        } else {
                            app.handle_key(key);
                        }

                        // `/plan <prompt>`: decompose in the background. Drained after
                        // every key event, so the overlay's keys act at once.
                        if let Some(plan_prompt) = app.take_pending_plan_prompt() {
                            // The previous turn's task may outlive its `Done`.
                            if last_turn_task.as_ref().is_some_and(|t| !t.is_finished()) {
                                app.refuse_plan();
                            } else {
                                // /plan decomposes with the model a Complex turn would use.
                                let mut plan_config = base_config.clone();
                                plan_config.model = complex_model!();
                                let working_dir = app.working_dir().to_path_buf();
                                let id = next_planning_id;
                                next_planning_id += 1;
                                planning_id = Some(id);
                                let tx = plan_tx.clone();
                                let watcher_tx = tx.clone();
                                let task = tokio::spawn(async move {
                                    let result = dispatch_plan_decompose(plan_config, working_dir, &plan_prompt)
                                        .await
                                        .map(|(plan, cost, _usage)| (plan, cost))
                                        .map_err(|e| e.to_string());
                                    let _ = tx.send((id, plan_prompt, result)).await;
                                });
                                // Esc cancels planning through the usual cancel path.
                                current_task_abort = Some(task.abort_handle());
                                // A task that ends without sending (a panic) must not
                                // leave the spinner running. An aborted one carries a
                                // stale id, so its message is dropped.
                                tokio::spawn(async move {
                                    if task.await.is_err() {
                                        let _ = watcher_tx
                                            .send((id, String::new(), Err("planning stopped unexpectedly".to_owned())))
                                            .await;
                                    }
                                });
                            }
                        }

                        // Run confirmed on the plan overlay: run the plan shown, with
                        // the settings of a Complex turn.
                        if let Some(preview) = app.take_pending_plan_run() {
                            if let Some((new_model, model)) = deferred_model.take() {
                                apply_model_switch!(new_model, model);
                            }
                            let complex = complex_model!();
                            current_turn_model = complex.to_string();
                            let mut run_config = base_config.clone();
                            run_config.model = complex.clone();
                            if complex.as_str() != shown_model {
                                shown_model = complex.to_string();
                                app.status_mut().model = shown_model.clone();
                                app.status_mut().context_limit = model_context_limit(complex.as_str());
                            }
                            let pending_gov = drain_governance!();
                            pending_gov.clone().apply_to_budget(&mut run_config.budget_limit);
                            // The classifier didn't pick this tier: it records nothing.
                            current_tier = None;
                            turn_fallback_model = None;
                            current_turn_prompt = preview.prompt.chars().take(200).collect::<String>();
                            let task = spawn_orchestrated_turn(
                                run_config,
                                preview.prompt,
                                agent_tx.clone(),
                                agent.clone(),
                                Some(preview.plan),
                            );
                            last_turn_task = Some(task.abort_handle());
                            current_task_abort = Some(task.abort_handle());
                        }

                        // Escape pressed during an AI turn — abort the task.
                        // Only act if there is actually a task to abort; without
                        // this guard, Escape during a shell escape would fire
                        // cancel_turn() spuriously and discard the shell output.
                        // `x` in the approval menu also asks to deny the paused call.
                        let deny_paused = app.take_pending_deny_paused();
                        if app.take_pending_cancel() {
                            if let Some(abort) = current_task_abort.take() {
                                abort.abort();
                                // Events the stopped run queued (e.g. another
                                // worker's approval prompt) must not reach the
                                // UI: they would open a menu for a dead run.
                                drain_after_cancel(&mut app, &mut agent_rx, step_tx.as_ref());
                                // A planning result arriving later is dropped.
                                planning_id = None;
                                app.cancel_turn();
                            }
                        }
                        // Esc while idle with an empty input box: stop the previous
                        // turn's task (the post-turn test run and any fix turns).
                        // Aborting drops the test run's command future, which kills
                        // the command. Only when idle, so a turn the user just
                        // started is never aborted by it.
                        if app.take_pending_stop_background()
                            && matches!(app.state(), AppState::Idle)
                        {
                            if let Some(task) = last_turn_task.take() {
                                task.abort();
                            }
                        }
                        // After the abort: a denied worker cannot start another
                        // call first, and a pause of another task is answered too.
                        if deny_paused {
                            if let Some(tx) = &step_tx {
                                let _ = tx.try_send(hone_proto::StepAction::Deny(
                                    "turn stopped by user".to_owned(),
                                ));
                            }
                        }

                        if app.should_quit() {
                            break;
                        }
                    }
                    Some(Ok(Event::Paste(text))) => {
                        app.handle_paste(&text);
                    }
                    Some(Ok(Event::Mouse(mouse_event))) => {
                        app.handle_mouse(mouse_event);
                    }
                    Some(Ok(Event::Resize(_, _))) => {
                        // Terminal resized; the next draw will pick up the new size.
                    }
                    Some(Err(_)) | None => {
                        // Stream error or closed — exit gracefully.
                        break;
                    }
                    _ => {}
                }
            }

            // Agent events from the provider task
            maybe_agent = agent_rx.recv() => {
                if let Some(event) = maybe_agent {
                    note_turn_fallback(&event, &mut turn_fallback_model);
                    record_outcome_for_event(&event, &adaptive, &mut current_tier, turn_fallback_model.as_deref().unwrap_or(current_turn_model.as_str()), base_config.pricing.input_per_million, base_config.pricing.output_per_million, &current_turn_prompt);
                    end_turn_fallback(&event, &mut turn_fallback_model);
                    app.handle_agent_event(event);
                    // When a turn completes, clear the abort handle and restore
                    // any follow-up message the user typed while waiting.
                    if matches!(app.state(), AppState::Idle | AppState::Error(_)) {
                        current_task_abort = None;
                        app.restore_queued_message_if_idle();
                    }
                }
            }

            // Result of the background `/plan` decompose
            Some((id, plan_prompt, result)) = plan_rx.recv() => {
                if planning_id == Some(id) {
                    planning_id = None;
                    match result {
                        Ok((task_plan, lead_cost)) => {
                            app.set_plan_preview(hone_tui::plan_preview::PlanPreviewState::from_decompose(
                                &plan_prompt,
                                task_plan,
                                lead_cost,
                            ));
                        }
                        Err(e) => {
                            tracing::warn!("/plan decompose failed: {e}");
                            app.plan_failed(&e);
                        }
                    }
                }
            }

            // Tick timer for animations
            _ = tokio::time::sleep(tick) => {}

            // OS-level SIGINT (e.g. from a parent process or wrapper script).
            _ = os_signal(&mut sigint) => {
                tracing::info!("SIGINT received, shutting down");
                break;
            }

            // SIGTERM / SIGHUP (e.g. the terminal closed).
            _ = os_term_signal(&mut term_signals) => {
                tracing::info!("SIGTERM/SIGHUP received, shutting down");
                break;
            }
        }

        // Drain any remaining agent events that arrived during the select branch.
        while let Ok(event) = agent_rx.try_recv() {
            note_turn_fallback(&event, &mut turn_fallback_model);
            record_outcome_for_event(
                &event,
                &adaptive,
                &mut current_tier,
                turn_fallback_model
                    .as_deref()
                    .unwrap_or(current_turn_model.as_str()),
                base_config.pricing.input_per_million,
                base_config.pricing.output_per_million,
                &current_turn_prompt,
            );
            end_turn_fallback(&event, &mut turn_fallback_model);
            app.handle_agent_event(event);
        }
        // Mirror the post-select restore logic for the drain path: if the drain
        // loop caused a transition to Idle or Error, clear the abort handle and
        // restore any queued follow-up message.
        if matches!(app.state(), AppState::Idle | AppState::Error(_))
            && current_task_abort.is_some()
        {
            current_task_abort = None;
            app.restore_queued_message_if_idle();
        }

        // Handle pending model changes from /model or /configure model. This runs on
        // every loop iteration, after any key handling, so a `/model` typed during a
        // turn is checked and answered at once. While a turn is running (the app is
        // not idle, or its task has not finished) an accepted change is only stored
        // and applied when the turn ends; the last accepted one wins.
        let turn_busy = !matches!(app.state(), AppState::Idle | AppState::Error(_))
            || last_turn_task.as_ref().is_some_and(|t| !t.is_finished());
        let mut to_apply: Option<(String, ModelId)> = None;
        if let Some(new_model) = app.take_pending_model_change() {
            // In an API session `/model` goes through the same check as
            // everything else: an API prefix whose provider is configured.
            let checked = match &routing {
                Some(r) => check_model_switch(&new_model, &|m| r.dispatch.has_provider_for(m)),
                None => Ok(ModelId::new(&new_model)),
            };
            match checked {
                Ok(model) if turn_busy => {
                    app.push_system_message(format!(
                        "Model will change to {new_model} when the current turn ends"
                    ));
                    deferred_model = Some((new_model, model));
                }
                Ok(model) => {
                    deferred_model = None;
                    to_apply = Some((new_model, model));
                }
                Err(msg) => {
                    // Keep the current model and say why.
                    app.push_error(msg);
                }
            }
        }
        // The turn ended: apply the change that was waiting for it.
        if to_apply.is_none() && !turn_busy {
            to_apply = deferred_model.take();
        }
        if let Some((new_model, model)) = to_apply {
            apply_model_switch!(new_model, model);
        }

        // Refresh context-window stats in the status bar.
        // context_tokens() reads an atomic snapshot — never blocks, safe during
        // long-running tool calls.
        app.status_mut().context_tokens = agent.context_tokens().await;

        app.tick();
    }

    // Persist any in-progress input for next session.
    app.save_draft();

    // If the user selected a session to switch to via Ctrl+O, print a resume hint.
    if let Some(session_name) = app.pending_session_switch.as_deref() {
        eprintln!(
            "\nTo resume session '{}': hone session resume {}",
            session_name, session_name
        );
    }

    // Generate and write the session report on exit.
    write_session_report(
        &app,
        model_id.as_str(),
        report::SessionMode::Interactive,
        None, // session name from app if needed
    );

    Ok(())
}

/// Aggregate turn ledgers from the App and write a session report to disk.
///
/// Errors are logged as warnings so a report failure never prevents the
/// clean exit path from completing.
fn write_session_report(
    app: &App,
    model: &str,
    mode: report::SessionMode,
    session_name: Option<&str>,
) {
    let ledgers = app.completed_turns();
    if ledgers.is_empty() {
        return; // Nothing to report.
    }

    let session_id = session_name.unwrap_or("session").to_string();

    let status = app.status();
    let now = chrono_like_timestamp();
    let report_data = report::ReportData::from_ledgers(
        ledgers,
        session_id,
        mode,
        model,
        status.session_cost_usd,
        status.token_count as u64,
        0, // tokens_out not separately tracked
        0, // cache_read not separately tracked
        status.elapsed_secs,
        now,
        None,
        true, // partial since audit may not be enabled
    );

    // Print concise stdout summary.
    let no_color = std::env::var("NO_COLOR").is_ok();
    let summary = report::render_stdout(&report_data, no_color);
    println!("\n{summary}");

    // Write report files.
    let cwd = std::env::current_dir().unwrap_or_default();
    match report::write_report(&cwd, &report_data, report::DEFAULT_REPORT_RETENTION) {
        Ok(path) => {
            if !no_color {
                println!("  Report       {}", path.display());
            } else {
                println!("  Report       {}", path.display());
            }
        }
        Err(e) => {
            tracing::warn!("failed to write session report: {e}");
        }
    }
}

fn chrono_like_timestamp() -> String {
    // Simple RFC3339-like timestamp without pulling in the chrono crate.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Format as approximate UTC datetime (seconds since epoch → rough date).
    // This gives a sortable, filesystem-safe string.
    // We use the same approach as egress_log: best-effort; not perfect UTC.
    format!("{secs}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // ---- should_color_stderr / color helpers --------------------------------

    #[test]
    fn warn_style_no_color_env_suppresses_ansi() {
        // When NO_COLOR is set, the helpers must return plain text regardless
        // of whether stderr is a terminal (which it never is in tests).
        // SAFETY: single-threaded test; no other thread reads NO_COLOR here.
        unsafe { std::env::set_var("NO_COLOR", "1") };
        let result = warn_style("Warning:");
        unsafe { std::env::remove_var("NO_COLOR") };
        assert_eq!(result, "Warning:");
    }

    #[test]
    fn error_style_no_color_env_suppresses_ansi() {
        // SAFETY: single-threaded test; no other thread reads NO_COLOR here.
        unsafe { std::env::set_var("NO_COLOR", "") };
        let result = error_style("Error:");
        unsafe { std::env::remove_var("NO_COLOR") };
        assert_eq!(result, "Error:");
    }

    #[test]
    fn bold_style_no_color_env_suppresses_ansi() {
        // SAFETY: single-threaded test; no other thread reads NO_COLOR here.
        unsafe { std::env::set_var("NO_COLOR", "1") };
        let result = bold_style("bold text");
        unsafe { std::env::remove_var("NO_COLOR") };
        assert_eq!(result, "bold text");
    }

    // Since stderr is not a terminal in the test runner, should_color_stderr()
    // returns false even without NO_COLOR.  Verify the helpers also return
    // plain text in that case (no env override needed).
    #[test]
    fn warn_style_non_terminal_stderr_suppresses_ansi() {
        // Ensure NO_COLOR is absent so the only gate is is_terminal().
        // SAFETY: single-threaded test; no other thread reads NO_COLOR here.
        unsafe { std::env::remove_var("NO_COLOR") };
        let result = warn_style("Warning:");
        // In a test runner stderr is not a TTY, so no escapes expected.
        assert!(!result.contains('\x1b'));
    }

    // ---- ensemble validation ------------------------------------------------

    /// Simulate the validation logic extracted from `main` to make it unit-testable.
    fn validate_ensemble(n: u32) -> Result<(), String> {
        if n < 2 || n > 10 {
            Err(format!(
                "--ensemble value must be between 2 and 10 (got {n})"
            ))
        } else {
            Ok(())
        }
    }

    #[test]
    fn ensemble_rejects_zero() {
        assert!(validate_ensemble(0).is_err());
    }

    #[test]
    fn ensemble_rejects_one() {
        assert!(validate_ensemble(1).is_err());
    }

    #[test]
    fn ensemble_accepts_two() {
        assert!(validate_ensemble(2).is_ok());
    }

    #[test]
    fn ensemble_accepts_ten() {
        assert!(validate_ensemble(10).is_ok());
    }

    #[test]
    fn ensemble_rejects_eleven() {
        assert!(validate_ensemble(11).is_err());
    }

    #[test]
    fn ensemble_rejects_one_hundred() {
        assert!(validate_ensemble(100).is_err());
    }

    // ---- model ID validation ------------------------------------------------

    fn is_valid_model_id(m: &str) -> bool {
        let known_prefixes = [
            "anthropic:",
            "openai:",
            "ollama:",
            "claude-cli:",
            "gemini-cli:",
        ];
        known_prefixes.iter().any(|p| m.starts_with(p))
    }

    #[test]
    fn model_id_valid_anthropic() {
        assert!(is_valid_model_id("anthropic:claude-sonnet-4-5"));
    }

    #[test]
    fn model_id_valid_openai() {
        assert!(is_valid_model_id("openai:gpt-4o"));
    }

    #[test]
    fn model_id_valid_ollama() {
        assert!(is_valid_model_id("ollama:llama3.2"));
    }

    #[test]
    fn model_id_rejects_bare_name() {
        assert!(!is_valid_model_id("gpt-4o"));
    }

    #[test]
    fn model_id_rejects_unknown_prefix() {
        assert!(!is_valid_model_id("mistral:mistral-7b"));
    }

    // -------------------------------------------------------------------------
    // Regression: orchestrated turn must write conversation back to AgentHandle
    //
    // Bug: `spawn_orchestrated_turn` builds a disposable `Orchestrator` and
    // streams tokens to `tx`, but never calls `agent.load_history(...)`.  A
    // second Simple turn therefore receives an empty context and replies "I
    // don't have a request to act on."
    //
    // This test proves the bug by asserting the invariant that SHOULD hold
    // after the fix: after `spawn_orchestrated_turn` completes, the persistent
    // `AgentHandle`'s message log must contain a user message with the original
    // prompt text AND a non-empty assistant message with the orchestrator's
    // merged response.  The assertion currently fails because the writeback
    // does not exist yet.
    // -------------------------------------------------------------------------
    #[tokio::test]
    async fn orchestrated_turn_must_write_back_context_to_agent_handle() {
        use hone_core::RetryConfig;
        use hone_proto::Role;
        use hone_providers::MockProvider;
        use hone_sandbox::NullSandbox;

        // The orchestrator makes at least two LLM calls: one to decompose the
        // request into a TaskPlan (expects JSON), one to merge worker results.
        // MockProvider repeats the same token sequence on every call, so we
        // use a minimal but valid JSON plan that it can parse for decompose
        // and that also serves as a harmless merge response.
        let plan_json = r#"{"goal":"do X please","subtasks":[{"id":"1","description":"do X please","agent":"debugger","context":"","depends_on":[]}]}"#;
        let provider = Arc::new(MockProvider::new(vec![plan_json.to_owned()]));

        let config = AgentConfig {
            provider: Arc::clone(&provider) as Arc<dyn hone_providers::ModelProvider>,
            tools: ToolRegistry::new(),
            sandbox: Arc::new(NullSandbox),
            sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
            system_prompt: "You are a lead orchestrator.".to_owned(),
            model: ModelId::new("mock:test"),
            max_tokens: None,
            temperature: None,
            budget_limit: None,
            pricing: PricingEntry {
                model: ModelId::new("mock:test"),
                input_per_million: 0.0,
                output_per_million: 0.0,
                cache_read_per_million: None,
                cache_creation_per_million: None,
            },
            working_dir: std::path::PathBuf::from("."),
            memory_store: None,
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
        };

        // Spawn the persistent agent that should accumulate context.
        let agent = AgentHandle::spawn(config.clone());

        // Run an orchestrated turn — today this creates a disposable
        // Orchestrator and never writes results back to `agent`.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);
        let _handle = spawn_orchestrated_turn(config, "do X please".to_string(), tx, agent.clone(), None);

        // Drain events until the sender is dropped (Orchestrator finished).
        while rx.recv().await.is_some() {}

        // Give the actor a scheduling tick to process any in-flight commands.
        tokio::task::yield_now().await;

        // After the orchestrated turn finishes, the persistent agent MUST hold
        // both the user prompt and the merged assistant response in its context.
        let messages = agent.get_messages().await;

        let has_user_msg = messages
            .iter()
            .any(|m| m.role == Role::User && m.content.as_text().contains("do X please"));

        let has_assistant_msg = messages
            .iter()
            .any(|m| m.role == Role::Assistant && !m.content.as_text().is_empty());

        // These assertions fail today because spawn_orchestrated_turn never
        // calls agent.load_history(...).  They will pass once the fix is
        // applied.
        assert!(
            has_user_msg,
            "persistent agent context is missing the user message; \
             spawn_orchestrated_turn does not write back to AgentHandle. \
             messages in context: {messages:?}"
        );
        assert!(
            has_assistant_msg,
            "persistent agent context is missing the assistant response; \
             spawn_orchestrated_turn does not write back to AgentHandle. \
             messages in context: {messages:?}"
        );
    }

    // ---- /plan gate G7 + G14 (docs/plans/plan-gate-spec.md) ----

    fn orch_config(
        provider: Arc<dyn hone_providers::ModelProvider>,
        dir: &std::path::Path,
    ) -> AgentConfig {
        use hone_core::RetryConfig;
        use hone_sandbox::NullSandbox;
        AgentConfig {
            provider,
            tools: ToolRegistry::new(),
            sandbox: Arc::new(NullSandbox),
            sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
            system_prompt: "You are a lead orchestrator.".to_owned(),
            model: ModelId::new("mock:test"),
            max_tokens: None,
            temperature: None,
            budget_limit: None,
            pricing: PricingEntry {
                model: ModelId::new("mock:test"),
                input_per_million: 1_000_000.0,
                output_per_million: 1_000_000.0,
                cache_read_per_million: None,
                cache_creation_per_million: None,
            },
            working_dir: dir.to_path_buf(),
            memory_store: None,
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
        }
    }

    /// Answers "ok" with usage 10 in / 5 out on every call; records each prompt.
    struct UsageRecorder {
        prompts: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl hone_providers::ModelProvider for UsageRecorder {
        fn id(&self) -> &str {
            "usage-recorder"
        }
        fn display_name(&self) -> &str {
            "Usage recorder"
        }
        fn supported_models(&self) -> Vec<hone_proto::ModelInfo> {
            vec![]
        }
        async fn complete(
            &self,
            req: hone_providers::CompletionRequest,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = hone_proto::ProviderEvent> + Send>>,
            hone_providers::ProviderError,
        > {
            let text = req
                .messages
                .iter()
                .map(|m| m.content.as_text())
                .collect::<Vec<_>>()
                .join("\n");
            self.prompts.lock().unwrap().push(text);
            Ok(Box::pin(futures::stream::iter(vec![
                hone_proto::ProviderEvent::Token {
                    text: "ok".to_owned(),
                },
                hone_proto::ProviderEvent::Usage(hone_proto::TokenUsage {
                    input: 10,
                    output: 5,
                    cache_read: 0,
                    cache_creation: 0,
                }),
                hone_proto::ProviderEvent::Done,
            ])))
        }
        fn estimate_tokens(&self, _: &[hone_proto::Message]) -> u32 {
            1
        }
        async fn health(&self) -> hone_proto::ProviderHealth {
            hone_proto::ProviderHealth::Healthy
        }
    }

    /// Every call fails with a non-retryable HTTP error.
    struct AlwaysFails;

    #[async_trait::async_trait]
    impl hone_providers::ModelProvider for AlwaysFails {
        fn id(&self) -> &str {
            "always-fails"
        }
        fn display_name(&self) -> &str {
            "Always fails"
        }
        fn supported_models(&self) -> Vec<hone_proto::ModelInfo> {
            vec![]
        }
        async fn complete(
            &self,
            _req: hone_providers::CompletionRequest,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = hone_proto::ProviderEvent> + Send>>,
            hone_providers::ProviderError,
        > {
            Err(hone_providers::ProviderError::Http {
                provider: "always-fails".to_owned(),
                status: 400,
                body: "bad request".to_owned(),
            })
        }
        fn estimate_tokens(&self, _: &[hone_proto::Message]) -> u32 {
            1
        }
        async fn health(&self) -> hone_proto::ProviderHealth {
            hone_proto::ProviderHealth::Healthy
        }
    }

    fn one_task_plan() -> hone_core::TaskPlan {
        hone_core::TaskPlan {
            goal: "ship the shown plan".to_owned(),
            subtasks: vec![hone_core::SubTask {
                id: "1".to_owned(),
                description: "do the one thing".to_owned(),
                agent: "debugger".to_owned(),
                context: String::new(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
                requires_capabilities: vec![],
            }],
        }
    }

    async fn collect_events(rx: &mut tokio::sync::mpsc::Receiver<AgentEvent>) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        while let Some(e) = rx.recv().await {
            out.push(e);
        }
        out
    }

    #[tokio::test]
    async fn g7_orchestrated_turn_with_a_plan_does_not_decompose_and_writes_history_back() {
        use hone_proto::Role;
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(UsageRecorder {
            prompts: std::sync::Mutex::new(Vec::new()),
        });
        let config = orch_config(Arc::clone(&provider) as _, dir.path());
        let agent = AgentHandle::spawn(config.clone());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_string(),
            tx,
            agent.clone(),
            Some(one_task_plan()),
        );
        let _ = collect_events(&mut rx).await;
        tokio::task::yield_now().await;

        let prompts = provider.prompts.lock().unwrap().clone();
        assert!(
            !prompts
                .iter()
                .any(|p| p.contains("You are a lead agent. Decompose")),
            "a staged plan must not be decomposed again: {prompts:?}"
        );
        let messages = agent.get_messages().await;
        assert!(
            messages
                .iter()
                .any(|m| m.role == Role::User && m.content.as_text().contains("ship it")),
            "{messages:?}"
        );
        assert!(
            messages
                .iter()
                .any(|m| m.role == Role::Assistant && m.content.as_text().contains("ok")),
            "{messages:?}"
        );
    }

    #[tokio::test]
    async fn g14_orchestrated_turn_ends_with_exactly_one_done_after_the_summary() {
        let dir = tempfile::tempdir().unwrap();
        let provider = Arc::new(UsageRecorder {
            prompts: std::sync::Mutex::new(Vec::new()),
        });
        let config = orch_config(provider as _, dir.path());
        let agent = AgentHandle::spawn(config.clone());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_string(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = collect_events(&mut rx).await;

        let dones: Vec<usize> = events
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, AgentEvent::Done { .. }))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(dones.len(), 1, "exactly one Done: {events:?}");
        assert_eq!(dones[0], events.len() - 1, "Done is the last event: {events:?}");
        match &events[events.len() - 2] {
            AgentEvent::Token { text } => {
                assert!(text.contains("Orchestration complete"), "{text}")
            }
            other => panic!("summary Token must precede Done, got {other:?}"),
        }
        assert!(
            !events.iter().any(|e| matches!(e, AgentEvent::Error { .. })),
            "inner errors are not forwarded: {events:?}"
        );
        // Worker + merge ran (2 inner turns, 10 in / 5 out each); the final
        // Done carries their sums. Priced at 1 USD per token.
        match events.last().unwrap() {
            AgentEvent::Done {
                usage,
                cost_usd,
                plan,
            } => {
                assert!(plan.is_none());
                assert_eq!(usage.input, 20, "{usage:?}");
                assert_eq!(usage.output, 10, "{usage:?}");
                assert!((cost_usd - 30.0).abs() < 1e-6, "cost {cost_usd}");
            }
            _ => unreachable!(),
        }
    }

    /// The first call fails (HTTP 400); every later call answers "ok".
    struct FirstCallFails {
        n: std::sync::Mutex<usize>,
    }

    #[async_trait::async_trait]
    impl hone_providers::ModelProvider for FirstCallFails {
        fn id(&self) -> &str {
            "first-call-fails"
        }
        fn display_name(&self) -> &str {
            "First call fails"
        }
        fn supported_models(&self) -> Vec<hone_proto::ModelInfo> {
            vec![]
        }
        async fn complete(
            &self,
            _req: hone_providers::CompletionRequest,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = hone_proto::ProviderEvent> + Send>>,
            hone_providers::ProviderError,
        > {
            let first = {
                let mut n = self.n.lock().unwrap();
                *n += 1;
                *n == 1
            };
            if first {
                return Err(hone_providers::ProviderError::Http {
                    provider: "first-call-fails".to_owned(),
                    status: 400,
                    body: "bad request".to_owned(),
                });
            }
            Ok(Box::pin(futures::stream::iter(vec![
                hone_proto::ProviderEvent::Token {
                    text: "ok".to_owned(),
                },
                hone_proto::ProviderEvent::Done,
            ])))
        }
        fn estimate_tokens(&self, _: &[hone_proto::Message]) -> u32 {
            1
        }
        async fn health(&self) -> hone_proto::ProviderHealth {
            hone_proto::ProviderHealth::Healthy
        }
    }

    #[tokio::test]
    async fn g22_a_failed_worker_is_forwarded_and_the_run_still_ends_with_one_done() {
        let dir = tempfile::tempdir().unwrap();
        // With a staged plan the worker is the first provider call; it fails,
        // the merge call succeeds.
        let config = orch_config(
            Arc::new(FirstCallFails {
                n: std::sync::Mutex::new(0),
            }),
            dir.path(),
        );
        let agent = AgentHandle::spawn(config.clone());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_string(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = collect_events(&mut rx).await;

        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentEvent::WorkerCompleted { status, .. } if status.starts_with("failed")
            )),
            "the failed WorkerCompleted must be forwarded: {events:?}"
        );
        assert!(
            !events.iter().any(|e| matches!(e, AgentEvent::Error { .. })),
            "no Error: {events:?}"
        );
        let dones = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Done { .. }))
            .count();
        assert_eq!(dones, 1, "exactly one Done: {events:?}");
        assert!(
            matches!(events.last(), Some(AgentEvent::Done { .. })),
            "Done is last: {events:?}"
        );
    }

    #[tokio::test]
    async fn g14_failed_orchestration_sends_exactly_one_error_and_no_done() {
        let dir = tempfile::tempdir().unwrap();
        let config = orch_config(Arc::new(AlwaysFails), dir.path());
        let agent = AgentHandle::spawn(config.clone());
        let (tx, mut rx) = tokio::sync::mpsc::channel::<AgentEvent>(256);

        // No staged plan: the decompose call hits the failing provider.
        let _h = spawn_orchestrated_turn(config, "ship it".to_string(), tx, agent, None);
        let events = collect_events(&mut rx).await;

        let errors: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Error { .. }))
            .collect();
        assert_eq!(errors.len(), 1, "exactly one Error: {events:?}");
        match errors[0] {
            AgentEvent::Error { message, .. } => {
                assert!(message.starts_with("orchestration failed:"), "{message}")
            }
            _ => unreachable!(),
        }
        assert!(
            !events.iter().any(|e| matches!(e, AgentEvent::Done { .. })),
            "no Done on failure: {events:?}"
        );
    }

    // ---- TUI approval wiring (docs/plans/tui-approval-fix-spec.md rows 6-8) ----

    /// Call 1: one tool call. Call 2+: text "done". (`MockProvider` repeats its tool
    /// call on every request, which would loop after a denial, so it is not used here.)
    struct OneCallThenDone {
        tool: String,
        n: std::sync::Mutex<usize>,
    }

    #[async_trait::async_trait]
    impl hone_providers::ModelProvider for OneCallThenDone {
        fn id(&self) -> &str {
            "one-call"
        }
        fn display_name(&self) -> &str {
            "One call"
        }
        fn supported_models(&self) -> Vec<hone_proto::ModelInfo> {
            vec![hone_proto::ModelInfo {
                id: ModelId::new("mock:test"),
                display_name: "Test".to_owned(),
                context_length: 4096,
                supports_tools: true,
                supports_streaming: true,
            }]
        }
        async fn complete(
            &self,
            _req: hone_providers::CompletionRequest,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = hone_proto::ProviderEvent> + Send>>,
            hone_providers::ProviderError,
        > {
            let i = {
                let mut n = self.n.lock().unwrap();
                let i = *n;
                *n += 1;
                i
            };
            let events = if i == 0 {
                vec![
                    hone_proto::ProviderEvent::ToolCall(hone_proto::ToolCallRequest {
                        id: "c1".to_owned(),
                        name: self.tool.clone(),
                        arguments: serde_json::json!({}),
                    }),
                    hone_proto::ProviderEvent::Done,
                ]
            } else {
                vec![
                    hone_proto::ProviderEvent::Token {
                        text: "done".to_owned(),
                    },
                    hone_proto::ProviderEvent::Done,
                ]
            };
            Ok(Box::pin(futures::stream::iter(events)))
        }
        fn estimate_tokens(&self, _m: &[Message]) -> u32 {
            5
        }
        async fn health(&self) -> hone_proto::ProviderHealth {
            hone_proto::ProviderHealth::Healthy
        }
    }

    /// Marker-file tool; the name is deliberately not one of the code-modifying
    /// tools that trigger post-turn test verification.
    struct MarkerTool {
        name: &'static str,
        read_only: bool,
        target: PathBuf,
    }

    #[async_trait::async_trait]
    impl hone_tools::Tool for MarkerTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "writes a marker file"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        async fn call(
            &self,
            _input: hone_tools::ToolInput,
            _ctx: &hone_tools::ToolContext,
        ) -> Result<hone_tools::ToolOutput, hone_tools::ToolError> {
            std::fs::write(&self.target, b"written").unwrap();
            Ok(hone_tools::ToolOutput::new(serde_json::json!({ "ok": true })))
        }
        fn layer(&self) -> hone_proto::ToolLayer {
            hone_proto::ToolLayer::Native
        }
        fn is_read_only(&self) -> bool {
            self.read_only
        }
    }

    /// An `Auto`, no-channel config exactly like `build_agent_config` produces.
    fn tui_wiring_config(tool: MarkerTool, dir: &std::path::Path) -> AgentConfig {
        let name = tool.name.to_owned();
        AgentConfig {
            provider: Arc::new(OneCallThenDone {
                tool: name,
                n: std::sync::Mutex::new(0),
            }),
            tools: ToolRegistry::builder().register(Box::new(tool)).build(),
            sandbox: Arc::new(hone_sandbox::NullSandbox),
            sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
            system_prompt: "test".to_owned(),
            model: ModelId::new("mock:test"),
            max_tokens: None,
            temperature: None,
            budget_limit: None,
            pricing: PricingEntry {
                model: ModelId::new("mock:test"),
                input_per_million: 0.0,
                output_per_million: 0.0,
                cache_read_per_million: None,
                cache_creation_per_million: None,
            },
            working_dir: dir.to_path_buf(),
            memory_store: None,
            retry_config: hone_core::RetryConfig::default(),
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
        }
    }

    /// Run a simple turn through `spawn_agent_turn` (what the TUI uses), answering
    /// each `ToolCallPaused` with the next reply. Returns all events.
    async fn drive_simple_turn(
        agent: &AgentHandle,
        step_tx: &mpsc::Sender<hone_proto::StepAction>,
        mut replies: Vec<hone_proto::StepAction>,
    ) -> Vec<AgentEvent> {
        replies.reverse();
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let _task = spawn_agent_turn(agent.clone(), "go".to_owned(), tx);
        let mut events = Vec::new();
        let collect = async {
            while let Some(e) = rx.recv().await {
                if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                    let reply = replies
                        .pop()
                        .unwrap_or_else(|| panic!("unexpected extra pause: {events:?}"));
                    step_tx.send(reply).await.unwrap();
                }
                let terminal = matches!(e, AgentEvent::Done { .. } | AgentEvent::Error { .. });
                events.push(e);
                if terminal {
                    break;
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(10), collect)
            .await
            .expect("turn hung: no pause where one was expected");
        events
    }

    fn paused_count(events: &[AgentEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCallPaused { .. }))
            .count()
    }

    // Spec row 6.
    #[tokio::test]
    async fn test_wire_tui_approvals_when_not_step_should_set_askrisky_everywhere_and_pause_simple_turn()
     {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.txt");
        let tool = MarkerTool {
            name: "mock_write",
            read_only: false,
            target: target.clone(),
        };
        let mut base_config = tui_wiring_config(tool, tmp.path());
        let agent = AgentHandle::spawn(base_config.clone());

        let step_tx = wire_tui_approvals(&agent, &mut base_config, false).await;

        assert!(matches!(
            base_config.approval_policy,
            ToolApprovalPolicy::AskRisky { ref always_allowed } if always_allowed.is_empty()
        ));
        assert!(base_config.step_rx.is_some(), "base_config must carry the channel so clones stay consistent with the running agent");
        assert!(matches!(
            agent.approval_policy().await,
            ToolApprovalPolicy::AskRisky { ref always_allowed } if always_allowed.is_empty()
        ));

        let events = drive_simple_turn(
            &agent,
            &step_tx,
            vec![hone_proto::StepAction::Deny("nope".to_owned())],
        )
        .await;

        assert_eq!(paused_count(&events), 1, "{events:?}");
        assert!(!target.exists(), "denied write must not happen");
    }

    // Spec row 7.
    #[tokio::test]
    async fn test_wire_tui_approvals_when_step_should_set_alwaysask_and_pause_on_read_only_tool() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.txt");
        let tool = MarkerTool {
            name: "mock_read",
            read_only: true,
            target: target.clone(),
        };
        let mut base_config = tui_wiring_config(tool, tmp.path());
        let agent = AgentHandle::spawn(base_config.clone());

        let step_tx = wire_tui_approvals(&agent, &mut base_config, true).await;

        assert!(matches!(
            base_config.approval_policy,
            ToolApprovalPolicy::AlwaysAsk
        ));
        assert!(matches!(
            agent.approval_policy().await,
            ToolApprovalPolicy::AlwaysAsk
        ));

        let events = drive_simple_turn(
            &agent,
            &step_tx,
            vec![hone_proto::StepAction::Deny("nope".to_owned())],
        )
        .await;

        assert_eq!(paused_count(&events), 1, "read-only tool must pause under --step: {events:?}");
        assert!(!target.exists());
    }

    // Spec row 8.
    #[tokio::test]
    async fn test_wire_tui_approvals_when_history_loaded_should_keep_history_in_running_agent() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = MarkerTool {
            name: "mock_write",
            read_only: false,
            target: tmp.path().join("out.txt"),
        };
        let mut base_config = tui_wiring_config(tool, tmp.path());
        let agent = AgentHandle::spawn(base_config.clone());
        agent
            .load_history(vec![
                Message::user("remember the blue heron"),
                Message::assistant("noted"),
            ])
            .await;

        let _step_tx = wire_tui_approvals(&agent, &mut base_config, false).await;

        let text: String = agent
            .get_messages()
            .await
            .iter()
            .map(|m| m.content.as_text())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            text.contains("remember the blue heron") && text.contains("noted"),
            "resumed history must survive wiring (no respawn); got: {text:?}"
        );
    }

    // ---- Review round 1 (spec section 6): R1, R2c, R6 --------------------------------------

    /// Provider that replays `script[n]` on the n-th `complete()` (the last entry repeats).
    struct SeqProvider {
        script: Vec<Vec<hone_proto::ProviderEvent>>,
        n: std::sync::Mutex<usize>,
    }

    #[async_trait::async_trait]
    impl hone_providers::ModelProvider for SeqProvider {
        fn id(&self) -> &str {
            "seq"
        }
        fn display_name(&self) -> &str {
            "Seq"
        }
        fn supported_models(&self) -> Vec<hone_proto::ModelInfo> {
            vec![hone_proto::ModelInfo {
                id: ModelId::new("mock:test"),
                display_name: "Test".to_owned(),
                context_length: 4096,
                supports_tools: true,
                supports_streaming: true,
            }]
        }
        async fn complete(
            &self,
            _req: hone_providers::CompletionRequest,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = hone_proto::ProviderEvent> + Send>>,
            hone_providers::ProviderError,
        > {
            let i = {
                let mut n = self.n.lock().unwrap();
                let i = (*n).min(self.script.len() - 1);
                *n += 1;
                i
            };
            Ok(Box::pin(futures::stream::iter(self.script[i].clone())))
        }
        fn estimate_tokens(&self, _m: &[Message]) -> u32 {
            5
        }
        async fn health(&self) -> hone_proto::ProviderHealth {
            hone_proto::ProviderHealth::Healthy
        }
    }

    fn usage10() -> hone_proto::ProviderEvent {
        hone_proto::ProviderEvent::Usage(hone_proto::TokenUsage {
            input: 10,
            output: 0,
            cache_read: 0,
            cache_creation: 0,
        })
    }

    fn text_turn(t: &str) -> Vec<hone_proto::ProviderEvent> {
        vec![
            hone_proto::ProviderEvent::Token { text: t.to_owned() },
            usage10(),
            hone_proto::ProviderEvent::Done,
        ]
    }

    /// Wait (bounded) for the first `ToolCallPaused` on `rx`.
    async fn wait_for_pause(rx: &mut mpsc::Receiver<AgentEvent>) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(e) = rx.recv().await {
                if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                    return;
                }
            }
            panic!("stream ended before a pause");
        })
        .await
        .expect("never paused");
    }

    /// A fresh turn on `agent` must finish (not wedge on a dead prompt).
    async fn assert_agent_still_usable(agent: &AgentHandle) {
        let (tx, _rx) = mpsc::channel::<AgentEvent>(256);
        let r = tokio::time::timeout(Duration::from_secs(10), agent.run_turn("again", tx))
            .await
            .expect("agent actor is wedged: a second turn never completed");
        assert!(r.is_ok(), "{r:?}");
    }

    // R1: medium turns keep forwarding past the first Done and send exactly one final Done.
    #[tokio::test]
    async fn test_r1_medium_turn_should_forward_refine_pause_and_send_single_summed_done() {
        use hone_proto::ProviderEvent;
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.txt");
        let tool = MarkerTool {
            name: "mock_write",
            read_only: false,
            target: target.clone(),
        };
        let mut base_config = tui_wiring_config(tool, tmp.path());
        // 1 USD per input token so cost_usd mirrors input tokens.
        base_config.pricing.input_per_million = 1_000_000.0;
        base_config.provider = Arc::new(SeqProvider {
            script: vec![
                text_turn("first answer"),
                text_turn("the answer needs work"), // critique without APPROVED
                vec![
                    ProviderEvent::ToolCall(hone_proto::ToolCallRequest {
                        id: "c1".to_owned(),
                        name: "mock_write".to_owned(),
                        arguments: serde_json::json!({}),
                    }),
                    usage10(),
                    ProviderEvent::Done,
                ],
                text_turn("refined"),
            ],
            n: std::sync::Mutex::new(0),
        });
        let agent = AgentHandle::spawn(base_config.clone());
        let mut base_config = base_config;
        let step_tx = wire_tui_approvals(&agent, &mut base_config, false).await;

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let _task = spawn_medium_turn(agent.clone(), "go".to_owned(), tx);

        let mut events = Vec::new();
        let collect = async {
            while let Some(e) = rx.recv().await {
                if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                    step_tx.send(hone_proto::StepAction::Proceed).await.unwrap();
                }
                events.push(e);
            }
        };
        tokio::time::timeout(Duration::from_secs(15), collect)
            .await
            .expect("medium turn wedged: the refine-round pause never reached the UI");

        assert_eq!(paused_count(&events), 1, "{events:?}");
        assert!(target.exists(), "approved refine write must run");
        let dones: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Done { usage, cost_usd, .. } => Some((*usage, *cost_usd)),
                _ => None,
            })
            .collect();
        assert_eq!(dones.len(), 1, "exactly one terminal Done: {events:?}");
        assert!(
            matches!(events.last(), Some(AgentEvent::Done { .. })),
            "Done must be last"
        );
        let (usage, cost) = dones[0];
        assert_eq!(usage.input, 40, "usage summed over all 4 model calls");
        assert!((cost - 40.0).abs() < 0.01, "cost summed over rounds: {cost}");
    }

    // R2c: Esc (abort of the forwarder) while paused must not wedge the actor.
    #[tokio::test]
    async fn test_r2c_aborting_spawn_agent_turn_while_paused_should_leave_agent_usable() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.txt");
        let tool = MarkerTool {
            name: "mock_write",
            read_only: false,
            target: target.clone(),
        };
        let mut base_config = tui_wiring_config(tool, tmp.path());
        let agent = AgentHandle::spawn(base_config.clone());
        let _step_tx = wire_tui_approvals(&agent, &mut base_config, false).await;

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let task = spawn_agent_turn(agent.clone(), "go".to_owned(), tx);
        wait_for_pause(&mut rx).await;
        task.abort();

        assert_agent_still_usable(&agent).await;
        assert!(!target.exists(), "the cancelled write must never run");
    }

    // R2c: the UI side going away (outer receiver dropped) must also release the actor.
    #[tokio::test]
    async fn test_r2c_closed_outer_channel_should_stop_spawn_agent_turn_forwarding_and_free_actor() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.txt");
        let tool = MarkerTool {
            name: "mock_write",
            read_only: false,
            target: target.clone(),
        };
        let mut base_config = tui_wiring_config(tool, tmp.path());
        let agent = AgentHandle::spawn(base_config.clone());
        let _step_tx = wire_tui_approvals(&agent, &mut base_config, false).await;

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let task = spawn_agent_turn(agent.clone(), "go".to_owned(), tx);
        wait_for_pause(&mut rx).await;
        drop(rx);

        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("forwarder must stop once the outer channel is closed")
            .ok();
        assert_agent_still_usable(&agent).await;
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn test_r2c_closed_outer_channel_should_stop_spawn_medium_turn_forwarding_and_free_actor() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.txt");
        let tool = MarkerTool {
            name: "mock_write",
            read_only: false,
            target: target.clone(),
        };
        let mut base_config = tui_wiring_config(tool, tmp.path());
        let agent = AgentHandle::spawn(base_config.clone());
        let _step_tx = wire_tui_approvals(&agent, &mut base_config, false).await;

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let task = spawn_medium_turn(agent.clone(), "go".to_owned(), tx);
        wait_for_pause(&mut rx).await;
        drop(rx);

        tokio::time::timeout(Duration::from_secs(10), task)
            .await
            .expect("forwarder must stop once the outer channel is closed")
            .ok();
        assert_agent_still_usable(&agent).await;
        assert!(!target.exists());
    }

    // R2c, inline `@agent` path. Seam (free fn in main.rs, replaces the inline closures in
    // `run_tui`):
    //   fn spawn_inline_turn_wrapper<F>(
    //       inner: tokio::task::JoinHandle<()>,
    //       inner_rx: mpsc::Receiver<AgentEvent>,
    //       outer_tx: mpsc::Sender<AgentEvent>,
    //       after: F,
    //   ) -> tokio::task::JoinHandle<()>
    //   where F: std::future::Future<Output = ()> + Send + 'static
    // It forwards `inner_rx` to `outer_tx`, runs `after` when the inner stream ends normally,
    // and aborts `inner` if the wrapper itself is aborted or dropped (drop guard).
    #[tokio::test]
    async fn test_r2c_aborting_inline_wrapper_while_paused_should_abort_inner_and_free_actor() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.txt");
        let tool = MarkerTool {
            name: "mock_write",
            read_only: false,
            target: target.clone(),
        };
        let mut base_config = tui_wiring_config(tool, tmp.path());
        let agent = AgentHandle::spawn(base_config.clone());
        let _step_tx = wire_tui_approvals(&agent, &mut base_config, false).await;

        let (tx2, rx2) = mpsc::channel::<AgentEvent>(256);
        let inner = spawn_agent_turn(agent.clone(), "go".to_owned(), tx2);
        let inner_abort = inner.abort_handle();
        let (outer_tx, mut outer_rx) = mpsc::channel::<AgentEvent>(256);
        let wrapper = spawn_inline_turn_wrapper(inner, rx2, outer_tx, async {});
        wait_for_pause(&mut outer_rx).await;

        wrapper.abort();

        tokio::time::timeout(Duration::from_secs(10), async {
            while !inner_abort.is_finished() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("aborting the wrapper must abort the inner turn task");
        assert_agent_still_usable(&agent).await;
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn test_r2c_inline_wrapper_should_run_after_hook_when_turn_ends_normally() {
        let tmp = tempfile::tempdir().unwrap();
        let tool = MarkerTool {
            name: "mock_read",
            read_only: true,
            target: tmp.path().join("unused.txt"),
        };
        let base_config = tui_wiring_config(tool, tmp.path());
        let agent = AgentHandle::spawn(base_config);
        let (tx2, rx2) = mpsc::channel::<AgentEvent>(256);
        // Auto policy: the read-only tool just runs, the turn ends.
        let inner = spawn_agent_turn(agent.clone(), "go".to_owned(), tx2);
        let (outer_tx, mut outer_rx) = mpsc::channel::<AgentEvent>(256);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<()>();
        let wrapper = spawn_inline_turn_wrapper(inner, rx2, outer_tx, async move {
            let _ = done_tx.send(());
        });

        let mut saw_done = false;
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(e) = outer_rx.recv().await {
                saw_done |= matches!(e, AgentEvent::Done { .. });
            }
        })
        .await
        .expect("events must be forwarded to completion");
        tokio::time::timeout(Duration::from_secs(5), done_rx)
            .await
            .expect("`after` must run on normal completion")
            .unwrap();
        let _ = wrapper.await;
        assert!(saw_done);
    }

    // R10. Seam (free fn in main.rs, extracted from the `--orchestrate` printer task):
    //   async fn print_orchestrate_events<O: std::io::Write, E: std::io::Write>(
    //       rx: mpsc::Receiver<AgentEvent>, out: &mut O, err: &mut E)
    // It prints Tokens / plan / worker lines to `out`, `error: <msg>` to `err`, and keeps
    // draining until `recv()` returns `None`. It must NOT stop at an `Error`: dropping the
    // receiver makes every later worker and the merge fail with `Cancelled`.
    #[tokio::test]
    async fn test_r10_orchestrate_printer_should_keep_draining_after_an_error_event() {
        let (tx, rx) = mpsc::channel::<AgentEvent>(8);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();

        let producer = async move {
            tx.send(AgentEvent::Token { text: "before ".to_owned() })
                .await
                .unwrap();
            tx.send(AgentEvent::Error { message: "worker 1 failed".to_owned() })
                .await
                .unwrap();
            // The orchestrator is still running: later sends must still be accepted.
            let later = tx
                .send(AgentEvent::Token { text: "after-error".to_owned() })
                .await;
            assert!(later.is_ok(), "printer dropped the receiver at the first Error");
            tx.send(AgentEvent::WorkerCompleted {
                task_id: "t2".to_owned(),
                agent: "coder".to_owned(),
                status: "ok".to_owned(),
                cost_usd: 0.0,
            })
            .await
            .unwrap();
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(producer, print_orchestrate_events(rx, &mut out, &mut err));
        })
        .await
        .expect("printer hung or producer blocked");

        let out = String::from_utf8(out).unwrap();
        let err = String::from_utf8(err).unwrap();
        assert!(out.contains("before "), "{out}");
        assert!(out.contains("after-error"), "events after the Error were dropped: {out}");
        assert!(out.contains("t2"), "{out}");
        assert!(err.contains("worker 1 failed"), "{err}");
    }

    // R11: critique text and "APPROVED" never reach the user; a refine round's text follows
    // exactly one separator; tool/pause events of every round still arrive.
    const REVISED_SEP: &str = "\n\n---\n\n*Revised answer:*\n\n";

    async fn collect_medium(
        script: Vec<Vec<hone_proto::ProviderEvent>>,
    ) -> (Vec<AgentEvent>, std::path::PathBuf, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("out.txt");
        let tool = MarkerTool {
            name: "mock_write",
            read_only: false,
            target: target.clone(),
        };
        let mut base_config = tui_wiring_config(tool, tmp.path());
        base_config.provider = Arc::new(SeqProvider {
            script,
            n: std::sync::Mutex::new(0),
        });
        let agent = AgentHandle::spawn(base_config.clone());
        let mut base_config = base_config;
        let step_tx = wire_tui_approvals(&agent, &mut base_config, false).await;

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let _task = spawn_medium_turn(agent.clone(), "go".to_owned(), tx);
        let mut events = Vec::new();
        let collect = async {
            while let Some(e) = rx.recv().await {
                if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                    step_tx.send(hone_proto::StepAction::Proceed).await.unwrap();
                }
                events.push(e);
            }
        };
        tokio::time::timeout(Duration::from_secs(15), collect)
            .await
            .expect("medium turn wedged");
        (events, target, tmp)
    }

    fn shown_text(events: &[AgentEvent]) -> String {
        events
            .iter()
            .filter_map(|e| match e {
                AgentEvent::Token { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn test_r11_medium_turn_should_hide_critique_and_separate_refined_answer() {
        use hone_proto::ProviderEvent;
        let (events, target, _tmp) = collect_medium(vec![
            text_turn("first answer"),
            text_turn("the answer needs work"), // critique, not APPROVED
            vec![
                ProviderEvent::ToolCall(hone_proto::ToolCallRequest {
                    id: "c1".to_owned(),
                    name: "mock_write".to_owned(),
                    arguments: serde_json::json!({}),
                }),
                usage10(),
                ProviderEvent::Done,
            ],
            text_turn("refined"),
        ])
        .await;

        let shown = shown_text(&events);
        assert!(!shown.contains("needs work"), "critique leaked: {shown:?}");
        assert!(!shown.contains("APPROVED"), "{shown:?}");
        assert_eq!(
            shown,
            format!("first answer{REVISED_SEP}refined"),
            "answer, exactly one separator, then the refined text"
        );
        // Tool/pause events of the refine round still arrive and the write ran.
        assert_eq!(paused_count(&events), 1, "{events:?}");
        assert!(events.iter().any(|e| matches!(e, AgentEvent::ToolCallEnd { .. })));
        assert!(target.exists());
    }

    #[tokio::test]
    async fn test_r11_medium_turn_when_critique_approves_should_show_only_the_answer() {
        let (events, _target, _tmp) = collect_medium(vec![
            text_turn("the answer"),
            text_turn("APPROVED"),
        ])
        .await;

        let shown = shown_text(&events);
        assert_eq!(shown, "the answer", "no critique, no APPROVED, no separator");
    }

    // T5 (docs/plans/tool-error-turn-spec.md): the TUI forwarders must always end the turn.
    // A provider whose `complete()` panics kills the agent actor; the forwarder then sees a
    // dead actor / failed turn without any Done or Error, and must send one Error itself.
    struct PanickingProvider;

    #[async_trait::async_trait]
    impl hone_providers::ModelProvider for PanickingProvider {
        fn id(&self) -> &str {
            "panicking"
        }
        fn display_name(&self) -> &str {
            "Panicking"
        }
        fn supported_models(&self) -> Vec<hone_proto::ModelInfo> {
            vec![hone_proto::ModelInfo {
                id: ModelId::new("mock:test"),
                display_name: "Test".to_owned(),
                context_length: 4096,
                supports_tools: true,
                supports_streaming: true,
            }]
        }
        async fn complete(
            &self,
            _req: hone_providers::CompletionRequest,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = hone_proto::ProviderEvent> + Send>>,
            hone_providers::ProviderError,
        > {
            panic!("provider exploded (test)");
        }
        fn estimate_tokens(&self, _m: &[Message]) -> u32 {
            5
        }
        async fn health(&self) -> hone_proto::ProviderHealth {
            hone_proto::ProviderHealth::Healthy
        }
    }

    async fn forwarder_events_for_panicking_provider(medium: bool) -> Vec<AgentEvent> {
        let tmp = tempfile::tempdir().unwrap();
        let tool = MarkerTool {
            name: "mock_read",
            read_only: true,
            target: tmp.path().join("unused.txt"),
        };
        let mut config = tui_wiring_config(tool, tmp.path());
        config.provider = Arc::new(PanickingProvider);
        let agent = AgentHandle::spawn(config);

        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let _task = if medium {
            spawn_medium_turn(agent.clone(), "go".to_owned(), tx)
        } else {
            spawn_agent_turn(agent.clone(), "go".to_owned(), tx)
        };
        let mut events = Vec::new();
        // The forwarder finishes (drops tx) either way; a hang here is the bug.
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(e) = rx.recv().await {
                events.push(e);
            }
        })
        .await
        .expect("forwarder never finished");
        events
    }

    #[tokio::test]
    async fn test_t5_spawn_agent_turn_should_send_error_when_turn_dies_without_terminal_event() {
        let events = forwarder_events_for_panicking_provider(false).await;
        let errors = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Error { .. }))
            .count();
        assert_eq!(errors, 1, "the spinner needs exactly one terminal Error: {events:?}");
    }

    #[tokio::test]
    async fn test_t5_spawn_medium_turn_should_send_error_when_turn_dies_without_terminal_event() {
        let events = forwarder_events_for_panicking_provider(true).await;
        let errors = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Error { .. }))
            .count();
        assert_eq!(errors, 1, "the spinner needs exactly one terminal Error: {events:?}");
    }

    // U5: the medium forwarder words a dead actor like the simple one.
    #[tokio::test]
    async fn test_u5_dead_actor_error_message_should_start_with_agent_stopped_unexpectedly() {
        for medium in [false, true] {
            let events = forwarder_events_for_panicking_provider(medium).await;
            let msg = events
                .iter()
                .find_map(|e| match e {
                    AgentEvent::Error { message } => Some(message.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("(medium={medium}) no Error event: {events:?}"));
            assert!(
                msg.starts_with("agent stopped unexpectedly"),
                "(medium={medium}) got {msg:?}"
            );
        }
    }

    // U4. Seam (pure fn in main.rs; `run_json_mode` prints its result):
    //   fn json_mode_output(
    //       turn: &Result<hone_core::TurnResult, hone_core::AgentError>,
    //       response: String,
    //       tool_calls: Vec<serde_json::Value>,
    //       error: Option<String>,
    //       spend: (hone_proto::TokenUsage, f64),
    //       model_id: &str,
    //   ) -> serde_json::Value
    // `usage` ({input, output, cache_read}) and `cost_usd` come from `spend`
    // (`AgentHandle::session_spend()`), whatever the result;
    // `success == turn.is_ok() && error.is_none()`.
    fn spend_100_50() -> (hone_proto::TokenUsage, f64) {
        (
            hone_proto::TokenUsage {
                input: 100,
                output: 50,
                cache_read: 7,
                cache_creation: 0,
            },
            0.001,
        )
    }

    #[test]
    fn test_u4_json_output_for_failed_turn_should_report_spend_and_failure() {
        let turn: Result<hone_core::TurnResult, hone_core::AgentError> =
            Err(hone_core::AgentError::Cancelled);
        let v = json_mode_output(
            &turn,
            "partial".to_owned(),
            vec![],
            Some("provider exploded".to_owned()),
            spend_100_50(),
            "mock:test",
            &FallbackLog::default(),
        );
        assert_eq!(v["usage"]["input"], 100, "{v}");
        assert_eq!(v["usage"]["output"], 50, "{v}");
        assert_eq!(v["usage"]["cache_read"], 7, "{v}");
        assert!(v["cost_usd"].as_f64().unwrap() > 0.0, "{v}");
        assert_eq!(v["success"], false);
        assert_eq!(v["error"], "provider exploded");
        assert_eq!(v["model"], "mock:test");
        assert_eq!(v["response"], "partial");
    }

    #[test]
    fn test_u4_json_output_for_failed_turn_without_error_event_should_still_be_unsuccessful() {
        let turn: Result<hone_core::TurnResult, hone_core::AgentError> =
            Err(hone_core::AgentError::Cancelled);
        let v = json_mode_output(
            &turn,
            String::new(),
            vec![],
            None,
            spend_100_50(),
            "m",
            &FallbackLog::default(),
        );
        assert_eq!(v["success"], false, "{v}");
        assert_eq!(v["usage"]["input"], 100, "{v}");
    }

    #[test]
    fn test_u4_json_output_for_ok_turn_should_take_usage_and_cost_from_spend() {
        let turn: Result<hone_core::TurnResult, hone_core::AgentError> =
            Ok(hone_core::TurnResult {
                response: "done".to_owned(),
                usage: hone_proto::TokenUsage::default(),
                cost_usd: 0.0,
                tool_calls_count: 0,
            });
        let v = json_mode_output(
            &turn,
            "done".to_owned(),
            vec![],
            None,
            spend_100_50(),
            "m",
            &FallbackLog::default(),
        );
        assert_eq!(v["success"], true, "{v}");
        assert_eq!(v["usage"]["input"], 100, "{v}");
        assert_eq!(v["usage"]["output"], 50, "{v}");
        assert!((v["cost_usd"].as_f64().unwrap() - 0.001).abs() < 1e-12, "{v}");
        assert!(v["error"].is_null());
    }

    // R6. Seam (pure fn in main.rs):
    //   fn code_modifying_tools_that_ran(events: &[AgentEvent]) -> Vec<String>
    // Names (in order of completion) of write_file/edit_file/multi_edit calls whose
    // `ToolCallEnd` is a non-error result other than `{"skipped": true}`, matched to their
    // `ToolCallStart` by id. `maybe_run_test_verification` must be fed from this, not from
    // `ToolCallStart`.
    fn start(id: &str, name: &str) -> AgentEvent {
        AgentEvent::ToolCallStart {
            id: id.to_owned(),
            name: name.to_owned(),
            args: serde_json::json!({}),
        }
    }

    fn end(id: &str, result: serde_json::Value, is_error: bool) -> AgentEvent {
        AgentEvent::ToolCallEnd {
            id: id.to_owned(),
            result,
            is_error,
            duration_ms: 0,
            cache_hit: false,
        }
    }

    #[test]
    fn test_r6_successful_code_modifying_call_should_count() {
        let events = vec![
            start("a", "write_file"),
            end("a", serde_json::json!({"ok": true}), false),
        ];
        assert_eq!(code_modifying_tools_that_ran(&events), vec!["write_file"]);
    }

    #[test]
    fn test_r6_denied_error_result_should_not_count() {
        let events = vec![
            start("a", "write_file"),
            end(
                "a",
                serde_json::json!({"error": "denied by user", "reason": "no"}),
                true,
            ),
        ];
        assert!(code_modifying_tools_that_ran(&events).is_empty());
    }

    #[test]
    fn test_r6_skipped_result_should_not_count() {
        let events = vec![
            start("a", "edit_file"),
            end("a", serde_json::json!({"skipped": true}), false),
        ];
        assert!(code_modifying_tools_that_ran(&events).is_empty());
    }

    #[test]
    fn test_r6_calls_are_matched_by_id_and_only_code_modifying_tools_count() {
        let events = vec![
            start("r", "read_file"),
            start("w1", "write_file"),
            start("w2", "multi_edit"),
            end("r", serde_json::json!({"ok": true}), false),
            end("w2", serde_json::json!({"skipped": true}), false),
            end("w1", serde_json::json!({"ok": true}), false),
            end("ghost", serde_json::json!({"ok": true}), false), // no matching start
        ];
        assert_eq!(code_modifying_tools_that_ran(&events), vec!["write_file"]);
    }

    // ---- fallback notice (docs/plans/fallback-notice-spec.md: N7, N8, N10) ----
    //
    // Seams assumed:
    //   FallbackLog::default(), FallbackLog::record(requested, fallback, reason)
    //   json_mode_output(.., model_id, &FallbackLog) -> Value  (final parameter added; emits "fallbacks")
    //   FallbackPrinter::default(), .wrote(&str), .event(&mut err, requested, fallback, reason)
    //   note_turn_fallback(&AgentEvent, &mut Option<String>), end_turn_fallback(..)
    // The `--ensemble` printer is not unit-testable as written (it owns stdout/stderr); it is
    // covered by reading only.

    fn mf_event(requested: &str, fallback: &str, reason: &str) -> AgentEvent {
        AgentEvent::ModelFallback {
            requested: requested.to_owned(),
            fallback: fallback.to_owned(),
            reason: reason.to_owned(),
        }
    }

    fn done_event() -> AgentEvent {
        AgentEvent::Done {
            usage: hone_proto::TokenUsage::default(),
            cost_usd: 0.0,
            plan: None,
        }
    }

    const NOTICE_LINE: &str = "hone: a:x failed (why); switched to fallback b:y";

    #[test]
    fn test_n7_printer_starts_the_line_on_a_new_one_after_a_token_without_newline() {
        let mut p = FallbackPrinter::default();
        let mut err: Vec<u8> = Vec::new();
        p.wrote("abc");

        p.event(&mut err, "a:x", "b:y", "why");

        assert_eq!(
            String::from_utf8(err).unwrap(),
            format!("\n{NOTICE_LINE}\n")
        );
    }

    #[test]
    fn test_n7_printer_adds_no_blank_line_after_a_token_ending_in_a_newline() {
        let mut p = FallbackPrinter::default();
        let mut err: Vec<u8> = Vec::new();
        p.wrote("abc\n");

        p.event(&mut err, "a:x", "b:y", "why");

        assert_eq!(String::from_utf8(err).unwrap(), format!("{NOTICE_LINE}\n"));
    }

    #[test]
    fn test_n7_printer_adds_no_blank_line_when_nothing_was_printed() {
        let mut p = FallbackPrinter::default();
        let mut err: Vec<u8> = Vec::new();

        p.event(&mut err, "a:x", "b:y", "why");

        assert_eq!(String::from_utf8(err).unwrap(), format!("{NOTICE_LINE}\n"));
    }

    #[test]
    fn test_n7_printer_prints_a_pair_once() {
        let mut p = FallbackPrinter::default();
        let mut err: Vec<u8> = Vec::new();

        p.event(&mut err, "a:x", "b:y", "why");
        p.event(&mut err, "a:x", "b:y", "why again");
        p.event(&mut err, "a:x", "c:z", "other");

        assert_eq!(
            String::from_utf8(err).unwrap(),
            format!("{NOTICE_LINE}\nhone: a:x failed (other); switched to fallback c:z\n")
        );
    }

    async fn run_printer(events: Vec<AgentEvent>) -> (String, String) {
        let (tx, rx) = mpsc::channel::<AgentEvent>(16);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        for e in events {
            tx.send(e).await.unwrap();
        }
        drop(tx);
        tokio::time::timeout(
            Duration::from_secs(10),
            print_orchestrate_events(rx, &mut out, &mut err),
        )
        .await
        .expect("printer hung");
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[tokio::test]
    async fn test_n7_orchestrate_printer_prints_a_pair_once_on_stderr_and_nothing_on_stdout() {
        let (out, err) = run_printer(vec![
            mf_event("a:x", "b:y", "why"),
            AgentEvent::Token {
                text: "hi\n".to_owned(),
            },
            mf_event("a:x", "b:y", "why"),
        ])
        .await;

        assert_eq!(err, format!("{NOTICE_LINE}\n"));
        assert_eq!(out, "hi\n", "stdout carries no notice");
    }

    #[tokio::test]
    async fn test_n7_orchestrate_printer_adds_no_blank_line_after_a_worker_line() {
        // The token leaves stdout mid-line; the worker line (writeln!) ends it.
        let (out, err) = run_printer(vec![
            AgentEvent::Token {
                text: "abc".to_owned(),
            },
            AgentEvent::WorkerCompleted {
                task_id: "t1".to_owned(),
                agent: "coder".to_owned(),
                status: "ok".to_owned(),
                cost_usd: 0.0,
            },
            mf_event("a:x", "b:y", "why"),
        ])
        .await;

        assert!(out.ends_with('\n'), "{out:?}");
        assert_eq!(err, format!("{NOTICE_LINE}\n"), "no leading blank line");
    }

    #[tokio::test]
    async fn test_n7_orchestrate_printer_starts_a_new_line_after_a_mid_line_token() {
        let (_, err) = run_printer(vec![
            AgentEvent::Token {
                text: "abc".to_owned(),
            },
            mf_event("a:x", "b:y", "why"),
        ])
        .await;

        assert_eq!(err, format!("\n{NOTICE_LINE}\n"));
    }

    fn failed_turn() -> Result<hone_core::TurnResult, hone_core::AgentError> {
        Err(hone_core::AgentError::Cancelled)
    }

    #[test]
    fn test_n8_json_output_has_an_empty_fallbacks_array_for_an_empty_log() {
        let v = json_mode_output(
            &failed_turn(),
            String::new(),
            vec![],
            None,
            spend_100_50(),
            "m",
            &FallbackLog::default(),
        );

        assert_eq!(v["fallbacks"], serde_json::json!([]), "{v}");
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "cost_usd",
                "error",
                "fallbacks",
                "model",
                "response",
                "success",
                "tool_calls",
                "usage"
            ]
        );
    }

    #[test]
    fn test_n8_json_output_lists_pairs_in_first_seen_order_with_counts_and_first_reason() {
        let mut log = FallbackLog::default();
        let _ = log.record("a:x", "b:y", "first reason");
        let _ = log.record("a:x", "c:z", "other");
        let _ = log.record("a:x", "b:y", "second reason");
        let _ = log.record("a:x", "b:y", "third reason");

        let v = json_mode_output(
            &failed_turn(),
            String::new(),
            vec![],
            None,
            spend_100_50(),
            "a:x",
            &log,
        );

        assert_eq!(
            v["fallbacks"],
            serde_json::json!([
                {"requested": "a:x", "fallback": "b:y", "reason": "first reason", "calls": 3},
                {"requested": "a:x", "fallback": "c:z", "reason": "other", "calls": 1},
            ])
        );
        assert_eq!(v["model"], "a:x", "model keeps its meaning");
    }

    #[test]
    fn test_n10_a_fallback_then_done_records_the_fallback_model_then_clears_it() {
        let mut slot: Option<String> = None;

        note_turn_fallback(&mf_event("a:x", "b:y", "why"), &mut slot);
        assert_eq!(slot.as_deref(), Some("b:y"));

        end_turn_fallback(&done_event(), &mut slot);
        assert_eq!(slot, None);
    }

    #[test]
    fn test_n10_the_last_fallback_of_a_turn_wins() {
        let mut slot: Option<String> = None;
        note_turn_fallback(&mf_event("a:x", "b:y", "why"), &mut slot);
        note_turn_fallback(&mf_event("a:x", "c:z", "why"), &mut slot);
        assert_eq!(slot.as_deref(), Some("c:z"));
    }

    #[test]
    fn test_n10_other_events_neither_set_nor_clear_the_slot() {
        let mut slot: Option<String> = None;
        let token = AgentEvent::Token {
            text: "t".to_owned(),
        };
        note_turn_fallback(&token, &mut slot);
        end_turn_fallback(&token, &mut slot);
        assert_eq!(slot, None);

        note_turn_fallback(&mf_event("a:x", "b:y", "why"), &mut slot);
        note_turn_fallback(&token, &mut slot);
        end_turn_fallback(&token, &mut slot);
        assert_eq!(slot.as_deref(), Some("b:y"));
    }

    #[test]
    fn test_n10_an_error_clears_the_slot() {
        let mut slot = Some("b:y".to_owned());
        end_turn_fallback(
            &AgentEvent::Error {
                message: "boom".to_owned(),
            },
            &mut slot,
        );
        assert_eq!(slot, None);
    }

    // N7 ensemble printer. Seam assumed (free fn in main.rs, extracted from `run_ensemble`):
    //   async fn print_ensemble_events<O: io::Write, E: io::Write>(
    //       rx: mpsc::Receiver<AgentEvent>, out: &mut O, err: &mut E)
    // Tokens and an `Error` are printed only until the first `Done` or `Error`; afterwards the
    // channel is read to its end, ignoring tokens and errors, so later notices still print.
    async fn run_ensemble_printer(events: Vec<AgentEvent>) -> (String, String) {
        let (tx, rx) = mpsc::channel::<AgentEvent>(16);
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        for e in events {
            tx.send(e).await.unwrap();
        }
        drop(tx);
        tokio::time::timeout(
            Duration::from_secs(10),
            print_ensemble_events(rx, &mut out, &mut err),
        )
        .await
        .expect("printer hung");
        (
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    fn token_event(text: &str) -> AgentEvent {
        AgentEvent::Token {
            text: text.to_owned(),
        }
    }

    fn error_event(message: &str) -> AgentEvent {
        AgentEvent::Error {
            message: message.to_owned(),
        }
    }

    #[tokio::test]
    async fn test_n7_ensemble_printer_prints_tokens_before_the_first_done_and_ignores_later_ones() {
        let (out, err) = run_ensemble_printer(vec![
            token_event("one "),
            token_event("two"),
            done_event(),
            token_event(" late"),
        ])
        .await;

        assert_eq!(out, "one two");
        assert_eq!(err, "");
    }

    #[tokio::test]
    async fn test_n7_ensemble_printer_prints_a_notice_after_the_first_done() {
        let (out, err) = run_ensemble_printer(vec![
            token_event("answer"),
            done_event(),
            mf_event("a:x", "b:y", "why"),
            mf_event("a:x", "b:y", "why"),
        ])
        .await;

        assert_eq!(out, "answer");
        assert_eq!(err, format!("\n{NOTICE_LINE}\n"), "once, on its own line");
    }

    #[tokio::test]
    async fn test_n7_ensemble_printer_does_not_print_an_error_after_done_but_prints_the_notice() {
        let (out, err) = run_ensemble_printer(vec![
            token_event("answer\n"),
            done_event(),
            error_event("candidate 2 failed"),
            mf_event("a:x", "b:y", "why"),
        ])
        .await;

        assert_eq!(out, "answer\n");
        assert!(!err.contains("candidate 2 failed"), "{err}");
        assert_eq!(err, format!("{NOTICE_LINE}\n"));
    }

    #[tokio::test]
    async fn test_n7_ensemble_printer_prints_the_first_error_then_still_prints_later_notices() {
        let (out, err) = run_ensemble_printer(vec![
            error_event("first failure"),
            token_event("ignored"),
            error_event("second failure"),
            mf_event("a:x", "b:y", "why"),
        ])
        .await;

        assert_eq!(out, "");
        assert!(err.starts_with("\nerror: first failure\n"), "{err:?}");
        assert!(!err.contains("second failure"), "{err}");
        assert!(!err.contains("ignored"), "{err}");
        assert!(err.ends_with(&format!("{NOTICE_LINE}\n")), "{err:?}");
    }

    // ---- worker tools W10-W12 (docs/plans/worker-tools-spec.md) ----

    type WtLog = Arc<std::sync::Mutex<Vec<String>>>;

    /// Records every execution by tool name.
    struct WtTool {
        name: &'static str,
        read_only: bool,
        log: WtLog,
        command_subject: Option<&'static str>,
    }

    #[async_trait::async_trait]
    impl hone_tools::Tool for WtTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "fake"
        }
        fn schema(&self) -> serde_json::Value {
            serde_json::json!({ "type": "object" })
        }
        async fn call(
            &self,
            _input: hone_tools::ToolInput,
            _ctx: &hone_tools::ToolContext,
        ) -> Result<hone_tools::ToolOutput, hone_tools::ToolError> {
            self.log.lock().unwrap().push(self.name.to_owned());
            Ok(hone_tools::ToolOutput::new(serde_json::json!({ "ok": true })))
        }
        fn layer(&self) -> hone_proto::ToolLayer {
            hone_proto::ToolLayer::Native
        }
        fn is_read_only(&self) -> bool {
            self.read_only
        }
        fn subjects(
            &self,
            _input: &hone_tools::ToolInput,
            _sctx: &hone_tools::SubjectContext,
        ) -> hone_tools::ToolSubjects {
            match self.command_subject {
                Some(c) => hone_tools::ToolSubjects {
                    subjects: vec![hone_config::policy::Subject::Command(c.to_owned())],
                    unparsed: false,
                    ..Default::default()
                },
                None => hone_tools::ToolSubjects::default(),
            }
        }
    }

    /// The tools of the `debugger` agent (`read, edit, write, shell, grep, glob`)
    /// under their real names, plus one extra.
    fn wt_tools(log: &WtLog, shell_subject: Option<&'static str>) -> ToolRegistry {
        let mut b = ToolRegistry::builder();
        for (name, ro) in [
            ("read_file", true),
            ("edit_file", false),
            ("write_file", false),
            ("shell", false),
            ("grep", true),
            ("glob", true),
            ("git_commit", false),
        ] {
            b = b.register(Box::new(WtTool {
                name,
                read_only: ro,
                log: Arc::clone(log),
                command_subject: if name == "shell" { shell_subject } else { None },
            }));
        }
        b.build()
    }

    /// A worker request that does not follow a tool result issues one `shell`
    /// call (ids `wt1`, `wt2`, ... in order); the request after it answers
    /// "ok". Lead merge: "merged". Records the tool names of each worker
    /// request, counts every request (lead included) and may delay answers
    /// (virtual time).
    struct WtProvider {
        worker_requests: std::sync::Mutex<Vec<Vec<String>>>,
        total_requests: std::sync::atomic::AtomicUsize,
        calls_issued: std::sync::atomic::AtomicUsize,
        delay: Option<Duration>,
    }

    #[async_trait::async_trait]
    impl hone_providers::ModelProvider for WtProvider {
        fn id(&self) -> &str {
            "wt-provider"
        }
        fn display_name(&self) -> &str {
            "WT provider"
        }
        fn supported_models(&self) -> Vec<hone_proto::ModelInfo> {
            vec![]
        }
        async fn complete(
            &self,
            req: hone_providers::CompletionRequest,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = hone_proto::ProviderEvent> + Send>>,
            hone_providers::ProviderError,
        > {
            let text = req
                .messages
                .iter()
                .map(|m| m.content.as_text())
                .collect::<Vec<_>>()
                .join("\n");
            self.total_requests
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let after_tool = req
                .messages
                .last()
                .is_some_and(|m| m.role == hone_proto::Role::Tool);
            let events = if text.contains("You are the lead agent. Based on") {
                vec![
                    hone_proto::ProviderEvent::Token {
                        text: "merged".to_owned(),
                    },
                    hone_proto::ProviderEvent::Done,
                ]
            } else {
                if let Some(d) = self.delay {
                    tokio::time::sleep(d).await;
                }
                {
                    let mut w = self.worker_requests.lock().unwrap();
                    let mut names: Vec<String> = req.tools.iter().map(|t| t.name.clone()).collect();
                    names.sort();
                    w.push(names);
                }
                if !after_tool {
                    let k = self
                        .calls_issued
                        .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                        + 1;
                    vec![
                        hone_proto::ProviderEvent::ToolCall(hone_proto::ToolCallRequest {
                            id: format!("wt{k}"),
                            name: "shell".to_owned(),
                            arguments: serde_json::json!({ "cmd": "cargo test" }),
                        }),
                        hone_proto::ProviderEvent::Done,
                    ]
                } else {
                    vec![
                        hone_proto::ProviderEvent::Token {
                            text: "ok".to_owned(),
                        },
                        hone_proto::ProviderEvent::Done,
                    ]
                }
            };
            Ok(Box::pin(futures::stream::iter(events)))
        }
        fn estimate_tokens(&self, _m: &[Message]) -> u32 {
            1
        }
        async fn health(&self) -> hone_proto::ProviderHealth {
            hone_proto::ProviderHealth::Healthy
        }
    }

    /// A TUI-like session: tools = `wt_tools`, `AskRisky`, a step channel.
    /// Returns (config, provider, tool log, step sender).
    fn wt_session(
        dir: &std::path::Path,
        delay: Option<Duration>,
    ) -> (
        AgentConfig,
        Arc<WtProvider>,
        WtLog,
        mpsc::Sender<hone_proto::StepAction>,
    ) {
        let log: WtLog = Arc::new(std::sync::Mutex::new(Vec::new()));
        let provider = Arc::new(WtProvider {
            worker_requests: std::sync::Mutex::new(Vec::new()),
            total_requests: std::sync::atomic::AtomicUsize::new(0),
            calls_issued: std::sync::atomic::AtomicUsize::new(0),
            delay,
        });
        let mut config = orch_config(Arc::clone(&provider) as _, dir);
        config.tools = wt_tools(&log, None);
        config.approval_policy = ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        };
        let (step_tx, step_rx) = mpsc::channel::<hone_proto::StepAction>(8);
        config.step_rx = Some(Arc::new(tokio::sync::Mutex::new(step_rx)));
        (config, provider, log, step_tx)
    }

    /// Drain the run's events, answering each `ToolCallPaused` with `answer`.
    /// Gives up (panics) after 10 s of the tokio clock, e.g. on an unanswerable
    /// pause that nobody expected.
    async fn wt_drive(
        rx: &mut mpsc::Receiver<AgentEvent>,
        step_tx: &mpsc::Sender<hone_proto::StepAction>,
        answer: hone_proto::StepAction,
        limit: Duration,
    ) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let drive = async {
            while let Some(e) = rx.recv().await {
                if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                    step_tx.send(answer.clone()).await.unwrap();
                }
                events.push(e);
            }
        };
        if tokio::time::timeout(limit, drive).await.is_err() {
            panic!("orchestrated run did not finish; events so far: {events:?}");
        }
        events
    }

    fn wt_pauses(events: &[AgentEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, AgentEvent::ToolCallPaused { .. }))
            .count()
    }

    const WT_STEP_LIMIT: Duration = Duration::from_secs(10);

    // W10 ---------------------------------------------------------------

    #[tokio::test]
    async fn w10_orchestrated_turn_when_agent_policy_changed_to_auto_should_run_workers_without_pause()
     {
        let dir = tempfile::tempdir().unwrap();
        let (config, _provider, log, step_tx) = wt_session(dir.path(), None);
        // The config snapshot says AskRisky; the session has since moved on.
        let agent = AgentHandle::spawn(config.clone());
        agent.set_approval_policy(ToolApprovalPolicy::Auto).await;
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;

        assert_eq!(wt_pauses(&events), 0, "{events:?}");
        assert_eq!(*log.lock().unwrap(), vec!["shell".to_owned()]);
    }

    #[tokio::test]
    async fn w10_orchestrated_turn_when_tool_was_approved_always_should_not_pause_for_it() {
        let dir = tempfile::tempdir().unwrap();
        let (config, _provider, log, step_tx) = wt_session(dir.path(), None);
        let agent = AgentHandle::spawn(config.clone());
        agent.approve_tools(vec!["shell".to_owned()]).await;
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;

        assert_eq!(wt_pauses(&events), 0, "{events:?}");
        assert_eq!(*log.lock().unwrap(), vec!["shell".to_owned()]);
    }

    #[tokio::test]
    async fn w10_orchestrated_turn_when_agent_mode_is_plan_should_give_workers_read_only_tools() {
        let dir = tempfile::tempdir().unwrap();
        let (config, provider, log, step_tx) = wt_session(dir.path(), None);
        assert_eq!(config.mode, AgentMode::Build, "the snapshot is Build");
        let agent = AgentHandle::spawn(config.clone());
        agent.set_mode(AgentMode::Plan).await;
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;

        let first_worker_request = provider.worker_requests.lock().unwrap()[0].clone();
        assert_eq!(
            first_worker_request,
            vec!["glob".to_owned(), "grep".to_owned(), "read_file".to_owned()],
            "debugger's tools narrowed to the read-only ones"
        );
        assert!(log.lock().unwrap().is_empty(), "shell must not run in plan mode");
        assert_eq!(wt_pauses(&events), 0);
    }

    #[tokio::test]
    async fn w10_orchestrated_turn_when_agent_mode_is_auto_approve_should_run_workers_without_pause()
     {
        let dir = tempfile::tempdir().unwrap();
        let (config, _provider, log, step_tx) = wt_session(dir.path(), None);
        let agent = AgentHandle::spawn(config.clone());
        agent.set_mode(AgentMode::AutoApprove).await;
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;

        assert_eq!(wt_pauses(&events), 0, "{events:?}");
        assert_eq!(*log.lock().unwrap(), vec!["shell".to_owned()]);
    }

    // W12 ---------------------------------------------------------------

    #[tokio::test]
    async fn w12_worker_tool_calls_should_reach_the_tui_and_a_step_action_should_answer_the_pause() {
        let dir = tempfile::tempdir().unwrap();
        let (config, _provider, log, step_tx) = wt_session(dir.path(), None);
        let agent = AgentHandle::spawn(config.clone());
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;

        let pos = |pred: &dyn Fn(&AgentEvent) -> bool| events.iter().position(pred);
        let start = pos(&|e| matches!(e, AgentEvent::ToolCallStart { id, .. } if id == "wt1"));
        let paused = pos(&|e| matches!(e, AgentEvent::ToolCallPaused { id, .. } if id == "wt1"));
        let end = pos(&|e| matches!(e, AgentEvent::ToolCallEnd { id, is_error: false, .. } if id == "wt1"));
        assert!(start.is_some() && paused.is_some() && end.is_some(), "{events:?}");
        assert!(start < paused && paused < end, "{events:?}");
        // Proceed ran the tool.
        assert_eq!(*log.lock().unwrap(), vec!["shell".to_owned()]);
        // G14: still exactly one Done, last.
        let dones = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Done { .. }))
            .count();
        assert_eq!(dones, 1, "{events:?}");
        assert!(matches!(events.last(), Some(AgentEvent::Done { .. })), "{events:?}");
    }

    #[tokio::test]
    async fn w12_worker_when_denied_in_the_tui_should_not_run_and_run_should_still_end_with_one_done()
     {
        let dir = tempfile::tempdir().unwrap();
        let (config, _provider, log, step_tx) = wt_session(dir.path(), None);
        let agent = AgentHandle::spawn(config.clone());
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = wt_drive(
            &mut rx,
            &step_tx,
            hone_proto::StepAction::Deny("no".to_owned()),
            WT_STEP_LIMIT,
        )
        .await;

        assert_eq!(wt_pauses(&events), 1, "{events:?}");
        assert!(log.lock().unwrap().is_empty());
        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentEvent::ToolCallEnd { id, is_error: true, .. } if id == "wt1"
            )),
            "{events:?}"
        );
        let dones = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Done { .. }))
            .count();
        assert_eq!(dones, 1, "{events:?}");
    }

    #[tokio::test]
    async fn w12_worker_when_policy_asks_should_forward_approval_required_before_the_pause() {
        let dir = tempfile::tempdir().unwrap();
        let (mut config, _provider, log, step_tx) = wt_session(dir.path(), None);
        config.tools = wt_tools(&log, Some("cargo test"));
        // The session approves risky tools silently; a policy `ask` rule asks.
        config.approval_policy = ToolApprovalPolicy::Auto;
        let context =
            hone_config::policy::PolicyContext::for_platform(dir.path().to_path_buf(), None);
        let rules = vec![
            hone_config::policy::PolicyRule::command("cargo *", hone_config::Decision::Ask)
                .with_id("ask-cargo"),
        ];
        let compiled =
            hone_config::policy::CompiledPolicy::compile(&context, &[], &rules, &[]).unwrap();
        config.policy = Some(hone_core::PolicyConfig {
            compiled: Arc::new(compiled),
            context,
            pre_approved: vec![],
            builtins: false,
        });
        let agent = AgentHandle::spawn(config.clone());
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;

        let approval = events
            .iter()
            .position(|e| matches!(e, AgentEvent::ApprovalRequired { .. }));
        let paused = events
            .iter()
            .position(|e| matches!(e, AgentEvent::ToolCallPaused { .. }));
        assert!(approval.is_some() && paused.is_some(), "{events:?}");
        assert!(approval < paused, "{events:?}");
        assert_eq!(*log.lock().unwrap(), vec!["shell".to_owned()]);
    }

    // W11 ---------------------------------------------------------------

    #[tokio::test(start_paused = true)]
    async fn w11_orchestrated_turn_should_not_time_out_a_slow_worker() {
        let dir = tempfile::tempdir().unwrap();
        // 400 s per provider call: past the 300 s default worker timeout.
        let (mut config, _provider, log, step_tx) =
            wt_session(dir.path(), Some(Duration::from_secs(400)));
        config.approval_policy = ToolApprovalPolicy::Auto;
        let agent = AgentHandle::spawn(config.clone());
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = wt_drive(
            &mut rx,
            &step_tx,
            hone_proto::StepAction::Proceed,
            Duration::from_secs(86_400),
        )
        .await;

        assert!(
            !events.iter().any(|e| matches!(
                e,
                AgentEvent::WorkerCompleted { status, .. } if status == "timed_out"
            )),
            "no per-worker timeout in the TUI: {events:?}"
        );
        assert!(
            events.iter().any(|e| matches!(
                e,
                AgentEvent::WorkerCompleted { status, .. } if status == "completed"
            )),
            "{events:?}"
        );
        assert_eq!(*log.lock().unwrap(), vec!["shell".to_owned()]);
    }

    // ---- shared "always" W17 (docs/plans/worker-tools-spec.md) ----

    /// Every model turn that is not a lead merge and does not follow a tool
    /// result issues one `shell` call (fresh id); the turn after answers "ok".
    struct W17Provider {
        n: std::sync::Mutex<usize>,
    }

    #[async_trait::async_trait]
    impl hone_providers::ModelProvider for W17Provider {
        fn id(&self) -> &str {
            "w17"
        }
        fn display_name(&self) -> &str {
            "W17"
        }
        fn supported_models(&self) -> Vec<hone_proto::ModelInfo> {
            vec![]
        }
        async fn complete(
            &self,
            req: hone_providers::CompletionRequest,
        ) -> Result<
            std::pin::Pin<Box<dyn futures::Stream<Item = hone_proto::ProviderEvent> + Send>>,
            hone_providers::ProviderError,
        > {
            let text = req
                .messages
                .iter()
                .map(|m| m.content.as_text())
                .collect::<Vec<_>>()
                .join("\n");
            let after_tool = req
                .messages
                .last()
                .is_some_and(|m| m.role == hone_proto::Role::Tool);
            let events = if text.contains("You are the lead agent. Based on") || after_tool {
                vec![
                    hone_proto::ProviderEvent::Token {
                        text: "ok".to_owned(),
                    },
                    hone_proto::ProviderEvent::Done,
                ]
            } else {
                let id = {
                    let mut n = self.n.lock().unwrap();
                    *n += 1;
                    format!("w17-{n}")
                };
                vec![
                    hone_proto::ProviderEvent::ToolCall(hone_proto::ToolCallRequest {
                        id,
                        name: "shell".to_owned(),
                        arguments: serde_json::json!({}),
                    }),
                    hone_proto::ProviderEvent::Done,
                ]
            };
            Ok(Box::pin(futures::stream::iter(events)))
        }
        fn estimate_tokens(&self, _m: &[Message]) -> u32 {
            1
        }
        async fn health(&self) -> hone_proto::ProviderHealth {
            hone_proto::ProviderHealth::Healthy
        }
    }

    /// `wt_session` with the W17 provider; `ask_rule` switches the session to
    /// `Auto` approval plus a policy `ask` rule on the shell command.
    fn w17_session(
        dir: &std::path::Path,
        ask_rule: bool,
    ) -> (AgentConfig, WtLog, mpsc::Sender<hone_proto::StepAction>) {
        let (mut config, _p, log, step_tx) = wt_session(dir, None);
        config.provider = Arc::new(W17Provider {
            n: std::sync::Mutex::new(0),
        });
        if ask_rule {
            config.tools = wt_tools(&log, Some("cargo test"));
            config.approval_policy = ToolApprovalPolicy::Auto;
            let context =
                hone_config::policy::PolicyContext::for_platform(dir.to_path_buf(), None);
            let rules = vec![
                hone_config::policy::PolicyRule::command("cargo *", hone_config::Decision::Ask)
                    .with_id("ask-cargo"),
            ];
            let compiled =
                hone_config::policy::CompiledPolicy::compile(&context, &[], &rules, &[]).unwrap();
            config.policy = Some(hone_core::PolicyConfig {
                compiled: Arc::new(compiled),
                context,
                pre_approved: vec![],
                builtins: false,
            });
        }
        (config, log, step_tx)
    }

    async fn w17_run(
        config: &AgentConfig,
        agent: &AgentHandle,
        step_tx: &mpsc::Sender<hone_proto::StepAction>,
        answer: hone_proto::StepAction,
    ) -> Vec<AgentEvent> {
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let _h = spawn_orchestrated_turn(
            config.clone(),
            "ship it".to_owned(),
            tx,
            agent.clone(),
            Some(one_task_plan()),
        );
        wt_drive(&mut rx, step_tx, answer, WT_STEP_LIMIT).await
    }

    /// A plain (simple-tier) turn of the session agent, as the TUI runs it.
    async fn w17_turn(
        agent: &AgentHandle,
        step_tx: &mpsc::Sender<hone_proto::StepAction>,
        answer: hone_proto::StepAction,
    ) -> Vec<AgentEvent> {
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let _task = spawn_agent_turn(agent.clone(), "go".to_owned(), tx);
        let mut events = Vec::new();
        let collect = async {
            while let Some(e) = rx.recv().await {
                if matches!(e, AgentEvent::ToolCallPaused { .. }) {
                    step_tx.send(answer.clone()).await.unwrap();
                }
                let terminal = matches!(e, AgentEvent::Done { .. } | AgentEvent::Error { .. });
                events.push(e);
                if terminal {
                    break;
                }
            }
        };
        tokio::time::timeout(WT_STEP_LIMIT, collect)
            .await
            .expect("turn hung");
        events
    }

    fn w17_always() -> hone_proto::StepAction {
        hone_proto::StepAction::ApproveAlways("shell".to_owned())
    }

    #[tokio::test]
    async fn w17_workers_always_for_a_tool_should_cover_the_session_agents_next_turn() {
        let dir = tempfile::tempdir().unwrap();
        let (config, log, step_tx) = w17_session(dir.path(), false);
        let agent = AgentHandle::spawn(config.clone());

        let run = w17_run(&config, &agent, &step_tx, w17_always()).await;
        assert_eq!(wt_pauses(&run), 1, "{run:?}");
        let turn = w17_turn(&agent, &step_tx, hone_proto::StepAction::Proceed).await;

        assert_eq!(wt_pauses(&turn), 0, "the worker's always covers the agent: {turn:?}");
        assert_eq!(*log.lock().unwrap(), vec!["shell".to_owned(), "shell".to_owned()]);
    }

    #[tokio::test]
    async fn w17_workers_always_should_cover_the_next_orchestrated_run() {
        let dir = tempfile::tempdir().unwrap();
        let (config, log, step_tx) = w17_session(dir.path(), false);
        let agent = AgentHandle::spawn(config.clone());

        let first = w17_run(&config, &agent, &step_tx, w17_always()).await;
        assert_eq!(wt_pauses(&first), 1);
        let second = w17_run(&config, &agent, &step_tx, hone_proto::StepAction::Proceed).await;

        assert_eq!(wt_pauses(&second), 0, "{second:?}");
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn w17_agents_always_for_a_tool_should_cover_the_runs_workers() {
        let dir = tempfile::tempdir().unwrap();
        let (config, log, step_tx) = w17_session(dir.path(), false);
        let agent = AgentHandle::spawn(config.clone());

        let turn = w17_turn(&agent, &step_tx, w17_always()).await;
        assert_eq!(wt_pauses(&turn), 1, "{turn:?}");
        let run = w17_run(&config, &agent, &step_tx, hone_proto::StepAction::Proceed).await;

        assert_eq!(wt_pauses(&run), 0, "{run:?}");
        assert_eq!(*log.lock().unwrap(), vec!["shell".to_owned(), "shell".to_owned()]);
    }

    #[tokio::test]
    async fn w17_workers_always_for_a_policy_ask_rule_should_cover_the_session_agents_next_turn() {
        let dir = std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
        let (config, log, step_tx) = w17_session(&dir, true);
        let agent = AgentHandle::spawn(config.clone());

        let run = w17_run(&config, &agent, &step_tx, w17_always()).await;
        assert_eq!(wt_pauses(&run), 1, "{run:?}");
        let turn = w17_turn(&agent, &step_tx, hone_proto::StepAction::Proceed).await;

        assert_eq!(wt_pauses(&turn), 0, "{turn:?}");
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn w17_agents_always_for_a_policy_ask_rule_should_cover_the_runs_workers() {
        let dir = std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
        let (config, log, step_tx) = w17_session(&dir, true);
        let agent = AgentHandle::spawn(config.clone());

        let turn = w17_turn(&agent, &step_tx, w17_always()).await;
        assert_eq!(wt_pauses(&turn), 1, "{turn:?}");
        let run = w17_run(&config, &agent, &step_tx, hone_proto::StepAction::Proceed).await;

        assert_eq!(wt_pauses(&run), 0, "{run:?}");
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    // ---- W19: a stopped agent fails closed (docs/plans/worker-tools-spec.md) ----

    #[tokio::test]
    async fn w19_orchestrated_turn_when_agent_has_stopped_should_send_one_error_and_call_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let (config, provider, log, step_tx) = wt_session(dir.path(), None);
        let agent = AgentHandle::spawn(config.clone());
        agent.clone().shutdown().await;
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            config,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let events = wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;

        let errors: Vec<&AgentEvent> = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Error { .. }))
            .collect();
        assert_eq!(errors.len(), 1, "exactly one Error: {events:?}");
        match errors[0] {
            AgentEvent::Error { message, .. } => {
                assert!(message.contains("the agent has stopped"), "{message}")
            }
            _ => unreachable!(),
        }
        assert!(
            !events.iter().any(|e| matches!(e, AgentEvent::Done { .. })),
            "no Done: {events:?}"
        );
        assert_eq!(
            provider
                .total_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no lead or worker call may start"
        );
        assert!(log.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn w19_orchestrated_turn_when_agent_has_stopped_and_no_plan_should_not_decompose_either() {
        let dir = tempfile::tempdir().unwrap();
        let (config, provider, _log, step_tx) = wt_session(dir.path(), None);
        let agent = AgentHandle::spawn(config.clone());
        agent.clone().shutdown().await;
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(config, "ship it".to_owned(), tx, agent, None);
        let events = wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;

        let errors = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Error { .. }))
            .count();
        assert_eq!(errors, 1, "{events:?}");
        assert_eq!(
            provider
                .total_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
    }

    // ---- W18: an @agent persona's tool list reaches orchestrated runs ----
    //
    // Pins a small function the TUI loop must use for the Complex branch's
    // config clone (`base_config_clone`) when an inline `@agent` with a `tools:`
    // list is active:
    //
    //     fn narrow_run_config_for_persona(config: AgentConfig, allowed: &[String]) -> AgentConfig
    //
    // It returns `config` with `tools` = `filtered_registry(&config.tools, allowed)`
    // (unchanged when `allowed` is empty). The tests then run the orchestrator on
    // the result and look at the tool definitions the workers really send.

    async fn w18_worker_tools(allowed: &[&str]) -> Vec<Vec<String>> {
        let dir = tempfile::tempdir().unwrap();
        let (config, provider, _log, step_tx) = wt_session(dir.path(), None);
        let allowed: Vec<String> = allowed.iter().map(|s| (*s).to_owned()).collect();
        let narrowed = narrow_run_config_for_persona(config, &allowed);
        let agent = AgentHandle::spawn(narrowed.clone());
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            narrowed,
            "ship it".to_owned(),
            tx,
            agent,
            Some(one_task_plan()),
        );
        let _ = wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;
        let seen = provider.worker_requests.lock().unwrap().clone();
        assert!(!seen.is_empty(), "the worker must have called the provider");
        seen
    }

    #[tokio::test]
    async fn w18_persona_with_read_only_should_leave_workers_with_at_most_read_file() {
        // The worker's own agent (debugger) lists read, edit, write, shell, grep, glob.
        for request in w18_worker_tools(&["read"]).await {
            assert_eq!(request, vec!["read_file".to_owned()], "{request:?}");
        }
    }

    #[tokio::test]
    async fn w18_persona_with_several_tools_should_intersect_with_the_workers_own_list() {
        for request in w18_worker_tools(&["read", "shell", "git_commit"]).await {
            // git_commit is in the persona's list but not the debugger's.
            assert_eq!(
                request,
                vec!["read_file".to_owned(), "shell".to_owned()],
                "{request:?}"
            );
        }
    }

    #[tokio::test]
    async fn w18_persona_without_a_tools_list_should_not_narrow_workers() {
        for request in w18_worker_tools(&[]).await {
            assert_eq!(
                request,
                vec![
                    "edit_file".to_owned(),
                    "glob".to_owned(),
                    "grep".to_owned(),
                    "read_file".to_owned(),
                    "shell".to_owned(),
                    "write_file".to_owned()
                ]
            );
        }
    }

    // ---- W26: a made-up agent name gets read-only tools ----

    fn unknown_agent_plan() -> hone_core::TaskPlan {
        let mut plan = one_task_plan();
        plan.subtasks[0].agent = "made-up-agent".to_owned();
        plan
    }

    async fn w26_worker_tools(allowed: &[&str]) -> Vec<Vec<String>> {
        let dir = tempfile::tempdir().unwrap();
        let (config, provider, log, step_tx) = wt_session(dir.path(), None);
        let allowed: Vec<String> = allowed.iter().map(|s| (*s).to_owned()).collect();
        let narrowed = narrow_run_config_for_persona(config, &allowed);
        let agent = AgentHandle::spawn(narrowed.clone());
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

        let _h = spawn_orchestrated_turn(
            narrowed,
            "ship it".to_owned(),
            tx,
            agent,
            Some(unknown_agent_plan()),
        );
        let events =
            wt_drive(&mut rx, &step_tx, hone_proto::StepAction::Proceed, WT_STEP_LIMIT).await;
        assert_eq!(wt_pauses(&events), 0, "{events:?}");
        assert!(log.lock().unwrap().is_empty(), "the risky call must not run");
        let seen = provider.worker_requests.lock().unwrap().clone();
        assert!(!seen.is_empty());
        seen
    }

    #[tokio::test]
    async fn w26_unknown_agent_worker_should_get_read_only_tools_and_its_risky_call_is_not_found() {
        for request in w26_worker_tools(&[]).await {
            assert_eq!(
                request,
                vec!["glob".to_owned(), "grep".to_owned(), "read_file".to_owned()]
            );
        }
    }

    #[tokio::test]
    async fn w26_unknown_agent_worker_with_a_persona_should_get_only_what_both_allow() {
        // Persona [read, shell]: narrowed to read_file + shell; the made-up
        // agent then keeps only the read-only one.
        for request in w26_worker_tools(&["read", "shell"]).await {
            assert_eq!(request, vec!["read_file".to_owned()], "{request:?}");
        }
    }


    // ---- W27 / W28 (AskRisky) in the TUI (docs/plans/worker-tools-spec.md) ----
    //
    // Pins a function the TUI loop's cancel path must use instead of
    // `while agent_rx.try_recv().is_ok() {}`:
    //
    //     fn drain_after_cancel(
    //         app: &mut App,
    //         rx: &mut mpsc::Receiver<AgentEvent>,
    //         step_tx: Option<&mpsc::Sender<hone_proto::StepAction>>,
    //     )
    //
    // Synchronous. It empties `rx`; each dropped `ToolCallPaused` is answered
    // with `StepAction::Deny("turn cancelled")` on `step_tx`; a dropped `Done`
    // still updates the session's cost and token counts; no menu is left open.

    #[tokio::test]
    async fn w27_drain_after_cancel_should_deny_dropped_pauses_keep_done_costs_and_open_no_menu() {
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(16);
        let (step_tx, mut step_rx) = mpsc::channel::<hone_proto::StepAction>(16);
        let paused = |id: &str| AgentEvent::ToolCallPaused {
            id: id.to_owned(),
            name: "shell".to_owned(),
            args: serde_json::json!({}),
            worker: None,
        };
        let usage = hone_proto::TokenUsage {
            input: 10,
            output: 5,
            cache_read: 0,
            cache_creation: 0,
        };
        tx.send(paused("p1")).await.unwrap();
        tx.send(AgentEvent::Done {
            usage,
            cost_usd: 0.5,
            plan: None,
        })
        .await
        .unwrap();
        tx.send(paused("p2")).await.unwrap();
        let mut app = App::new();

        drain_after_cancel(&mut app, &mut rx, Some(&step_tx));

        for _ in 0..2 {
            assert!(
                matches!(step_rx.try_recv(), Ok(hone_proto::StepAction::Deny(_))),
                "each dropped pause is denied"
            );
        }
        assert!(step_rx.try_recv().is_err(), "exactly two answers");
        assert!(rx.try_recv().is_err(), "the queue is empty");
        assert!((app.status().session_cost_usd - 0.5).abs() < 1e-9);
        assert_eq!(app.status().token_count, usage.total());
        assert!(!app.debugger_open, "no menu for the cancelled run");
    }

    #[tokio::test]
    async fn w27_drain_after_cancel_should_release_a_task_waiting_on_the_step_channel() {
        // A separate turn (like a post-turn fix run) is paused for approval and
        // holds the step lock; its prompt sits in the queue being drained.
        let dir = tempfile::tempdir().unwrap();
        let (config, _provider, log, step_tx) = wt_session(dir.path(), None);
        let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);
        let turn = tokio::spawn(async move {
            let mut agent = hone_core::Agent::new(config);
            agent.run_turn("go", tx).await
        });
        // Wait for its ToolCallPaused to be queued.
        tokio::time::timeout(Duration::from_secs(5), async {
            while rx.len() < 3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the turn should pause for approval");
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut app = App::new();

        drain_after_cancel(&mut app, &mut rx, Some(&step_tx));

        tokio::time::timeout(Duration::from_secs(5), turn)
            .await
            .expect("the denied turn must be released, not wait forever")
            .unwrap()
            .unwrap();
        assert!(log.lock().unwrap().is_empty(), "the denied call did not run");
    }

    // W28 under AskRisky (the TUI default); the W17 policy tests used Auto.

    fn w17_risky_policy_session(
        dir: &std::path::Path,
    ) -> (AgentConfig, WtLog, mpsc::Sender<hone_proto::StepAction>) {
        let (mut config, log, step_tx) = w17_session(dir, true);
        config.approval_policy = ToolApprovalPolicy::AskRisky {
            always_allowed: vec![],
        };
        (config, log, step_tx)
    }

    #[tokio::test]
    async fn w28_ask_risky_workers_always_for_a_policy_rule_should_cover_the_agents_next_turn() {
        let dir = std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
        let (config, log, step_tx) = w17_risky_policy_session(&dir);
        let agent = AgentHandle::spawn(config.clone());

        let run = w17_run(&config, &agent, &step_tx, w17_always()).await;
        assert_eq!(wt_pauses(&run), 1, "{run:?}");
        let turn = w17_turn(&agent, &step_tx, hone_proto::StepAction::Proceed).await;

        assert_eq!(wt_pauses(&turn), 0, "no menu of any kind: {turn:?}");
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn w28_ask_risky_agents_always_for_a_policy_rule_should_cover_the_runs_workers() {
        let dir = std::fs::canonicalize(tempfile::tempdir().unwrap().keep()).unwrap();
        let (config, log, step_tx) = w17_risky_policy_session(&dir);
        let agent = AgentHandle::spawn(config.clone());

        let turn = w17_turn(&agent, &step_tx, w17_always()).await;
        assert_eq!(wt_pauses(&turn), 1, "{turn:?}");
        let run = w17_run(&config, &agent, &step_tx, hone_proto::StepAction::Proceed).await;

        assert_eq!(wt_pauses(&run), 0, "no menu of any kind: {run:?}");
        assert_eq!(log.lock().unwrap().len(), 2);
    }

    // ---- W44: the inline wrapper's restore also runs when it is aborted ----

    #[tokio::test]
    async fn w44_aborted_inline_wrapper_should_still_run_its_restore_exactly_once() {
        let inner = tokio::spawn(std::future::pending::<()>());
        let (_inner_tx, inner_rx) = mpsc::channel::<AgentEvent>(8);
        let (outer_tx, _outer_rx) = mpsc::channel::<AgentEvent>(8);
        let restored = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let flag = Arc::clone(&restored);
        let wrapper = spawn_inline_turn_wrapper(inner, inner_rx, outer_tx, async move {
            flag.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
        // Let the wrapper start waiting for events.
        tokio::time::sleep(Duration::from_millis(50)).await;

        wrapper.abort();

        tokio::time::timeout(Duration::from_secs(2), async {
            while restored.load(std::sync::atomic::Ordering::SeqCst) == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the persona restore must run when the wrapper is aborted");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(restored.load(std::sync::atomic::Ordering::SeqCst), 1, "once");
    }

    #[tokio::test]
    async fn w44_inline_wrapper_ending_normally_should_run_its_restore_exactly_once() {
        let (inner_tx, inner_rx) = mpsc::channel::<AgentEvent>(8);
        let inner = tokio::spawn(async move { drop(inner_tx) });
        let (outer_tx, _outer_rx) = mpsc::channel::<AgentEvent>(8);
        let restored = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let flag = Arc::clone(&restored);
        let wrapper = spawn_inline_turn_wrapper(inner, inner_rx, outer_tx, async move {
            flag.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });

        tokio::time::timeout(Duration::from_secs(2), wrapper)
            .await
            .expect("wrapper ends")
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert_eq!(restored.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}

// ---------------------------------------------------------------------------
// Tests: CachingToolWrapper must forward `subjects()` and `is_read_only()`
// ---------------------------------------------------------------------------

#[cfg(test)]
mod caching_wrapper_forwarding_tests {
    use super::*;
    use hone_cache::InMemoryBackend;
    use hone_config::policy::{PathOp, Subject};
    use hone_tools::{SubjectContext, ToolInput};

    fn registry(cache_on: bool) -> ToolRegistry {
        let lsp: Arc<dyn LspBridge> = Arc::new(MockLspBridge::builder().build());
        let cache =
            cache_on.then(|| Arc::new(ToolResultCache::new(Arc::new(InMemoryBackend::new()))));
        build_tool_registry(lsp, None, None, cache, Vec::new())
    }

    fn sctx(wd: &std::path::Path) -> SubjectContext {
        SubjectContext {
            workdir: wd.to_path_buf(),
            home: None,
            case_insensitive: false,
        }
    }

    /// One input carrying every argument name the CLI tools read, so each
    /// tool extracts whatever it understands.
    fn union_input() -> ToolInput {
        ToolInput::new(serde_json::json!({
            "path": "notes.txt",
            "file": "src/lib.rs",
            "url": "http://127.0.0.1/x",
            "pattern": "*.rs",
            "query": "q",
            "command": "ls",
            "name": "main",
        }))
    }

    #[test]
    fn wrapper_forwards_subjects_and_read_only_for_every_cli_tool() {
        let wd = tempfile::tempdir().unwrap();
        let wd = std::fs::canonicalize(wd.path()).unwrap();
        let cached = registry(true);
        let plain = registry(false);
        let ctx = sctx(&wd);
        let input = union_input();

        let mut names: Vec<&str> = cached.names();
        names.sort_unstable();
        assert!(!names.is_empty());
        let mut plain_names: Vec<&str> = plain.names();
        plain_names.sort_unstable();
        assert_eq!(plain_names, names, "cache on/off must register the same tools");

        for name in names {
            let w = cached.get(name).unwrap();
            let p = plain.get(name).unwrap();
            assert_eq!(
                w.is_read_only(),
                p.is_read_only(),
                "is_read_only not forwarded for {name}"
            );
            let ws = w.subjects(&input, &ctx);
            let ps = p.subjects(&input, &ctx);
            assert_eq!(ws.subjects, ps.subjects, "subjects not forwarded for {name}");
            assert_eq!(ws.unparsed, ps.unparsed, "unparsed not forwarded for {name}");
        }
    }

    #[test]
    fn wrapped_read_file_yields_path_read_of_workdir_relative_file() {
        let wd = tempfile::tempdir().unwrap();
        let wd = std::fs::canonicalize(wd.path()).unwrap();
        let reg = registry(true);
        let tool = reg.get("read_file").expect("read_file registered");

        let s = tool.subjects(
            &ToolInput::new(serde_json::json!({ "path": "notes.txt" })),
            &sctx(&wd),
        );

        assert_eq!(
            s.subjects,
            vec![Subject::Path {
                path: wd.join("notes.txt"),
                op: PathOp::Read,
            }]
        );
        assert!(tool.is_read_only());
    }

    #[test]
    fn wrapped_web_fetch_yields_host_subject() {
        let wd = tempfile::tempdir().unwrap();
        let wd = std::fs::canonicalize(wd.path()).unwrap();
        let reg = registry(true);
        let tool = reg.get("web_fetch").expect("web_fetch registered");
        let s = tool.subjects(
            &ToolInput::new(serde_json::json!({ "url": "http://127.0.0.1/x" })),
            &sctx(&wd),
        );
        assert!(
            s.subjects.iter().any(|x| matches!(x, Subject::Host(_))),
            "{:?}",
            s.subjects
        );
    }
}

// ---------------------------------------------------------------------------
// Tests: `--approve <RULE_ID>` (P2.3 spec section 3)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod approve_flag_tests {
    use super::*;

    // Assumes `Cli` gains `approve: Vec<String>` (`#[arg(long, value_name = "RULE_ID")]`).

    #[test]
    fn approve_flag_when_repeated_should_collect_all_values_in_order() {
        let cli = Cli::try_parse_from(["hone", "--approve", "a", "--approve", "b", "do it"])
            .expect("parses");
        assert_eq!(cli.approve, vec!["a".to_owned(), "b".to_owned()]);
    }

    #[test]
    fn approve_flag_when_absent_should_be_empty() {
        let cli = Cli::try_parse_from(["hone", "do it"]).expect("parses");
        assert!(cli.approve.is_empty());
    }

    #[test]
    fn approve_flag_when_value_has_colon_should_parse_verbatim() {
        let cli = Cli::try_parse_from(["hone", "--approve", "user:3", "do it"]).expect("parses");
        assert_eq!(cli.approve, vec!["user:3".to_owned()]);
    }

    #[test]
    fn approve_flag_when_missing_value_should_fail_to_parse() {
        assert!(Cli::try_parse_from(["hone", "--approve"]).is_err());
    }
}
