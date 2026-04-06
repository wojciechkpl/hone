use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{Event, EventStream, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use hone_config::{Config, HoneMd};
use hone_core::{Agent, AgentConfig, AgentRegistry, Orchestrator};
use hone_proto::{AgentEvent, ModelId, PricingEntry};
use hone_providers::{
    ClaudeCliProvider, GeminiCliProvider, MockProvider, ModelProvider,
    adapters::anthropic::AnthropicProvider,
    adapters::ollama::OllamaProvider,
    adapters::openai::OpenAiProvider,
};
use hone_recipes::Recipe;
use hone_sandbox::NullSandbox;
use hone_tools::{
    EditFileTool, GlobTool, GitCommitTool, GitDiffTool, GitStatusTool, ReadFileTool, ShellTool,
    ToolRegistry, WriteFileTool,
};
use hone_tui::{App, AppState};
use hone_tui::render::root;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::{Mutex, mpsc};

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

struct CliArgs {
    model: Option<String>,
    one_shot: Option<String>,
    recipe: Option<String>,
    list_recipes: bool,
    orchestrate: bool,
}

fn parse_args() -> CliArgs {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        std::process::exit(0);
    }

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("hone {}", env!("CARGO_PKG_VERSION"));
        std::process::exit(0);
    }

    let mut model: Option<String> = None;
    let mut one_shot: Option<String> = None;
    let mut recipe: Option<String> = None;
    let mut list_recipes = false;
    let mut orchestrate = false;
    let mut i = 1usize;

    while i < args.len() {
        match args[i].as_str() {
            "--model" | "-m" => {
                i += 1;
                if i < args.len() {
                    model = Some(args[i].clone());
                }
            }
            "--recipe" => {
                i += 1;
                if i < args.len() {
                    recipe = Some(args[i].clone());
                }
            }
            "--list-recipes" => {
                list_recipes = true;
            }
            "--orchestrate" => {
                orchestrate = true;
            }
            // Accepted but ignored flags for forward-compatibility.
            "--agent" | "--ephemeral" => {
                // Consume the next argument if the flag takes a value.
                if args[i].as_str() == "--agent" {
                    i += 1;
                }
            }
            arg if !arg.starts_with('-') => {
                one_shot = Some(arg.to_owned());
            }
            _ => {}
        }
        i += 1;
    }

    CliArgs { model, one_shot, recipe, list_recipes, orchestrate }
}

fn print_help() {
    println!("hone {}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Usage: hone [OPTIONS] [PROMPT]");
    println!();
    println!("Arguments:");
    println!("  PROMPT  Run a single prompt and exit (one-shot mode)");
    println!();
    println!("Options:");
    println!("  -m, --model <ID>        Model ID to use (e.g. anthropic:claude-sonnet-4-5)");
    println!("      --recipe <NAME>     Run a named built-in recipe");
    println!("      --list-recipes      List all available built-in recipes");
    println!("      --orchestrate       Run prompt through the multi-agent orchestrator");
    println!("      --agent <NAME>      Agent name (reserved for future use)");
    println!("      --ephemeral         Ephemeral session (reserved for future use)");
    println!("  -h, --help              Print this help message");
    println!("  -V, --version           Print version");
    println!();
    println!("Environment:");
    println!("  ANTHROPIC_API_KEY  Anthropic API key (enables tool use)");
    println!("  RUST_LOG           Tracing filter (e.g. RUST_LOG=debug)");
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

async fn run_recipe(agent: Arc<Mutex<Agent>>, recipe_name: &str) -> Result<()> {
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
        {
            let mut guard = agent.lock().await;
            if let Err(e) = guard.run_turn(&prompt, tx).await {
                eprintln!("  Step failed: {e}");
            }
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
        tracing::info!("provider: Anthropic API (tool use enabled)");
        return DetectedProvider {
            provider: Arc::new(AnthropicProvider::new(api_key)),
            model_id: ModelId::new("anthropic:claude-sonnet-4-5"),
            supports_tools: true,
        };
    }

    // OpenAI API — supports tool calling.
    if let Ok(api_key) = std::env::var("OPENAI_API_KEY") {
        tracing::info!("provider: OpenAI API (tool use enabled)");
        return DetectedProvider {
            provider: Arc::new(OpenAiProvider::new(api_key)),
            model_id: ModelId::new("openai:gpt-4o"),
            supports_tools: true,
        };
    }

    // Fall back to CLI pass-through providers (no tool calling).
    if binary_on_path("claude").await {
        tracing::info!("provider: Claude CLI detected (no tool use)");
        return DetectedProvider {
            provider: Arc::new(ClaudeCliProvider::new()),
            model_id: ModelId::new("claude-cli:claude-sonnet-4-5"),
            supports_tools: false,
        };
    }

    if binary_on_path("gemini").await {
        tracing::info!("provider: Gemini CLI detected (no tool use)");
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
        tracing::info!("provider: Ollama (local, no tool use)");
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

/// Build a `ToolRegistry` with all available native tools.
fn build_tool_registry() -> ToolRegistry {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ReadFileTool));
    registry.register(Box::new(WriteFileTool));
    registry.register(Box::new(EditFileTool));
    registry.register(Box::new(GlobTool));
    registry.register(Box::new(ShellTool));
    registry.register(Box::new(GitStatusTool));
    registry.register(Box::new(GitDiffTool));
    registry.register(Box::new(GitCommitTool));
    registry
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
    let hone_md_path = PathBuf::from("HONE.md");

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

    let hone_md = if hone_md_path.exists() {
        std::fs::read_to_string(&hone_md_path)
            .map_err(|e| tracing::warn!("Failed to read HONE.md: {e}"))
            .ok()
            .map(|content| HoneMd::parse(&content))
    } else {
        None
    };

    (config, hone_md)
}

// ---------------------------------------------------------------------------
// Agent construction
// ---------------------------------------------------------------------------

fn build_agent(
    detected: &DetectedProvider,
    model_id: ModelId,
    system_prompt: String,
) -> Agent {
    // Only register tools when the provider can actually use them.
    let tools = if detected.supports_tools {
        build_tool_registry()
    } else {
        ToolRegistry::new()
    };

    let config = AgentConfig {
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
    };

    Agent::new(config)
}

// ---------------------------------------------------------------------------
// Agent turn dispatch
// ---------------------------------------------------------------------------

fn spawn_agent_turn(
    agent: Arc<Mutex<Agent>>,
    user_text: String,
    tx: mpsc::Sender<AgentEvent>,
) {
    tokio::spawn(async move {
        let mut guard = agent.lock().await;
        if let Err(e) = guard.run_turn(&user_text, tx.clone()).await {
            let _ = tx
                .send(AgentEvent::Error {
                    message: e.to_string(),
                })
                .await;
        }
    });
}

// ---------------------------------------------------------------------------
// Orchestrate mode
// ---------------------------------------------------------------------------

async fn run_orchestrate(
    lead_config: AgentConfig,
    prompt: &str,
) -> Result<()> {
    let mut registry = AgentRegistry::new();
    registry.load_builtins();

    let mut orchestrator = Orchestrator::new(lead_config, registry, 4);

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
// One-shot mode
// ---------------------------------------------------------------------------

async fn run_one_shot(agent: Arc<Mutex<Agent>>, prompt: &str) -> Result<()> {
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

    {
        let mut guard = agent.lock().await;
        guard.run_turn(prompt, tx).await?;
    }

    let _ = printer.await;
    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// TUI event loop
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = parse_args();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(io::stderr)
        .init();

    let (config, hone_md) = load_project_config().await;

    if config.is_some() {
        tracing::info!("config loaded successfully");
    }

    // Build the system prompt, injecting any HONE.md sections.
    let mut system_prompt = "You are Hone, an AI coding assistant.".to_string();
    if let Some(ref md) = hone_md {
        let mut keys: Vec<&String> = md.sections.keys().collect();
        keys.sort(); // deterministic ordering
        for key in keys {
            let content = &md.sections[key];
            system_prompt.push_str(&format!("\n\n## {key}\n{content}"));
        }
        tracing::info!(
            sections = md.sections.len(),
            "HONE.md loaded and injected into system prompt"
        );
    }

    tracing::debug!(system_prompt_len = system_prompt.len(), "system prompt ready");

    let detected = detect_provider().await;

    let model_id: ModelId = cli
        .model
        .as_deref()
        .map(ModelId::new)
        .unwrap_or_else(|| detected.model_id.clone());

    let agent = Arc::new(Mutex::new(build_agent(&detected, model_id.clone(), system_prompt.clone())));

    // --list-recipes: print available recipes and exit.
    if cli.list_recipes {
        list_builtin_recipes();
        return Ok(());
    }

    // --recipe <name>: run a built-in recipe and exit.
    if let Some(ref recipe_name) = cli.recipe {
        return run_recipe(agent, recipe_name).await;
    }

    // Orchestrate mode: decompose → execute workers → merge.
    if cli.orchestrate {
        let prompt = cli
            .one_shot
            .clone()
            .unwrap_or_else(|| "Please describe what you need help with.".to_owned());

        let lead_config = AgentConfig {
            provider: Arc::clone(&detected.provider),
            tools: if detected.supports_tools {
                build_tool_registry()
            } else {
                ToolRegistry::new()
            },
            sandbox: Arc::new(NullSandbox),
            system_prompt: system_prompt.clone(),
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
        };

        return run_orchestrate(lead_config, &prompt).await;
    }

    // One-shot mode: run a single turn, print to stdout, and exit.
    if let Some(ref prompt) = cli.one_shot {
        return run_one_shot(agent, prompt).await;
    }

    // TUI mode.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_tui(&mut terminal, agent, model_id).await;

    // Always restore terminal even on error.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    agent: Arc<Mutex<Agent>>,
    model_id: ModelId,
) -> Result<()> {
    let mut app = App::new();

    // Show which model is active in the status bar.
    app.status_mut().model = model_id.to_string();

    let (agent_tx, mut agent_rx) = mpsc::channel::<AgentEvent>(256);

    let tick = Duration::from_millis(100);

    let mut events = EventStream::new();

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
                                spawn_agent_turn(
                                    Arc::clone(&agent),
                                    user_text,
                                    agent_tx.clone(),
                                );
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
        }

        // Drain any remaining agent events that arrived during the select branch.
        while let Ok(event) = agent_rx.try_recv() {
            app.handle_agent_event(event);
        }

        app.tick();
    }

    Ok(())
}
