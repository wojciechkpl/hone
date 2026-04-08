mod system_prompt;
mod tools;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;
use crossterm::{
    event::{Event, EventStream, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use hone_config::{Config, HoneMd};
use hone_core::{
    AgentConfig, AgentEnsemble, AgentHandle, AgentRegistry, ComplexityClassifier, ComplexityTier,
    Orchestrator, PlanStore, SelfRefineLoop, TestVerificationLoop,
};
use hone_index::{IndexWatcher, SqliteIndex, WatcherHandle};
use hone_proto::{AgentEvent, ModelId, PricingEntry};
use hone_providers::{
    ClaudeCliProvider, GeminiCliProvider, MockProvider, ModelProvider,
    adapters::anthropic::AnthropicProvider,
    adapters::ollama::OllamaProvider,
    adapters::openai::OpenAiProvider,
};
use hone_recipes::Recipe;
use hone_sandbox::NullSandbox;
use hone_memory::{MemoryStore, SqliteStore};
use hone_tools::{
    EditFileTool, GlobTool, GrepTool, GitCommitTool, GitDiffTool, GitStatusTool, Language,
    LspBridge, LspCodeActionTool, LspDiagnosticsTool, LspHoverTool, MockLspBridge, MultiEditTool,
    ReadFileTool, ShellTool, StdioLspBridge, ToolRegistry, WriteFileTool,
};
use hone_tui::{App, AppState};
use hone_tui::render::root;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{Mutex, mpsc};

// ---------------------------------------------------------------------------
// CLI argument parsing via clap
// ---------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "hone", version, about = "The universal terminal AI coding agent")]
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

    /// Agent name (reserved for future use)
    #[arg(long)]
    agent: Option<String>,

    /// Ephemeral session (no memory persistence)
    #[arg(long)]
    ephemeral: bool,

    /// Run N parallel candidates with varied temperatures and pick the best
    /// response using the MASAI Ranker pattern (requires N >= 2)
    #[arg(long, value_name = "N")]
    ensemble: Option<u32>,
}

// ---------------------------------------------------------------------------
// Built-in recipe loading
// ---------------------------------------------------------------------------

fn load_builtin_recipe(name: &str) -> Option<&'static str> {
    match name {
        "tdd-feature" => Some(include_str!(
            "../../../crates/hone-recipes/recipes/tdd-feature.yaml"
        )),
        "bug-fix" => Some(include_str!(
            "../../../crates/hone-recipes/recipes/bug-fix.yaml"
        )),
        "code-review" => Some(include_str!(
            "../../../crates/hone-recipes/recipes/code-review.yaml"
        )),
        "refactor" => Some(include_str!(
            "../../../crates/hone-recipes/recipes/refactor.yaml"
        )),
        _ => None,
    }
}

fn list_builtin_recipes() {
    println!("Available recipes:");
    println!(
        "  {:<16} Implement a feature using Test-Driven Development",
        "tdd-feature"
    );
    println!(
        "  {:<16} Fix a bug using systematic debugging with regression test",
        "bug-fix"
    );
    println!(
        "  {:<16} Perform a comprehensive code review",
        "code-review"
    );
    println!(
        "  {:<16} Safely refactor code with test verification at each step",
        "refactor"
    );
}

// ---------------------------------------------------------------------------
// Recipe execution mode
// ---------------------------------------------------------------------------

async fn run_recipe(agent: AgentHandle, recipe_name: &str) -> Result<()> {
    let yaml = load_builtin_recipe(recipe_name)
        .ok_or_else(|| anyhow::anyhow!("unknown recipe: {recipe_name}"))?;

    let recipe = Recipe::from_yaml(yaml)?;

    println!("Running recipe: {} ({} steps)", recipe.name, recipe.steps.len());
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

async fn detect_provider() -> DetectedProvider {
    // Prefer direct Anthropic API — supports tool calling.
    if let Ok(api_key) = std::env::var("ANTHROPIC_API_KEY") {
        tracing::info!(provider = "anthropic", api = "http", tools = true, "provider detected");
        return DetectedProvider {
            provider: Arc::new(AnthropicProvider::new(api_key)),
            model_id: ModelId::new("anthropic:claude-sonnet-4-5"),
            supports_tools: true,
        };
    }

    // OpenAI API — supports tool calling.
    if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        tracing::info!(provider = "openai", api = "http", tools = true, "provider detected");
        return DetectedProvider {
            provider: Arc::new(OpenAiProvider::new(api_key)),
            model_id: ModelId::new("openai:gpt-4o"),
            supports_tools: true,
        };
    }

    // Fall back to CLI pass-through providers (no tool calling).
    if binary_on_path("claude").await {
        tracing::info!(provider = "claude-cli", api = "cli", tools = false, "provider detected");
        return DetectedProvider {
            provider: Arc::new(ClaudeCliProvider::new()),
            model_id: ModelId::new("claude-cli:claude-sonnet-4-5"),
            supports_tools: false,
        };
    }

    if binary_on_path("gemini").await {
        tracing::info!(provider = "gemini-cli", api = "cli", tools = false, "provider detected");
        return DetectedProvider {
            provider: Arc::new(GeminiCliProvider::new()),
            model_id: ModelId::new("gemini-cli:gemini-2.5-pro"),
            supports_tools: false,
        };
    }

    // Check if Ollama is running locally (no tool use, but real local inference).
    if reqwest::Client::new()
        .get("http://localhost:11434/api/tags")
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok()
    {
        tracing::info!(provider = "ollama", api = "http", tools = false, "provider detected");
        return DetectedProvider {
            provider: Arc::new(OllamaProvider::new("http://localhost:11434")),
            model_id: ModelId::new("ollama:llama3.2"),
            supports_tools: false,
        };
    }

    tracing::warn!(
        "provider: no provider found (ANTHROPIC_API_KEY, OPENAI_API_KEY, claude, gemini, ollama) — \
         falling back to MockProvider. Set ANTHROPIC_API_KEY or OPENAI_API_KEY for real completions."
    );
    eprintln!(concat!(
        "\nNo AI provider found. To use Hone:\n",
        "  - Set ANTHROPIC_API_KEY for Anthropic API (with tool use)\n",
        "  - Set OPENAI_API_KEY for OpenAI API (with tool use)\n",
        "  - Install Claude CLI: https://claude.ai/download\n",
        "  - Install Gemini CLI: https://github.com/google-gemini/gemini-cli\n",
        "  - Or run Ollama locally on port 11434\n",
    ));
    DetectedProvider {
        provider: Arc::new(MockProvider::new(vec![
            "No AI provider found. ".to_owned(),
            "Set ANTHROPIC_API_KEY, OPENAI_API_KEY, or install the Claude CLI: https://claude.ai/download"
                .to_owned(),
        ])),
        model_id: ModelId::new("mock:mock-model"),
        supports_tools: false,
    }
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
fn build_tool_registry(lsp: Arc<dyn LspBridge>, index: Option<Arc<SqliteIndex>>) -> ToolRegistry {
    let mut builder = ToolRegistry::builder()
        .register(Box::new(ReadFileTool))
        .register(Box::new(WriteFileTool))
        .register(Box::new(EditFileTool))
        .register(Box::new(MultiEditTool))
        .register(Box::new(GlobTool))
        .register(Box::new(GrepTool))
        .register(Box::new(ShellTool))
        .register(Box::new(GitStatusTool))
        .register(Box::new(GitDiffTool))
        .register(Box::new(GitCommitTool))
        .register(Box::new(LspDiagnosticsTool::new(Arc::clone(&lsp))))
        .register(Box::new(LspHoverTool::new(Arc::clone(&lsp))))
        .register(Box::new(LspCodeActionTool::new(lsp)));

    if let Some(idx) = index {
        builder = builder
            .register(Box::new(tools::SymbolSearchTool::new(Arc::clone(&idx))))
            .register(Box::new(tools::SymbolAtTool::new(idx)));
    }

    builder.build()
}

// ---------------------------------------------------------------------------
// Config and HONE.md loading
// ---------------------------------------------------------------------------

/// Load user-level and project-level config, plus HONE.md from the current
/// directory.
///
/// Priority: project `.hone/config.toml` > `~/.config/hone/config.toml` >
/// built-in defaults.  Errors at any layer are logged and silently skipped so
/// that a missing or malformed config never prevents the binary from starting.
async fn load_project_config() -> (Option<Config>, Option<HoneMd>) {
    let user_config_path: Option<PathBuf> = std::env::var("HOME")
        .ok()
        .map(|home| PathBuf::from(home).join(".config/hone/config.toml"));

    let project_config_path = PathBuf::from(".hone/config.toml");

    let config = Config::load(
        user_config_path.as_deref(),
        if project_config_path.exists() {
            Some(project_config_path.as_path())
        } else {
            None
        },
    )
    .map_err(|e| {
        tracing::warn!("Failed to load config: {e}");
    })
    .ok();

    let cwd = std::env::current_dir().unwrap_or_default();
    let hone_md = {
        let merged = HoneMd::load_hierarchical(&cwd);
        if merged.sections.is_empty() {
            None
        } else {
            Some(merged)
        }
    };

    (config, hone_md)
}

// ---------------------------------------------------------------------------
// Agent construction
// ---------------------------------------------------------------------------

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
    memory_store: Option<Arc<Mutex<dyn MemoryStore>>>,
) -> AgentConfig {
    // Only register tools when the provider can actually use them.
    let tools = if detected.supports_tools {
        build_tool_registry(lsp, index)
    } else {
        ToolRegistry::new()
    };

    AgentConfig {
        provider: Arc::clone(&detected.provider),
        tools,
        sandbox: Arc::new(NullSandbox),
        system_prompt,
        model: model_id.clone(),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: model_id.clone(),
            input_per_million: 3.0,
            output_per_million: 15.0,
            cache_read_per_million: None,
        },
        working_dir: std::env::current_dir().unwrap_or_default(),
        memory_store,
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

// ---------------------------------------------------------------------------
// Agent turn dispatch — simple (direct)
// ---------------------------------------------------------------------------

fn spawn_agent_turn(
    agent: AgentHandle,
    user_text: String,
    tx: mpsc::Sender<AgentEvent>,
) {
    tokio::spawn(async move {
        // Use an intermediate channel so we can observe events before
        // forwarding them to the caller and track which tools were used.
        let (inner_tx, mut inner_rx) = mpsc::channel::<AgentEvent>(256);

        let agent_clone = agent.clone();
        let turn_task = tokio::spawn(async move {
            agent_clone.run_turn(&user_text, inner_tx).await
        });

        let mut tools_used: Vec<String> = Vec::new();

        while let Some(event) = inner_rx.recv().await {
            if let AgentEvent::ToolCallStart { ref name, .. } = event {
                tools_used.push(name.clone());
            }
            let is_terminal = matches!(
                event,
                AgentEvent::Done { .. } | AgentEvent::Error { .. }
            );
            let _ = tx.send(event).await;
            if is_terminal {
                break;
            }
        }

        // Drain the turn task result (errors were already forwarded through
        // the event channel above).
        let _ = turn_task.await;

        // After the turn completes, run test verification if applicable.
        maybe_run_test_verification(&agent, &tx, &tools_used).await;
    });
}

// ---------------------------------------------------------------------------
// Agent turn dispatch — medium (agent + self-refine)
// ---------------------------------------------------------------------------

fn spawn_medium_turn(
    agent: AgentHandle,
    user_text: String,
    tx: mpsc::Sender<AgentEvent>,
) {
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

        let mut tools_used: Vec<String> = Vec::new();
        let mut had_error = false;

        while let Some(event) = inner_rx.recv().await {
            if let AgentEvent::ToolCallStart { ref name, .. } = event {
                tools_used.push(name.clone());
            }
            let is_terminal = matches!(
                event,
                AgentEvent::Done { .. } | AgentEvent::Error { .. }
            );
            if matches!(event, AgentEvent::Error { .. }) {
                had_error = true;
            }
            let _ = tx.send(event).await;
            if is_terminal {
                break;
            }
        }

        // Surface any error that the refine task returned but wasn't forwarded.
        if let Ok(Err(e)) = refine_task.await {
            if !had_error {
                let _ = tx
                    .send(AgentEvent::Error {
                        message: e.to_string(),
                    })
                    .await;
            }
            return;
        }

        if had_error {
            return;
        }

        // After the self-refine turn completes, run test verification.
        // This replaces the previous unconditional detect_test_cmd call and
        // respects the code-modifying tool guard, so Q&A turns don't trigger
        // an unnecessary test run.
        maybe_run_test_verification(&agent, &tx, &tools_used).await;
    });
}

// ---------------------------------------------------------------------------
// Agent turn dispatch — complex (orchestrator as spawned task)
// ---------------------------------------------------------------------------

fn spawn_orchestrated_turn(
    lead_config: AgentConfig,
    user_text: String,
    tx: mpsc::Sender<AgentEvent>,
) {
    tokio::spawn(async move {
        let mut registry = AgentRegistry::new();
        registry.load_builtins();

        let plan_store = PlanStore::default_for(&lead_config.working_dir);
        let mut orchestrator = Orchestrator::new(lead_config, registry, 4)
            .with_plan_store(plan_store);

        match orchestrator.run(&user_text, tx.clone()).await {
            Ok(result) => {
                let summary = format!(
                    "\n\n--- Orchestration complete ---\n{}\n\nTotal cost: ${:.6} | workers: {}",
                    result.merged_response,
                    result.total_cost_usd,
                    result.workers.len()
                );
                let _ = tx
                    .send(AgentEvent::Token { text: summary })
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
    });
}

// ---------------------------------------------------------------------------
// Orchestrate mode (blocking, for the --orchestrate one-shot path)
// ---------------------------------------------------------------------------

async fn run_orchestrate(
    lead_config: AgentConfig,
    prompt: &str,
) -> Result<()> {
    let mut registry = AgentRegistry::new();
    registry.load_builtins();

    let plan_store = PlanStore::default_for(&lead_config.working_dir);
    let mut orchestrator = Orchestrator::new(lead_config, registry, 4)
        .with_plan_store(plan_store);

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Token { text } => {
                    print!("{text}");
                    use io::Write;
                    let _ = io::stdout().flush();
                }
                AgentEvent::PlanCreated { goal, subtask_count } => {
                    println!("\n[orchestrator] plan: {goal} ({subtask_count} subtasks)");
                }
                AgentEvent::WorkerStarted { task_id, agent, description } => {
                    println!("[{task_id}] starting {agent}: {description}");
                }
                AgentEvent::WorkerCompleted { task_id, agent, status, cost_usd } => {
                    println!("[{task_id}] {agent}: {status} (${cost_usd:.4})");
                }
                AgentEvent::Done { .. } => {}
                AgentEvent::Error { message } => {
                    eprintln!("\nerror: {message}");
                    break;
                }
                _ => {}
            }
        }
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

/// Run a prompt through N parallel candidates with varied temperatures, then
/// let a ranker agent pick the best response (MASAI Ranker pattern).
async fn run_ensemble(
    base_config: &AgentConfig,
    prompt: &str,
    n: u32,
) -> Result<()> {
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

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Token { text } => {
                    print!("{text}");
                    use io::Write;
                    let _ = io::stdout().flush();
                }
                AgentEvent::Done { .. } => break,
                AgentEvent::Error { message } => {
                    eprintln!("\nerror: {message}");
                    break;
                }
                _ => {}
            }
        }
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

async fn run_one_shot(agent: AgentHandle, prompt: &str) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

    // Drain events and print tokens while the agent runs inline.
    let printer = tokio::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Token { text } => {
                    print!("{text}");
                    use io::Write;
                    let _ = io::stdout().flush();
                }
                AgentEvent::Done { .. } => break,
                AgentEvent::Error { message } => {
                    eprintln!("\nerror: {message}");
                    break;
                }
                _ => {}
            }
        }
    });

    agent.run_turn(prompt, tx).await?;

    let _ = printer.await;
    println!();
    Ok(())
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
    /// The opened memory store, wrapped for shared async mutation.
    memory: Option<Arc<Mutex<dyn MemoryStore>>>,
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
async fn init_persistence(ephemeral: bool) -> PersistenceHandles {
    if ephemeral {
        tracing::info!("ephemeral mode: skipping persistence init");
        return PersistenceHandles { memory: None, index: None, _watcher: None };
    }

    let dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("failed to create data dir {}: {e}", dir.display());
        return PersistenceHandles { memory: None, index: None, _watcher: None };
    }

    // Memory store
    let memory_path = dir.join("memory.db");
    let memory: Option<Arc<Mutex<dyn MemoryStore>>> = match SqliteStore::open(&memory_path).await {
        Ok(store) => {
            tracing::info!(path = %memory_path.display(), "memory store initialised");
            Some(Arc::new(Mutex::new(store)))
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

    PersistenceHandles { memory, index: index_arc, _watcher: watcher_handle }
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

    let (config, hone_md) = load_project_config().await;

    if config.is_some() {
        tracing::info!("config loaded successfully");
    }

    // Initialise SQLite persistence (non-blocking; errors are warnings only).
    // The returned handles keep the memory store and file watcher alive for
    // the process lifetime.
    let persistence = init_persistence(cli.ephemeral).await;
    let memory_store = persistence.memory.clone();
    let index = persistence.index.clone();

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
    let system_prompt = system_prompt::build_system_prompt(&hone_md_sections);

    tracing::debug!(system_prompt_len = system_prompt.len(), "system prompt ready");

    let detected = detect_provider().await;

    // Validate --model against known provider prefixes.
    let model_id: ModelId = if let Some(ref m) = cli.model {
        let known_prefixes = ["anthropic:", "openai:", "ollama:", "claude-cli:", "gemini-cli:"];
        if !known_prefixes.iter().any(|p| m.starts_with(p)) {
            eprintln!(
                "Invalid model ID: '{m}'\nSupported model ID prefixes:\n  - anthropic:claude-...\n  - openai:gpt-... or openai:o3\n  - ollama:llama3.2 (etc)\n  - claude-cli:... (CLI pass-through)\n  - gemini-cli:..."
            );
        }
        ModelId::new(m)
    } else {
        detected.model_id.clone()
    };

    // Try to spawn an LSP bridge for the project language (falls back to mock
    // if the language server binary is not installed).
    let lsp = detect_lsp_bridge().await;

    // Build a single clonable config; share it across all code paths.
    let base_config = build_agent_config(
        &detected,
        model_id.clone(),
        system_prompt.clone(),
        Arc::clone(&lsp),
        index,
        memory_store.clone(),
    );

    let agent = AgentHandle::spawn(base_config.clone());

    // --list-recipes: print available recipes and exit.
    if cli.list_recipes {
        list_builtin_recipes();
        return Ok(());
    }

    // --recipe <name>: run a built-in recipe and exit.
    if let Some(ref recipe_name) = cli.recipe {
        return run_recipe(agent.clone(), recipe_name).await;
    }

    // --ensemble N: run N parallel candidates and pick the best.
    if let Some(n) = cli.ensemble {
        if n < 2 {
            eprintln!("Error: --ensemble requires N >= 2");
            return Ok(());
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

    // --orchestrate: full orchestrator for the supplied prompt (or a default).
    if cli.orchestrate {
        let prompt = cli
            .prompt
            .clone()
            .unwrap_or_else(|| "Please describe what you need help with.".to_owned());

        return run_orchestrate(base_config, &prompt).await;
    }

    // One-shot mode: run a single turn, print to stdout, and exit.
    if let Some(ref prompt) = cli.prompt {
        return run_one_shot(agent.clone(), prompt).await;
    }

    // TUI mode.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_tui(&mut terminal, agent.clone(), model_id, base_config).await;

    // Always restore terminal even on error.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
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

async fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    agent: AgentHandle,
    model_id: ModelId,
    base_config: AgentConfig,
) -> Result<()> {
    let mut app = App::new();

    // Show which model is active in the status bar.
    app.status_mut().model = model_id.to_string();

    let (agent_tx, mut agent_rx) = mpsc::channel::<AgentEvent>(256);

    let tick = Duration::from_millis(100);

    let mut events = EventStream::new();

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

    loop {
        if app.should_quit() {
            break;
        }

        terminal.draw(|frame| root::render(&app, frame))?;

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

                        if key.code == KeyCode::Enter
                            && !app.input().is_empty()
                            && matches!(app.state(), AppState::Idle | AppState::Error(_))
                        {
                            let user_text = app.input().buffer.clone();
                            let is_command = user_text.starts_with('/');

                            // Let the App update its own state.
                            app.handle_key(key);

                            // Only spawn an AI turn for regular messages, not commands.
                            if !is_command && matches!(app.state(), AppState::WaitingForResponse) {
                                // Adaptive routing: classify complexity and pick the
                                // appropriate execution strategy.
                                let tier = ComplexityClassifier::classify(&user_text);
                                match tier {
                                    ComplexityTier::Simple => {
                                        tracing::debug!("complexity: Simple — direct agent turn");
                                        spawn_agent_turn(
                                            agent.clone(),
                                            user_text,
                                            agent_tx.clone(),
                                        );
                                    }
                                    ComplexityTier::Medium => {
                                        tracing::debug!("complexity: Medium — self-refine turn");
                                        spawn_medium_turn(
                                            agent.clone(),
                                            user_text,
                                            agent_tx.clone(),
                                        );
                                    }
                                    ComplexityTier::Complex => {
                                        tracing::debug!("complexity: Complex — orchestrated turn");
                                        spawn_orchestrated_turn(
                                            base_config.clone(),
                                            user_text,
                                            agent_tx.clone(),
                                        );
                                    }
                                }
                            }
                        } else {
                            app.handle_key(key);
                        }

                        if app.should_quit() {
                            break;
                        }
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
                    app.handle_agent_event(event);
                }
            }

            // Tick timer for animations
            _ = tokio::time::sleep(tick) => {}

            // OS-level SIGINT (e.g. from a parent process or wrapper script).
            _ = os_signal(&mut sigint) => {
                tracing::info!("SIGINT received, shutting down");
                break;
            }
        }

        // Drain any remaining agent events that arrived during the select branch.
        while let Ok(event) = agent_rx.try_recv() {
            app.handle_agent_event(event);
        }

        // Refresh context-window stats in the status bar after every tick.
        // The actor handles the query asynchronously so we don't stall the event
        // loop — if the agent is busy the reply arrives on the next iteration.
        {
            let tokens = agent.context_tokens().await;
            app.status_mut().context_tokens = tokens;
        }

        app.tick();
    }

    Ok(())
}
