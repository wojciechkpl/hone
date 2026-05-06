//! Hone Desktop core — Tauri 2 shell that spawns or connects to `honed`.
//!
//! Modules:
//! - [`supervisor`] manages a child `honed` process (spawn, health, shutdown,
//!   bounded restart). Unit-tested with a stub child so it does not require
//!   the real binary on `PATH`.
//! - [`commands`] exposes Tauri commands the frontend can invoke.
//! - [`tray`] builds the system-tray menu.
//! - [`config`] reads/writes the GUI's persistent preferences.

pub mod commands;
pub mod config;
pub mod supervisor;
pub mod tray;

use crate::commands::DaemonState;

/// Run the Tauri application. Returns when the user quits.
pub fn run() -> anyhow::Result<()> {
    let cfg = config::DesktopConfig::load_or_default();
    let state = DaemonState::new(cfg);

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::daemon_status,
            commands::daemon_start,
            commands::daemon_stop,
            commands::daemon_set_external,
        ])
        .setup(|app| {
            tray::install(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Hide-on-close so the tray icon stays useful.
                if window.label() == "main" {
                    let _ = window.hide();
                    api.prevent_close();
                }
            }
        })
        .run(tauri::generate_context!())
        .map_err(|e| anyhow::anyhow!("tauri runtime error: {e}"))?;

    Ok(())
}

