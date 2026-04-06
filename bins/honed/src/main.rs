use anyhow::Result;
use hone_server::{AppState, router};

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

    let state = AppState::new(auth_token);
    let app = router(state);

    let addr = format!("0.0.0.0:{port}");
    tracing::info!("honed listening on {addr}");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
