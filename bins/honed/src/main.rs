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
        println!("  HONE_AUTH_TOKEN    Bearer token for auth (sent as: Authorization: Bearer <token>)");
        println!("  HONE_SESSION_DB    Path to SQLite file (default ~/.local/share/hone/sessions.db)");
        println!("  HONE_EPHEMERAL     Disable session persistence entirely");
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

    let auth_token = std::env::var("HONE_AUTH_TOKEN").ok();

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

    let state = AppState::with_sessions(auth_token, sessions).with_tunnel(tunnel_manager);

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

    let addr = format!("0.0.0.0:{port}");
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
