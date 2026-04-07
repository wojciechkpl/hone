use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use hone_server::{AppState, SessionStorage, SqliteSessionStore, router};
use hone_server::session::SessionStore;

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
        println!("  HONE_AUTH_TOKEN    Bearer token required on every request");
        println!("  HONE_SESSION_DB    Path to SQLite file for persistent sessions");
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

    let sessions: Arc<SessionStorage> = if let Ok(db_path) = std::env::var("HONE_SESSION_DB") {
        let store = SqliteSessionStore::open(Path::new(&db_path)).await?;
        Arc::new(SessionStorage::Sqlite(store))
    } else {
        Arc::new(SessionStorage::InMemory(SessionStore::new()))
    };

    let state = AppState::with_sessions(auth_token, sessions);
    let app = router(state);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("honed listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
