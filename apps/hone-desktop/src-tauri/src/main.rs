//! Tauri 2 entry point for the Hone desktop shell.
//!
//! All app logic lives in the `hone_desktop_core` library so it can be
//! exercised from integration tests without launching a window.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(err) = hone_desktop_core::run() {
        tracing::error!(?err, "hone-desktop exited with error");
        std::process::exit(1);
    }
}
