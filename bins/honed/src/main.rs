use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use hone_server::session::SessionStore;
use hone_server::tunnel::TunnelManager;
use hone_server::{
    AppState, BoreTunnelManager, MockTunnelManager, SessionStorage, SqliteSessionStore, router,
};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("honed {}", env!("CARGO_PKG_VERSION"));
        println!();
        println!("Usage: honed [PORT]");
        println!();
        println!("Arguments:");
        println!("  PORT  Port to listen on (default: 3117)");
        println!();
        println!("Options:");
        println!("  -h, --help     Print this help message");
        println!("  -V, --version  Print version");
        println!();
        println!("Environment:");
        println!(
            "  HONE_AUTH_TOKEN    Bearer token for auth (sent as: Authorization: Bearer <token>)"
        );
        println!(
            "                     Required when binding non-loopback (see HONE_BIND, HONE_ALLOW_UNAUTH)"
        );
        println!(
            "  HONE_BIND          Host to bind (default: 0.0.0.0). Use 127.0.0.1 for loopback-only"
        );
        println!(
            "  HONE_ALLOW_UNAUTH  Set to '1' to allow unauthenticated non-loopback bind (NOT recommended)"
        );
        println!(
            "  HONE_SESSION_DB    Path to SQLite file (default ~/.local/share/hone/sessions.db)"
        );
        println!("  HONE_EPHEMERAL     Disable session persistence entirely");
        println!(
            "  HONE_DISTRIBUTED_DB        Enable distributed orchestrator at this SQLite path"
        );
        println!(
            "  HONE_DISTRIBUTED_REAP_SECS Lease-reaper tick interval in seconds (default 15)"
        );
        println!("  HONE_DISTRIBUTED_MAX_ATTEMPTS  Max claim attempts before force-fail (default 3)");
        println!(
            "  HONE_WORKER_COORDINATOR    Coordinator URL (e.g. http://10.0.0.5:3117) — when set,"
        );
        println!(
            "                             this honed runs in worker mode: polls /distributed/claim,"
        );
        println!("                             executes via Agent, posts results back");
        println!(
            "  HONE_WORKER_ID             This worker's id (default: hostname or random uuid v7)"
        );
        println!(
            "  HONE_WORKER_CAPABILITIES   Comma-separated capability tags (e.g. 'rust,python')"
        );
        println!(
            "  HONE_WORKER_LSP_LANGS      Comma-separated LSP languages to enable (rust|python|typescript|go)"
        );
        println!(
            "  HONE_WORKER_MCP_CONFIG     Path to an mcp.json config file (enables MCP tools on the worker)"
        );
        println!(
            "  HONE_WORKER_MEMORY_DB      Path to a SQLite memory file (enables memory read tool)"
        );
        println!();
        println!("Endpoints:");
        println!("  POST   /sessions           Create a new session");
        println!("  GET    /sessions/{{id}}      Get session info");
        println!("  DELETE /sessions/{{id}}      Delete a session");
        println!("  DELETE /sessions/gc         Purge old sessions (?older_than_days=30)");
        println!("  GET    /health             Health check");
        println!("  GET    /openapi.json       OpenAPI 3.1 specification");
        println!();
        println!("Session GC runs automatically every hour (purges sessions older than 30 days).");
        return Ok(());
    }

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("honed {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let port: u16 = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .and_then(|s| s.parse().ok())
        .unwrap_or(3117);

    // Evaluate the auth gate as a pure function so it can be unit-tested.
    let bind_host = std::env::var("HONE_BIND").unwrap_or_else(|_| "0.0.0.0".to_string());
    let (auth_token, warn_unauth) = match evaluate_auth_gate(
        std::env::var("HONE_AUTH_TOKEN").ok().as_deref(),
        &bind_host,
        std::env::var("HONE_ALLOW_UNAUTH").ok().as_deref(),
    ) {
        Ok(outcome) => (outcome.normalized_token, outcome.warn_unauth),
        Err(refusal) => {
            eprintln!("honed: {}", refusal.message);
            std::process::exit(2);
        }
    };
    if warn_unauth {
        tracing::warn!(
            "starting WITHOUT bearer-token authentication (HONE_AUTH_TOKEN unset or empty); \
             all routes are unauthenticated"
        );
    }

    // Session storage:
    // - HONE_EPHEMERAL=1 forces in-memory only (no persistence)
    // - HONE_SESSION_DB=<path> uses an explicit SQLite path
    // - default: ~/.local/share/hone/sessions.db (created if absent)
    let sessions: Arc<SessionStorage> = if std::env::var("HONE_EPHEMERAL").is_ok() {
        tracing::info!("ephemeral mode: sessions stored in memory only");
        Arc::new(SessionStorage::InMemory(SessionStore::new()))
    } else if let Ok(db_path) = std::env::var("HONE_SESSION_DB") {
        let store = SqliteSessionStore::open(Path::new(&db_path)).await?;
        tracing::info!("sessions persisted at {}", db_path);
        Arc::new(SessionStorage::Sqlite(store))
    } else {
        // Default to ~/.local/share/hone/sessions.db
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let dir = Path::new(&home).join(".local/share/hone");
        let _ = std::fs::create_dir_all(&dir);
        let db_path = dir.join("sessions.db");
        match SqliteSessionStore::open(&db_path).await {
            Ok(store) => {
                tracing::info!("sessions persisted at {}", db_path.display());
                Arc::new(SessionStorage::Sqlite(store))
            }
            Err(e) => {
                tracing::warn!(
                    "failed to open default session db {}: {e}; falling back to in-memory",
                    db_path.display()
                );
                Arc::new(SessionStorage::InMemory(SessionStore::new()))
            }
        }
    };

    let tunnel_manager: Arc<dyn TunnelManager> = if BoreTunnelManager::is_available().await {
        tracing::info!("using bore tunnel backend");
        Arc::new(BoreTunnelManager::new())
    } else {
        tracing::info!("using mock tunnel backend (install bore CLI for real tunneling)");
        Arc::new(MockTunnelManager::new())
    };

    // Distributed orchestrator (P2-4 Phase 1): when HONE_DISTRIBUTED_DB is
    // set, open the DistributedTaskStore + spawn the lease reaper. Otherwise
    // distributed mode is off and /distributed/* endpoints return 503.
    let distributed_store: Option<Arc<hone_server::distributed::DistributedTaskStore>> =
        if let Ok(db_path) = std::env::var("HONE_DISTRIBUTED_DB") {
            match hone_server::distributed::DistributedTaskStore::open(Path::new(&db_path)).await {
                Ok(store) => {
                    tracing::info!("distributed orchestrator enabled, store at {}", db_path);
                    Some(Arc::new(store))
                }
                Err(e) => {
                    tracing::warn!(
                        "failed to open distributed store {}: {e}; distributed mode disabled",
                        db_path
                    );
                    None
                }
            }
        } else {
            None
        };

    let mut state = AppState::with_sessions(auth_token, sessions).with_tunnel(tunnel_manager);
    if let Some(ref store) = distributed_store {
        state = state.with_distributed_store(Arc::clone(store));
    }

    // Spawn the lease reaper when distributed mode is on. Default per
    // design doc §5.3: 15s interval, 3 max attempts. Override via
    // HONE_DISTRIBUTED_REAP_SECS / HONE_DISTRIBUTED_MAX_ATTEMPTS.
    let _reaper_handle: Option<tokio::task::JoinHandle<()>> =
        distributed_store.as_ref().map(|store| {
            let interval_secs: u64 = std::env::var("HONE_DISTRIBUTED_REAP_SECS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(15);
            let max_attempts: u32 = std::env::var("HONE_DISTRIBUTED_MAX_ATTEMPTS")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(3);
            tracing::info!(
                "lease reaper spawned (interval={interval_secs}s, max_attempts={max_attempts})"
            );
            hone_server::distributed_reaper::spawn_lease_reaper(
                Arc::clone(store),
                std::time::Duration::from_secs(interval_secs),
                max_attempts,
            )
        });

    // Worker mode (P2-4 Phase 3b): when HONE_WORKER_COORDINATOR is set,
    // spawn a WorkerPool that polls the named coordinator for work, runs
    // each claimed SubTask through a real Agent, and posts results back.
    // The same honed binary now plays one of three roles based on env vars:
    //   default                     pure session/HTTP server
    //   HONE_DISTRIBUTED_DB         coordinator (queue + reaper)
    //   HONE_WORKER_COORDINATOR     worker (poll-claim-execute loop)
    //   both env vars               coordinator + worker on same machine
    let _worker_handle: Option<tokio::task::JoinHandle<()>> =
        if let Ok(coordinator_url) = std::env::var("HONE_WORKER_COORDINATOR") {
            let worker_id = std::env::var("HONE_WORKER_ID").unwrap_or_else(|_| {
                hostname::get()
                    .ok()
                    .and_then(|h| h.into_string().ok())
                    .unwrap_or_else(|| format!("worker-{}", uuid::Uuid::now_v7()))
            });
            let capabilities: Vec<String> = std::env::var("HONE_WORKER_CAPABILITIES")
                .unwrap_or_default()
                .split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .collect();
            let token = std::env::var("HONE_AUTH_TOKEN").unwrap_or_default();
            if token.is_empty() {
                tracing::warn!(
                    "HONE_WORKER_COORDINATOR set but HONE_AUTH_TOKEN empty; coordinator may reject"
                );
            }

            // Provider: Anthropic preferred, OpenAI fallback (mirrors hone eval).
            let provider: Option<Arc<dyn hone_providers::ModelProvider>> =
                if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
                    Some(Arc::new(
                        hone_providers::adapters::anthropic::AnthropicProvider::new(key),
                    ))
                } else if let Ok(key) = std::env::var("OPENAI_API_KEY") {
                    Some(Arc::new(hone_providers::adapters::openai::OpenAiProvider::new(key)))
                } else {
                    None
                };

            match provider {
                None => {
                    tracing::warn!(
                        "worker mode requires ANTHROPIC_API_KEY or OPENAI_API_KEY; \
                         worker pool not spawned"
                    );
                    None
                }
                Some(provider) => {
                    let registry = hone_core::AgentRegistry::load_hierarchical(
                        &std::env::current_dir().unwrap_or_default(),
                    );

                    // Base tools always registered.
                    let mut builder = hone_tools::ToolRegistry::builder()
                        .register(Box::new(hone_tools::ReadFileTool))
                        .register(Box::new(hone_tools::GlobTool))
                        .register(Box::new(hone_tools::GrepTool))
                        .register(Box::new(hone_tools::WriteFileTool))
                        .register(Box::new(hone_tools::EditFileTool))
                        .register(Box::new(hone_tools::MultiEditTool))
                        .register(Box::new(hone_tools::ShellTool))
                        .register(Box::new(hone_tools::GitStatusTool))
                        .register(Box::new(hone_tools::GitDiffTool))
                        .register(Box::new(hone_tools::GitCommitTool));

                    // HONE_WORKER_LSP_LANGS — per-language LSP tools.
                    let lsp_langs: Vec<String> = std::env::var("HONE_WORKER_LSP_LANGS")
                        .unwrap_or_default()
                        .split(',')
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| s.trim().to_lowercase())
                        .collect();
                    for lang_str in &lsp_langs {
                        let lang = match lang_str.as_str() {
                            "rust" => hone_tools::lsp::Language::Rust,
                            "python" => hone_tools::lsp::Language::Python,
                            "typescript" => hone_tools::lsp::Language::TypeScript,
                            "go" => hone_tools::lsp::Language::Go,
                            other => {
                                tracing::warn!(
                                    "HONE_WORKER_LSP_LANGS: unknown language '{other}'; skipping"
                                );
                                continue;
                            }
                        };
                        match hone_tools::lsp::StdioLspBridge::for_language(lang).await {
                            Ok(bridge) => {
                                let bridge: std::sync::Arc<dyn hone_tools::lsp::LspBridge> =
                                    std::sync::Arc::new(bridge);
                                builder = builder
                                    .register(Box::new(hone_tools::lsp::LspDiagnosticsTool::new(
                                        std::sync::Arc::clone(&bridge),
                                    )))
                                    .register(Box::new(hone_tools::lsp::LspHoverTool::new(
                                        std::sync::Arc::clone(&bridge),
                                    )))
                                    .register(Box::new(hone_tools::lsp::LspCodeActionTool::new(
                                        std::sync::Arc::clone(&bridge),
                                    )));
                                tracing::info!("worker LSP enabled for: {lang_str}");
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "HONE_WORKER_LSP_LANGS: failed to spawn LSP server for \
                                     '{lang_str}': {e}; skipping"
                                );
                            }
                        }
                    }

                    // HONE_WORKER_MCP_CONFIG — MCP tools from a config file.
                    if let Ok(mcp_config_path) = std::env::var("HONE_WORKER_MCP_CONFIG") {
                        let path = std::path::Path::new(&mcp_config_path);
                        let mcp_config = hone_config::McpConfig::load(None, Some(path));
                        let mcp_manager = hone_mcp::McpManager::from_config(&mcp_config).await;
                        for mcp_tool in mcp_manager.tools_as_native() {
                            builder = builder.register(mcp_tool);
                        }
                        tracing::info!(
                            "worker MCP enabled with config at: {mcp_config_path}"
                        );
                    }

                    // HONE_WORKER_MEMORY_DB — memory read tool backed by SQLite.
                    // KNOWN LIMITATION: MemoryReadTool lives in bins/hone and is not
                    // yet in a shared crate.  The env var surface is wired so users can
                    // configure it now; full wiring will land when MemoryReadTool is
                    // promoted to hone-tools or a dedicated crate.
                    if let Ok(memory_db_path) = std::env::var("HONE_WORKER_MEMORY_DB") {
                        tracing::warn!(
                            "HONE_WORKER_MEMORY_DB is set ('{}') but memory tool support is \
                             not yet wired in honed (MemoryReadTool is not yet in a shared \
                             crate); ignored for now",
                            memory_db_path
                        );
                    }

                    let base_tools = builder.build();
                    let model = hone_proto::ModelId::new("anthropic:claude-sonnet-4-5");
                    let pricing = hone_proto::PricingEntry {
                        model: model.clone(),
                        input_per_million: 3.0,
                        output_per_million: 15.0,
                        cache_read_per_million: None,
                        cache_creation_per_million: None,
                    };
                    let working_dir = std::env::current_dir().unwrap_or_default();

                    let executor = Arc::new(hone_server::AgentTaskExecutor::new(
                        registry, base_tools, provider, model, pricing, working_dir,
                    ));
                    let client = Arc::new(hone_core::queue_client::QueueClient::new(
                        coordinator_url.clone(),
                        token,
                    ));
                    let pool = hone_server::WorkerPool::new(
                        client,
                        worker_id.clone(),
                        capabilities.clone(),
                        executor,
                    );
                    tracing::info!(
                        "worker pool spawned: id={worker_id}, capabilities={:?}, coordinator={coordinator_url}",
                        capabilities
                    );
                    Some(pool.spawn())
                }
            }
        } else {
            None
        };

    // Background session GC: purge sessions older than 30 days every hour.
    {
        let sessions = Arc::clone(&state.sessions);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
            interval.tick().await; // skip the immediate first tick
            loop {
                interval.tick().await;
                let cutoff = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs()
                    .saturating_sub(30 * 86_400);
                match sessions.gc(cutoff).await {
                    Ok(0) => {}
                    Ok(n) => tracing::info!(removed = n, "session GC: purged expired sessions"),
                    Err(e) => tracing::warn!("session GC failed: {e}"),
                }
            }
        });
    }

    let app = router(state);

    let addr = format!("{bind_host}:{port}");
    tracing::info!("honed listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;

    tracing::info!("honed stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("SIGINT received, shutting down gracefully");
        }
        _ = terminate => {
            tracing::info!("SIGTERM received, shutting down gracefully");
        }
    }
}

/// Successful gate evaluation.
#[derive(Debug)]
struct AuthGateOk {
    /// Normalised token: `None` if absent, empty, or whitespace-only.
    /// Otherwise the trimmed token (whitespace stripped, never empty).
    normalized_token: Option<String>,
    /// True iff the daemon is starting without a token (loopback or
    /// HONE_ALLOW_UNAUTH=1).  Caller should emit a runtime warning.
    warn_unauth: bool,
}

/// Gate refusal — startup must not proceed.
#[derive(Debug)]
struct AuthGateBlock {
    message: String,
}

/// Decide whether `honed` is allowed to start given environment.
///
/// Pure function: takes the raw env-var values and returns a verdict so the
/// gate's policy can be unit-tested without spawning a process or touching
/// the network stack.
///
/// Rules:
/// - HONE_AUTH_TOKEN set to a non-empty, non-whitespace value → allow.
/// - bind host is loopback (anything in 127.0.0.0/8, ::1, or the literal
///   "localhost") → allow even without a token (with a warning).
/// - HONE_ALLOW_UNAUTH is exactly "1" → allow even without a token (with a
///   warning).  Any other value (including "", "0", "true", "yes") does NOT
///   bypass the gate.
/// - Otherwise → block.
fn evaluate_auth_gate(
    auth_token_raw: Option<&str>,
    bind_host: &str,
    allow_unauth_raw: Option<&str>,
) -> Result<AuthGateOk, AuthGateBlock> {
    // TODO(security): two UX edges remain undecided.
    //
    //   1. `HONE_ALLOW_UNAUTH` whitespace-trimming consistency.  We trim
    //      `HONE_AUTH_TOKEN` before comparison but require `HONE_ALLOW_UNAUTH`
    //      to equal "1" exactly.  A user setting `HONE_ALLOW_UNAUTH=" 1"` in
    //      a YAML config will see the gate refuse with a message about the
    //      auth token, never realising the ALLOW_UNAUTH value was the actual
    //      cause.  Either trim it (consistent with the token) or document
    //      strict-equality in --help.  Currently neither.
    //
    //   2. `bind_host == "localhost"` is case-sensitive.  `HONE_BIND=Localhost`
    //      falls through to `IpAddr::from_str` (fails) and is treated as
    //      non-loopback.  Fail-closed and inconsistent with how
    //      `to_socket_addrs()` actually resolves it later.  Either lowercase
    //      before comparison or document the strict-case requirement.

    // Normalise the token: trim whitespace, treat empty as absent.  An empty
    // `expected` token in the auth middleware would compare equal to a
    // request bearing `Authorization: Bearer ` and bypass auth entirely.
    let normalized_token = auth_token_raw
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty());

    let is_loopback = bind_host == "localhost"
        || bind_host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());

    let allow_unauth = allow_unauth_raw == Some("1");

    if normalized_token.is_some() {
        return Ok(AuthGateOk {
            normalized_token,
            warn_unauth: false,
        });
    }
    if is_loopback || allow_unauth {
        return Ok(AuthGateOk {
            normalized_token: None,
            warn_unauth: true,
        });
    }
    Err(AuthGateBlock {
        message: format!(
            "refusing to start: no HONE_AUTH_TOKEN set (or it is empty) and binding to non-loopback host {bind_host}.\n\
             Either set HONE_AUTH_TOKEN=<secret>, bind to loopback via HONE_BIND=127.0.0.1,\n\
             or explicitly opt in with HONE_ALLOW_UNAUTH=1 (NOT recommended)."
        ),
    })
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod auth_gate_tests {
    use super::*;

    // -- Block cases (expect Err) --

    #[test]
    fn no_token_default_bind_blocks() {
        let r = evaluate_auth_gate(None, "0.0.0.0", None);
        assert!(r.is_err());
    }

    #[test]
    fn empty_token_default_bind_blocks() {
        let r = evaluate_auth_gate(Some(""), "0.0.0.0", None);
        assert!(r.is_err(), "empty token must NOT count as configured");
    }

    #[test]
    fn whitespace_token_default_bind_blocks() {
        let r = evaluate_auth_gate(Some("   \t\n"), "0.0.0.0", None);
        assert!(r.is_err(), "whitespace-only token must NOT count as configured");
    }

    #[test]
    fn allow_unauth_zero_does_not_bypass() {
        let r = evaluate_auth_gate(None, "0.0.0.0", Some("0"));
        assert!(r.is_err(), "HONE_ALLOW_UNAUTH=0 must NOT bypass the gate");
    }

    #[test]
    fn allow_unauth_empty_does_not_bypass() {
        let r = evaluate_auth_gate(None, "0.0.0.0", Some(""));
        assert!(r.is_err());
    }

    #[test]
    fn allow_unauth_true_does_not_bypass() {
        // Documented semantic: only "1" exactly disables the gate.
        let r = evaluate_auth_gate(None, "0.0.0.0", Some("true"));
        assert!(r.is_err());
    }

    #[test]
    fn ipv6_wildcard_is_not_loopback() {
        let r = evaluate_auth_gate(None, "::", None);
        assert!(r.is_err());
    }

    #[test]
    fn ipv4_mapped_ipv6_loopback_is_treated_as_non_loopback() {
        // Rust stdlib's Ipv6Addr::is_loopback only recognises ::1 exactly,
        // not ::ffff:127.0.0.1.  Document this with a test so the behaviour
        // is intentional, not accidental.
        let r = evaluate_auth_gate(None, "::ffff:127.0.0.1", None);
        assert!(r.is_err());
    }

    // -- Allow cases (expect Ok) --

    #[test]
    fn loopback_127_0_0_1_allows_no_token() {
        let r = evaluate_auth_gate(None, "127.0.0.1", None).expect("should allow");
        assert!(r.normalized_token.is_none());
        assert!(r.warn_unauth);
    }

    #[test]
    fn loopback_127_0_0_2_allows_no_token() {
        // The whole 127.0.0.0/8 block routes to loopback.  Verifies the
        // IpAddr::is_loopback() rewrite (vs the original string allowlist).
        let r = evaluate_auth_gate(None, "127.0.0.2", None).expect("should allow");
        assert!(r.warn_unauth);
    }

    #[test]
    fn loopback_ipv6_allows_no_token() {
        let r = evaluate_auth_gate(None, "::1", None).expect("should allow");
        assert!(r.warn_unauth);
    }

    #[test]
    fn loopback_literal_allows_no_token() {
        let r = evaluate_auth_gate(None, "localhost", None).expect("should allow");
        assert!(r.warn_unauth);
    }

    #[test]
    fn allow_unauth_one_bypasses_gate() {
        let r = evaluate_auth_gate(None, "0.0.0.0", Some("1")).expect("should allow");
        assert!(r.normalized_token.is_none());
        assert!(r.warn_unauth);
    }

    #[test]
    fn valid_token_allows_non_loopback_bind() {
        let r = evaluate_auth_gate(Some("secret123"), "0.0.0.0", None).expect("should allow");
        assert_eq!(r.normalized_token.as_deref(), Some("secret123"));
        assert!(!r.warn_unauth, "warn flag must be off when a real token is set");
    }

    #[test]
    fn token_is_trimmed_before_storing() {
        let r = evaluate_auth_gate(Some("  secret  "), "0.0.0.0", None).expect("should allow");
        assert_eq!(r.normalized_token.as_deref(), Some("secret"));
    }

    // -- Error-message and combined-case assertions (regression guards) --

    #[test]
    fn block_message_includes_bind_host_and_three_remediation_paths() {
        // The block message is a user-facing artifact: it must echo the
        // offending bind host AND name all three escape hatches so the
        // operator can self-serve a fix without reading the source.  A
        // refactor that drops `{bind_host}` from the format string or omits
        // any of the three env-var names should be caught by this test.
        let err = evaluate_auth_gate(None, "10.0.5.42", None)
            .expect_err("non-loopback + no-token must block");
        assert!(
            err.message.contains("10.0.5.42"),
            "message must echo the offending bind host, got: {}",
            err.message
        );
        assert!(
            err.message.contains("HONE_AUTH_TOKEN"),
            "message must mention HONE_AUTH_TOKEN remediation, got: {}",
            err.message
        );
        assert!(
            err.message.contains("HONE_BIND"),
            "message must mention HONE_BIND remediation, got: {}",
            err.message
        );
        assert!(
            err.message.contains("HONE_ALLOW_UNAUTH"),
            "message must mention HONE_ALLOW_UNAUTH remediation, got: {}",
            err.message
        );
    }

    #[test]
    fn token_plus_loopback_does_not_warn() {
        // When a real token is configured AND the bind is loopback, the
        // warning flag must stay off — the loopback exemption only applies
        // when the token is absent.  A future refactor that moves the
        // loopback check above the token short-circuit would silently flip
        // warn_unauth to true on a properly-authenticated loopback bind;
        // this test guards that path.
        let r = evaluate_auth_gate(Some("secret"), "127.0.0.1", None).expect("should allow");
        assert_eq!(r.normalized_token.as_deref(), Some("secret"));
        assert!(
            !r.warn_unauth,
            "warn flag must be off when a real token is set, even on loopback"
        );
    }
}
