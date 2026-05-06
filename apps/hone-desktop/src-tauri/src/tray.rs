//! System tray icon + menu.
//!
//! On the v0.0.1 shell the menu only does what the supervisor and main
//! window already support: show/hide the window, stop the sidecar, quit.
//! Approval-pending notifications are wired in Phase 2 alongside the SSE
//! consumer.

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager};

use crate::commands::DaemonState;

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Show Hone", true, None::<&str>)?;
    let hide = MenuItem::with_id(app, "hide", "Hide window", true, None::<&str>)?;
    let stop_daemon =
        MenuItem::with_id(app, "stop_daemon", "Stop honed sidecar", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Hone", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &hide, &stop_daemon, &quit])?;

    TrayIconBuilder::with_id("hone-tray")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => focus_main(app),
            "hide" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            "stop_daemon" => {
                if let Some(state) = app.try_state::<DaemonState>() {
                    let taken = { state.sidecar.lock().take() };
                    if let Some(sup) = taken {
                        let _ = sup.shutdown();
                    }
                }
            }
            "quit" => {
                // Best-effort sidecar cleanup before exit.
                if let Some(state) = app.try_state::<DaemonState>() {
                    let taken = { state.sidecar.lock().take() };
                    if let Some(sup) = taken {
                        let _ = sup.shutdown();
                    }
                }
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click { .. } = event {
                focus_main(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn focus_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}
