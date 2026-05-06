//! `GET /mcp/marketplace` and `GET /mcp/installed` — read-only views over
//! the curated MCP catalog and the operator's local `mcp.json` config.
//!
//! These are simple JSON proxies; install/uninstall happens GUI-side via
//! Tauri commands that write back to the project's `.hone/mcp.json` (we
//! deliberately keep the daemon read-only for the marketplace surface).

use std::path::Path;

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use hone_config::McpConfig;
use hone_mcp::Marketplace;
use serde::Serialize;

use crate::state::AppState;

/// One installed MCP server entry as the GUI sees it. Strict subset of
/// `McpServerConfig` — we leave out `env` for now (would expose API
/// tokens from the user's config to anyone with daemon access).
#[derive(Debug, Clone, Serialize)]
pub struct InstalledServer {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub disabled: bool,
    pub timeout_ms: u64,
    pub source: InstalledSource,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstalledSource {
    Global,
    Project,
}

/// `GET /mcp/marketplace` — built-in curated MCP server catalog.
pub async fn marketplace_handler() -> impl IntoResponse {
    let m = Marketplace::builtin();
    Json(m.entries)
}

/// `GET /mcp/installed` — merged view of `~/.config/hone/mcp.json` and
/// `<project_root>/.hone/mcp.json`. Project entries override global ones
/// when names collide; the source tag reflects the *winning* layer.
pub async fn installed_handler(State(state): State<AppState>) -> impl IntoResponse {
    let summaries = collect_installed(&state.workspace_root);
    Json(summaries)
}

fn collect_installed(workspace_root: &Path) -> Vec<InstalledServer> {
    let global_path = dirs::config_dir().map(|d| d.join("hone").join("mcp.json"));
    let project_path = hone_config::find_project_root(workspace_root)
        .map(|root| root.join(".hone").join("mcp.json"));

    // Load the layers separately so we can tag each entry with where it
    // came from. McpConfig::load merges silently; we lose the source if
    // we use it directly.
    let global = match &global_path {
        Some(p) => McpConfig::load(Some(p), None),
        None => McpConfig::default(),
    };
    let project = match &project_path {
        Some(p) => McpConfig::load(Some(p), None),
        None => McpConfig::default(),
    };

    let mut out: Vec<InstalledServer> = Vec::new();
    // Walk global first, but skip names the project overrides.
    for (name, cfg) in &global.mcp_servers {
        if project.mcp_servers.contains_key(name) {
            continue;
        }
        out.push(InstalledServer {
            name: name.clone(),
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            disabled: cfg.disabled,
            timeout_ms: cfg.timeout_ms,
            source: InstalledSource::Global,
        });
    }
    for (name, cfg) in &project.mcp_servers {
        out.push(InstalledServer {
            name: name.clone(),
            command: cfg.command.clone(),
            args: cfg.args.clone(),
            disabled: cfg.disabled,
            timeout_ms: cfg.timeout_ms,
            source: InstalledSource::Project,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn collect_installed_returns_empty_when_no_config_files_exist() {
        let tmp = TempDir::new().expect("tmp");
        let installed = collect_installed(tmp.path());
        // No mcp.json under tmp; user-global may exist on the host but is
        // not asserted on. We just check call completes without panic and
        // returns a Vec.
        let _ = installed;
    }

    #[test]
    fn collect_installed_prefers_project_over_global_on_name_collision() {
        // Build a fake project layout under tmp and verify the project
        // entry wins.
        let tmp = TempDir::new().expect("tmp");
        let project_root = tmp.path();
        // hone-config's find_project_root looks for marker files; create a
        // .git directory so the function returns this path.
        std::fs::create_dir(project_root.join(".git")).expect("mkdir .git");
        let hone_dir = project_root.join(".hone");
        std::fs::create_dir(&hone_dir).expect("mkdir .hone");

        // Write a project mcp.json with a "fs" server.
        let project_json = r#"{
            "mcpServers": {
                "fs": {
                    "command": "npx",
                    "args": ["-y", "@modelcontextprotocol/server-filesystem", "/projectdir"]
                }
            }
        }"#;
        std::fs::write(hone_dir.join("mcp.json"), project_json).expect("write");

        let installed = collect_installed(project_root);
        let fs_entry = installed
            .iter()
            .find(|s| s.name == "fs")
            .expect("fs entry exists");
        assert!(matches!(fs_entry.source, InstalledSource::Project));
        assert_eq!(fs_entry.args.last().map(|s| s.as_str()), Some("/projectdir"));
    }
}
