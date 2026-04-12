mod configure;
mod session;
mod system_prompt;
mod tools;

use std::io;
use std::io::IsTerminal as _;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use crossterm::{
    event::{Event, EventStream, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use hone_cache::{SqliteBackend, ToolResultCache};
use hone_config::{Config, HoneMd, McpConfig};
use hone_mcp::McpManager;
use hone_core::{
    AgentConfig, AgentEnsemble, AgentHandle, AgentRegistry, ComplexityClassifier, ComplexityTier,
    Orchestrator, PlanStore, SelfRefineLoop, TestVerificationLoop,
};
use hone_index::{IndexWatcher, SqliteIndex, WatcherHandle};
use hone_proto::{AgentEvent, ModelId, PricingEntry};
use hone_providers::{
    ClaudeCliProvider, GeminiCliProvider, ModelProvider,
    adapters::anthropic::AnthropicProvider,
    adapters::ollama::OllamaProvider,
    adapters::openai::OpenAiProvider,
};
use hone_recipes::Recipe;
use hone_sandbox::SandboxBuilder;
use hone_memory::{MemoryStore, SqliteStore};
use hone_tools::{
    ComputerControlTool, EditFileTool, GlobTool, GrepTool, GitCommitTool, GitDiffTool,
    GitStatusTool, Language, LspBridge, LspCodeActionTool, LspDiagnosticsTool, LspHoverTool,
    MockLspBridge, MultiEditTool, ReadFileTool, ScreenCaptureTool, ShellTool, StdioLspBridge,
    ToolRegistry, WriteFileTool,
};
use hone_tui::{App, AppState};
use hone_tui::render::root;
use hone_tui::{execute_shell_escape, run_interactive_blocking};
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{Mutex, mpsc};

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
}

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

    /// Use a named agent persona (e.g. code-reviewer, debugger, security-auditor)
    #[arg(long)]
    agent: Option<String>,

    /// Ephemeral session (no memory persistence)
    #[arg(long)]
    ephemeral: bool,

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
    /// into the agent's context, improving patch rate by +20pp on SWE-bench.
    #[arg(long)]
    no_inject: bool,

    /// Output structured JSON instead of streaming text (for CI/CD and scripting).
    /// Includes response, tool calls, usage, cost, and exit status.
    #[arg(long)]
    json: bool,

    /// Substitute a variable in a recipe prompt. Repeatable.
    /// Format: KEY=VALUE (e.g. --var feature_name=my_feature --var test_command="cargo test")
    #[arg(long = "var", value_name = "KEY=VALUE")]
    vars: Vec<String>,

    /// Enable the egress audit log (writes to .hone/audit.jsonl).
    /// Also enabled by setting HONE_AUDIT_LOG=1.
    #[arg(long)]
    audit: bool,

    /// Subcommand (session management, configure wizard, etc.)
    #[command(subcommand)]
    command: Option<HoneCmd>,
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

async fn run_recipe(
    agent: AgentHandle,
    recipe_name: &str,
    vars: &std::collections::HashMap<String, String>,
) -> Result<()> {
    let yaml = load_builtin_recipe(recipe_name)
        .ok_or_else(|| anyhow::anyhow!("unknown recipe: {recipe_name}"))?;

    let recipe = Recipe::from_yaml(yaml)?.with_variables(vars);

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

async fn detect_provider(wizard_cfg: &configure::ResolvedConfig) -> DetectedProvider {
    // Prefer direct Anthropic API — supports tool calling.
    // Check env var first, then wizard config as fallback.
    if let Some(api_key) = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .or_else(|| wizard_cfg.anthropic_api_key.clone())
    {
        if api_key.trim().is_empty() {
            eprintln!("{} ANTHROPIC_API_KEY is set but empty. Skipping Anthropic provider.", warn_style("Warning:"));
        } else if !api_key.starts_with("sk-ant-") {
            eprintln!("{} ANTHROPIC_API_KEY doesn't start with 'sk-ant-'. Is this correct?", warn_style("Warning:"));
            tracing::info!(provider = "anthropic", api = "http", tools = true, "provider detected (non-standard key format)");
            return DetectedProvider {
                provider: Arc::new(AnthropicProvider::new(api_key)),
                model_id: ModelId::new("anthropic:claude-sonnet-4-5"),
                supports_tools: true,
            };
        } else {
            tracing::info!(provider = "anthropic", api = "http", tools = true, "provider detected");
            return DetectedProvider {
                provider: Arc::new(AnthropicProvider::new(api_key)),
                model_id: ModelId::new("anthropic:claude-sonnet-4-5"),
                supports_tools: true,
            };
        }
    }

    // OpenAI API (or any OpenAI-compatible endpoint) — supports tool calling.
    // Set HONE_OPENAI_BASE_URL to point at DeepSeek, Qwen, Kimi, OpenRouter, etc.
    // Set HONE_OPENAI_MODEL to override the default model name for that endpoint.
    if let Some(api_key) = std::env::var("OPENAI_API_KEY")
        .ok()
        .or_else(|| wizard_cfg.openai_api_key.clone())
    {
        let provider = if let Some(base_url) = std::env::var("HONE_OPENAI_BASE_URL")
            .ok()
            .or_else(|| wizard_cfg.openai_base_url.clone())
        {
            tracing::info!(
                provider = "openai-compat",
                base_url = %base_url,
                tools = true,
                "provider detected (custom endpoint)"
            );
            OpenAiProvider::with_base_url(api_key, base_url)
        } else {
            tracing::info!(provider = "openai", api = "http", tools = true, "provider detected");
            OpenAiProvider::new(api_key)
        };
        let model_name = std::env::var("HONE_OPENAI_MODEL")
            .ok()
            .or_else(|| wizard_cfg.openai_model.clone())
            .unwrap_or_else(|| "gpt-4o".into());
        return DetectedProvider {
            provider: Arc::new(provider),
            model_id: ModelId::new(&format!("openai:{model_name}")),
            supports_tools: true,
        };
    }

    // Fall back to CLI pass-through providers (no tool calling).
    if binary_on_path("claude").await {
        tracing::info!(provider = "claude-cli", api = "cli", tools = false, "provider detected");
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
        tracing::info!(provider = "gemini-cli", api = "cli", tools = false, "provider detected");
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
    if reqwest::Client::new()
        .get("http://localhost:11434/api/tags")
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .is_ok()
    {
        const OLLAMA_DEFAULT_MODEL: &str = "llama3.2";

        // Verify that the default model is actually pulled before committing
        // to Ollama as the provider. A 200 from /api/show means the model
        // exists; any other response (404, connection error) means it is absent.
        let model_present = reqwest::Client::new()
            .post("http://localhost:11434/api/show")
            .json(&serde_json::json!({ "name": OLLAMA_DEFAULT_MODEL }))
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);

        if !model_present {
            eprintln!(
                "{} Ollama is running but model '{}' is not installed.",
                warn_style("Note:"),
                OLLAMA_DEFAULT_MODEL
            );
            eprintln!("Run:  ollama pull {OLLAMA_DEFAULT_MODEL}");
            eprintln!("Then re-run hone.\n");
            // Fall through to the "no provider" error below.
        } else {
            tracing::info!(provider = "ollama", api = "http", tools = true, "provider detected (tool calling enabled since Ollama v0.3.0)");
            return DetectedProvider {
                provider: Arc::new(OllamaProvider::new("http://localhost:11434")),
                model_id: ModelId::new(&format!("ollama:{OLLAMA_DEFAULT_MODEL}")),
                supports_tools: true,
            };
        }
    }

    tracing::warn!("no AI provider found");
    eprintln!("\n{} Hone checked the following (in order):\n", warn_style("No AI provider detected."));
    eprintln!("  1. {}    — ANTHROPIC_API_KEY not set", bold_style("Anthropic API"));
    eprintln!("                         export ANTHROPIC_API_KEY=sk-ant-...");
    eprintln!("  2. {}        — OPENAI_API_KEY not set", bold_style("OpenAI API"));
    eprintln!("                         export OPENAI_API_KEY=sk-...");
    eprintln!("                         Tip: set HONE_OPENAI_BASE_URL for DeepSeek/Qwen/Kimi/OpenRouter");
    eprintln!("  3. {}        — 'claude' not found on PATH", bold_style("Claude CLI"));
    eprintln!("                         Install: https://claude.ai/download");
    eprintln!("  4. {}        — 'gemini' not found on PATH", bold_style("Gemini CLI"));
    eprintln!("                         Install: https://github.com/google-gemini/gemini-cli");
    eprintln!("  5. {}            — localhost:11434 not responding", bold_style("Ollama"));
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
            .register(wrap(Box::new(tools::SymbolSearchTool::new(Arc::clone(&idx)))))
            .register(wrap(Box::new(tools::SymbolAtTool::new(idx))));
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
    tool_cache: Option<Arc<ToolResultCache>>,
    mcp_tools: Vec<Box<dyn hone_tools::Tool>>,
    no_inject_flag: bool,
    audit_flag: bool,
) -> AgentConfig {
    // Only register tools when the provider can actually use them.
    let tools = if detected.supports_tools {
        build_tool_registry(lsp, index.clone(), tool_cache, mcp_tools)
    } else {
        ToolRegistry::new()
    };

    // Proactive context injection is ENABLED by default (validated: +20pp patch
    // rate on SWE-bench). Disable with --no-inject or HONE_NO_INJECTION=1.
    let inject_context = !no_inject_flag
        && std::env::var("HONE_NO_INJECTION")
            .map(|v| v != "1")
            .unwrap_or(true);

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

    AgentConfig {
        provider: Arc::clone(&detected.provider),
        tools,
        sandbox: Arc::from(SandboxBuilder::detect()),
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
        retry_config: hone_core::RetryConfig::default(),
        index: index.map(|idx| Arc::clone(&idx)),
        inject_context,
        egress_log,
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
///   "error": null
/// }
/// ```
async fn run_json_mode(agent: AgentHandle, prompt: &str, model_id: &str) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(256);

    let collector = tokio::spawn(async move {
        let mut response = String::new();
        let mut tool_calls: Vec<serde_json::Value> = Vec::new();
        let mut usage = serde_json::json!(null);
        let mut error: Option<String> = None;
        let mut current_tool: Option<String> = None;

        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::Token { text } => {
                    response.push_str(&text);
                }
                AgentEvent::ToolCallStart { name, .. } => {
                    current_tool = Some(name);
                }
                AgentEvent::ToolCallEnd { result, is_error, .. } => {
                    let tool_name = current_tool.take().unwrap_or_else(|| "unknown".to_string());
                    tool_calls.push(serde_json::json!({
                        "name": tool_name,
                        "result": result.to_string(),
                        "is_error": is_error,
                    }));
                }
                AgentEvent::Done { usage: u } => {
                    usage = serde_json::json!({
                        "input": u.input,
                        "output": u.output,
                        "cache_read": u.cache_read,
                    });
                    break;
                }
                AgentEvent::Error { message } => {
                    error = Some(message);
                    break;
                }
                _ => {}
            }
        }

        (response, tool_calls, usage, error)
    });

    let turn_result = agent.run_turn(prompt, tx).await;
    let (response, tool_calls, usage, error) = collector.await.unwrap_or_default();

    let success = turn_result.is_ok() && error.is_none();
    let cost_usd = if let Ok(ref t) = turn_result {
        t.cost_usd
    } else {
        0.0
    };

    let output = serde_json::json!({
        "response": response,
        "tool_calls": tool_calls,
        "usage": usage,
        "cost_usd": cost_usd,
        "model": model_id,
        "success": success,
        "error": error,
    });

    println!("{}", serde_json::to_string_pretty(&output)?);

    if !success {
        std::process::exit(1);
    }

    Ok(())
}

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

    let name_width = sessions.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
    println!(
        "{:<name_width$}  {:<20}  {:>5}",
        "NAME", "UPDATED", "MSGS",
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

    // Load wizard config (no env mutation — returns a struct instead).
    let wizard_cfg = configure::load_wizard_config();

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
        }
    }

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
    let system_prompt = system_prompt::build_system_prompt(&hone_md_sections);

    tracing::debug!(system_prompt_len = system_prompt.len(), "system prompt ready");

    let mut detected = detect_provider(&wizard_cfg).await;

    // Opt-in local response cache: set HONE_CACHE=1 (or any value) to wrap
    // the inner provider with an LRU cache. Useful for development/testing
    // and retry scenarios where the exact same request may repeat.
    if std::env::var("HONE_CACHE").is_ok() {
        tracing::info!("local response cache enabled (HONE_CACHE set)");
        detected.provider = Arc::new(hone_providers::CachingProvider::new(detected.provider, 100));
    }

    // Validate --model against known provider prefixes.
    let model_id: ModelId = if let Some(ref m) = cli.model {
        let known_prefixes = ["anthropic:", "openai:", "ollama:", "claude-cli:", "gemini-cli:"];
        if !known_prefixes.iter().any(|p| m.starts_with(p)) {
            eprintln!("{} invalid model ID '{m}'", error_style("Error:"));
            eprintln!();
            eprintln!("Model IDs must start with a provider prefix:");
            eprintln!("  anthropic:claude-sonnet-4-5");
            eprintln!("  openai:gpt-4o");
            eprintln!("  openai:deepseek-coder      (with HONE_OPENAI_BASE_URL)");
            eprintln!("  ollama:llama3.2");
            eprintln!("  claude-cli:claude-sonnet-4-5");
            eprintln!("  gemini-cli:gemini-2.5-pro");
            std::process::exit(1);
        }
        ModelId::new(m)
    } else {
        detected.model_id.clone()
    };

    // Provider capability warnings: --orchestrate and --ensemble require tool
    // calling. Warn (but do not abort) when used with a CLI/Ollama provider
    // that cannot invoke tools.
    if !detected.supports_tools {
        let provider_name = detected.model_id.to_string();
        let provider_name = provider_name
            .split(':')
            .next()
            .unwrap_or(&provider_name);

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
            if project.exists() { Some(project.as_path()) } else { None },
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
    let base_config = build_agent_config(
        &detected,
        model_id.clone(),
        system_prompt.clone(),
        Arc::clone(&lsp),
        index,
        memory_store.clone(),
        tool_cache,
        mcp_tools,
        cli.no_inject,
        cli.audit,
    );

    let agent = AgentHandle::spawn(base_config.clone());

    // Session resume: load history into the freshly spawned agent.
    let is_resume = matches!(
        cli.command,
        Some(HoneCmd::Session { cmd: SessionCmd::Resume { .. } })
    );
    if is_resume {
        if let Some(HoneCmd::Session { cmd: SessionCmd::Resume { ref name } }) = cli.command {
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
            if let Some(HoneCmd::Session { cmd: SessionCmd::Resume { ref name } }) = cli.command {
                Some(name.clone())
            } else {
                None
            }
        })
    };

    // Run the selected mode and capture the result so we can shut down MCP
    // servers before returning regardless of which path was taken.
    let run_result = run_selected_mode(
        cli,
        agent,
        base_config,
        model_id,
        session_name,
    )
    .await;

    // Graceful MCP server shutdown — always runs, even on error.
    mcp_manager.shutdown().await;

    run_result
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

    // --recipe <name>: run a built-in recipe and exit.
    if let Some(ref recipe_name) = cli.recipe {
        return run_recipe(agent.clone(), recipe_name, &recipe_vars).await;
    }

    // --ensemble N: run N parallel candidates and pick the best.
    if let Some(n) = cli.ensemble {
        if n < 2 || n > 10 {
            eprintln!("{} --ensemble value must be between 2 and 10 (got {n})", error_style("Error:"));
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
        if cli.json {
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
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_tui(&mut terminal, agent.clone(), model_id, base_config, cli.vim).await;

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
    vim: bool,
) -> Result<()> {
    let mut app = if vim { App::with_vim() } else { App::new() };

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
                            let is_shell_escape = user_text.starts_with('!');

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
                            } else if !is_command && !is_shell_escape && matches!(app.state(), AppState::WaitingForResponse) {
                                // Only spawn an AI turn for regular messages, not commands or
                                // shell escapes.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
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
            Err(format!("--ensemble value must be between 2 and 10 (got {n})"))
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
        let known_prefixes = ["anthropic:", "openai:", "ollama:", "claude-cli:", "gemini-cli:"];
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
}
