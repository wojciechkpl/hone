#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! A7 of `docs/plans/desktop-sidecar-autostart-spec.md`: the page can reach the
//! app. Without `app.withGlobalTauri`, `window.__TAURI__` is undefined and
//! `app.js` silently falls back to a stub `invoke`, so no command ever reaches
//! Rust. Also pins that the capability still covers the app's window, so a label
//! change cannot silently drop the permissions.

use std::path::Path;

fn read_json(rel: &str) -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path:?} is not JSON: {e}"))
}

#[test]
fn test_a7_global_tauri_should_be_enabled_so_the_page_can_invoke_commands() {
    let conf = read_json("tauri.conf.json");
    assert_eq!(
        conf["app"]["withGlobalTauri"],
        serde_json::Value::Bool(true),
        "app.withGlobalTauri must be true: otherwise window.__TAURI__ is undefined \
         and the page runs on a stub invoke"
    );
}

#[test]
fn test_a7_capability_should_grant_core_default_to_the_main_window() {
    let cap = read_json("capabilities/default.json");
    let windows: Vec<&str> = cap["windows"]
        .as_array()
        .expect("capability `windows` is a list")
        .iter()
        .filter_map(|w| w.as_str())
        .collect();
    let permissions: Vec<&str> = cap["permissions"]
        .as_array()
        .expect("capability `permissions` is a list")
        .iter()
        .filter_map(|p| p.as_str())
        .collect();
    assert!(windows.contains(&"main"), "windows: {windows:?}");
    assert!(
        permissions.contains(&"core:default"),
        "permissions: {permissions:?}"
    );
}

#[test]
fn test_a7_app_windows_should_be_labelled_main_or_unlabelled() {
    let conf = read_json("tauri.conf.json");
    let windows = conf["app"]["windows"]
        .as_array()
        .expect("app.windows is a list");
    assert!(!windows.is_empty());
    for w in windows {
        // No label means "main" in Tauri 2.
        let label = w.get("label").and_then(|l| l.as_str()).unwrap_or("main");
        assert_eq!(
            label, "main",
            "a window not labelled `main` would not get the capability's permissions"
        );
    }
}
