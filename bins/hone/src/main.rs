use std::io;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use futures::StreamExt;
use hone_proto::{AgentEvent, ModelId, ProviderEvent, TokenUsage};
use hone_providers::{ClaudeCliProvider, CompletionRequest, GeminiCliProvider, MockProvider, ModelProvider};
use hone_tui::{App, AppState};
use hone_tui::render::root;
use ratatui::{Terminal, backend::CrosstermBackend};
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// CLI argument parsing
// ---------------------------------------------------------------------------

struct CliArgs {
    model: Option<String>,
    one_shot: Option<String>,
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
    let mut i = 1usize;

    while i < args.len() {
        match args[i].as_str() {
            "--model" | "-m" => {
                i += 1;
                if i < args.len() {
                    model = Some(args[i].clone());
                }
            }
            // Accepted but ignored flags for forward-compatibility.
            "--recipe" | "--agent" | "--ephemeral" => {
                // Consume the next argument if the flag takes a value.
                if matches!(args[i].as_str(), "--recipe" | "--agent") {
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

    CliArgs { model, one_shot }
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
    println!("  -m, --model <ID>        Model ID to use (e.g. claude-cli:claude-sonnet-4-5)");
    println!("      --recipe <NAME>     Recipe name (reserved for future use)");
    println!("      --agent <NAME>      Agent name (reserved for future use)");
    println!("      --ephemeral         Ephemeral session (reserved for future use)");
    println!("  -h, --help              Print this help message");
    println!("  -V, --version           Print version");
    println!();
    println!("Environment:");
    println!("  RUST_LOG  Tracing filter (e.g. RUST_LOG=debug)");
}

// ---------------------------------------------------------------------------
// Provider detection
// ---------------------------------------------------------------------------

enum DetectedProvider {
    Claude(ClaudeCliProvider),
    Gemini(GeminiCliProvider),
    Mock(MockProvider),
}

impl DetectedProvider {
    fn model_id(&self) -> ModelId {
        match self {
            DetectedProvider::Claude(_) => ModelId::new("claude-cli:claude-sonnet-4-5"),
            DetectedProvider::Gemini(_) => ModelId::new("gemini-cli:gemini-2.5-pro"),
            DetectedProvider::Mock(_) => ModelId::new("mock:mock-model"),
        }
    }

    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = ProviderEvent> + Send>>, hone_providers::ProviderError> {
        match self {
            DetectedProvider::Claude(p) => p.complete(request).await,
            DetectedProvider::Gemini(p) => p.complete(request).await,
            DetectedProvider::Mock(p) => p.complete(request).await,
        }
    }
}

async fn binary_on_path(name: &str) -> bool {
    // `which` is available on macOS and Linux; on Windows `where` would be
    // needed, but Hone targets Unix only.
    tokio::process::Command::new("which")
        .arg(name)
        .output()
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn detect_provider() -> DetectedProvider {
    if binary_on_path("claude").await {
        tracing::info!("provider: Claude CLI detected");
        return DetectedProvider::Claude(ClaudeCliProvider::new());
    }

    if binary_on_path("gemini").await {
        tracing::info!("provider: Gemini CLI detected");
        return DetectedProvider::Gemini(GeminiCliProvider::new());
    }

    tracing::warn!(
        "provider: no CLI provider found (claude, gemini) — falling back to MockProvider. \
         Install the Claude CLI (https://claude.ai/download) for real completions."
    );
    DetectedProvider::Mock(MockProvider::new(vec![
        "No AI provider found. ".to_owned(),
        "Install the Claude CLI to get real completions: https://claude.ai/download".to_owned(),
    ]))
}

// ---------------------------------------------------------------------------
// One-shot mode: print the response to stdout without a TUI
// ---------------------------------------------------------------------------

async fn run_one_shot(provider: &DetectedProvider, model_id: ModelId, prompt: &str) -> Result<()> {
    use hone_proto::Message;

    let messages = vec![Message::user(prompt)];
    let request = CompletionRequest::new(model_id, messages);

    let mut stream = provider.complete(request).await?;

    while let Some(event) = stream.next().await {
        match event {
            ProviderEvent::Token { text } => {
                print!("{text}");
                // Flush so streaming tokens appear in real time.
                use io::Write;
                let _ = io::stdout().flush();
            }
            ProviderEvent::Done | ProviderEvent::Error { .. } => break,
            _ => {}
        }
    }

    println!();
    Ok(())
}

// ---------------------------------------------------------------------------
// Agent turn: spawn a task that streams events back through mpsc
// ---------------------------------------------------------------------------

fn spawn_turn(
    provider: Arc<DetectedProvider>,
    model_id: ModelId,
    messages: Vec<hone_proto::Message>,
    tx: mpsc::Sender<AgentEvent>,
) {
    tokio::spawn(async move {
        let request = CompletionRequest::new(model_id, messages);

        let stream = match provider.complete(request).await {
            Ok(s) => s,
            Err(e) => {
                let _ = tx
                    .send(AgentEvent::Error {
                        message: e.to_string(),
                    })
                    .await;
                return;
            }
        };

        tokio::pin!(stream);

        let mut usage = TokenUsage::default();

        while let Some(event) = stream.next().await {
            match event {
                ProviderEvent::Token { text } => {
                    let _ = tx.send(AgentEvent::Token { text }).await;
                }
                ProviderEvent::Usage(u) => {
                    usage = usage + u;
                }
                ProviderEvent::Done => {
                    let _ = tx.send(AgentEvent::Done { usage }).await;
                    return;
                }
                ProviderEvent::Error { message } => {
                    let _ = tx.send(AgentEvent::Error { message }).await;
                    return;
                }
                // Tool calls are not supported by CLI providers yet.
                ProviderEvent::ToolCall(_) => {}
            }
        }

        // Stream ended without Done — send Done so the TUI exits WaitingForResponse.
        let _ = tx.send(AgentEvent::Done { usage }).await;
    });
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

    let provider = Arc::new(detect_provider().await);

    let model_id: ModelId = cli
        .model
        .as_deref()
        .map(ModelId::new)
        .unwrap_or_else(|| provider.model_id());

    // One-shot mode: just print and exit.
    if let Some(ref prompt) = cli.one_shot {
        return run_one_shot(&provider, model_id, prompt).await;
    }

    // TUI mode.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_tui(&mut terminal, provider, model_id).await;

    // Always restore terminal even if we got an error.
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

async fn run_tui(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    provider: Arc<DetectedProvider>,
    model_id: ModelId,
) -> Result<()> {
    let mut app = App::new();

    // Show which model is active in the status bar.
    app.status_mut().model = model_id.to_string();

    // Conversation history sent to the provider on each turn.
    let mut history: Vec<hone_proto::Message> = Vec::new();

    let (agent_tx, mut agent_rx) = mpsc::channel::<AgentEvent>(256);

    let tick = Duration::from_millis(100);

    loop {
        terminal.draw(|frame| root::render(&app, frame))?;

        // Wait up to `tick` for a crossterm event; treat timeout as a tick.
        if event::poll(tick)? {
            if let Event::Key(key) = event::read()? {
                // Ctrl-C is handled inside handle_key, but also intercept
                // here to ensure we always quit cleanly.
                if key.code == KeyCode::Char('c')
                    && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                {
                    break;
                }

                if key.code == KeyCode::Enter
                    && !app.input().is_empty()
                    && matches!(app.state(), AppState::Idle | AppState::Error(_))
                {
                    // Check if this is a slash command BEFORE processing.
                    let user_text = app.input().buffer.clone();
                    let is_command = user_text.starts_with('/');

                    // Let the App update its own state.
                    app.handle_key(key);

                    // Only spawn an AI turn for regular messages, not commands.
                    if !is_command && matches!(app.state(), AppState::WaitingForResponse) {
                        history.push(hone_proto::Message::user(user_text.as_str()));
                        spawn_turn(
                            Arc::clone(&provider),
                            model_id.clone(),
                            history.clone(),
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
        }

        // Drain all pending agent events.
        while let Ok(event) = agent_rx.try_recv() {
            // Accumulate streaming tokens into the conversation history so the
            // next turn has full context.
            if let AgentEvent::Token { ref text } = event {
                match history.last_mut() {
                    Some(m) if matches!(m.role, hone_proto::Role::Assistant) => {
                        // Append to the existing text content in place.
                        match &mut m.content {
                            hone_proto::Content::Text(s) => s.push_str(text),
                            hone_proto::Content::Parts(_) => {
                                // Multi-part: fall back to a new assistant message.
                                history.push(hone_proto::Message::assistant(text.clone()));
                            }
                        }
                    }
                    _ => {
                        history.push(hone_proto::Message::assistant(text.clone()));
                    }
                }
            }
            app.handle_agent_event(event);
        }

        app.tick();
    }

    Ok(())
}
