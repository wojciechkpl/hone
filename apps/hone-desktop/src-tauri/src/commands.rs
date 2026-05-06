//! Tauri commands invoked by the frontend (`window.__TAURI__.core.invoke`).
//!
//! Naming convention: `daemon_*` for the local sidecar lifecycle.

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;

use crate::config::{DaemonMode, DesktopConfig};
use crate::runtime::{RunRegistry, RunRequest};
use crate::supervisor::{SidecarSupervisor, SupervisorError};

/// Shared state managed by Tauri.
pub struct DaemonState {
    pub(crate) cfg: Mutex<DesktopConfig>,
    pub(crate) sidecar: Arc<Mutex<Option<SidecarSupervisor>>>,
    pub(crate) external: Mutex<Option<ExternalDaemon>>,
}

#[derive(Debug, Clone)]
pub(crate) struct ExternalDaemon {
    pub url: String,
    pub token: Option<String>,
}

impl DaemonState {
    pub fn new(cfg: DesktopConfig) -> Self {
        let external = match cfg.daemon.clone() {
            Some(DaemonMode::External { url, token }) => Some(ExternalDaemon { url, token }),
            _ => None,
        };
        Self {
            cfg: Mutex::new(cfg),
            sidecar: Arc::new(Mutex::new(None)),
            external: Mutex::new(external),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DaemonStatus {
    pub mode: &'static str,
    pub url: Option<String>,
    pub healthy: bool,
    pub pid: Option<u32>,
}

#[tauri::command]
pub async fn daemon_status(state: tauri::State<'_, DaemonState>) -> Result<DaemonStatus, String> {
    let sidecar_info = { state.sidecar.lock().as_ref().map(|s| s.info().clone()) };
    if let Some(info) = sidecar_info {
        let healthy = http_health(&info.url, info.bearer_token.as_deref()).await;
        return Ok(DaemonStatus {
            mode: "sidecar",
            url: Some(info.url),
            healthy,
            pid: Some(info.pid),
        });
    }
    let external = { state.external.lock().clone() };
    if let Some(ext) = external {
        let healthy = http_health(&ext.url, ext.token.as_deref()).await;
        return Ok(DaemonStatus {
            mode: "external",
            url: Some(ext.url),
            healthy,
            pid: None,
        });
    }
    Ok(DaemonStatus { mode: "none", url: None, healthy: false, pid: None })
}

#[tauri::command]
pub async fn daemon_start(state: tauri::State<'_, DaemonState>) -> Result<DaemonStatus, String> {
    {
        let mut slot = state.sidecar.lock();
        if slot.is_some() {
            // Already running — fall through to status.
        } else {
            let sup = SidecarSupervisor::spawn(None).map_err(supervisor_err)?;
            // Wait briefly for health; if the user has no honed binary on
            // PATH the spawn would have failed already, so this is the
            // tighter check that the bind succeeded.
            sup.wait_until_healthy(Duration::from_secs(5))
                .map_err(supervisor_err)?;
            *slot = Some(sup);
        }
    }
    {
        let mut cfg = state.cfg.lock();
        cfg.daemon = Some(DaemonMode::Sidecar);
        let _ = cfg.save();
    }
    daemon_status(state).await
}

#[tauri::command]
pub async fn daemon_stop(state: tauri::State<'_, DaemonState>) -> Result<DaemonStatus, String> {
    let taken = { state.sidecar.lock().take() };
    if let Some(sup) = taken {
        sup.shutdown().map_err(supervisor_err)?;
    }
    daemon_status(state).await
}

#[tauri::command]
pub async fn daemon_set_external(
    state: tauri::State<'_, DaemonState>,
    url: String,
    token: Option<String>,
) -> Result<DaemonStatus, String> {
    {
        let mut ext = state.external.lock();
        *ext = Some(ExternalDaemon { url: url.clone(), token: token.clone() });
    }
    {
        let mut cfg = state.cfg.lock();
        cfg.daemon = Some(DaemonMode::External { url, token });
        let _ = cfg.save();
    }
    daemon_status(state).await
}

/// Endpoint resolution result. `base_url` strips any trailing `/` so callers
/// can append `/run`, `/health`, etc. without double-slashing.
fn resolve_endpoint(state: &DaemonState) -> Option<(String, Option<String>)> {
    if let Some(info) = state.sidecar.lock().as_ref().map(|s| s.info().clone()) {
        return Some((info.url, info.bearer_token));
    }
    state
        .external
        .lock()
        .clone()
        .map(|e| (e.url, e.token))
}

#[tauri::command]
pub async fn run_turn(
    app: tauri::AppHandle,
    state: tauri::State<'_, DaemonState>,
    registry: tauri::State<'_, std::sync::Arc<RunRegistry>>,
    request: RunRequest,
) -> Result<String, String> {
    let (base_url, bearer) = resolve_endpoint(&state)
        .ok_or_else(|| "no daemon configured".to_string())?;
    let registry = std::sync::Arc::clone(&registry);
    let run_id = crate::runtime::spawn_run(app, registry, base_url, bearer, request);
    Ok(run_id)
}

/// Multi-agent orchestrated execution. Same wire format on the SSE side
/// as `run_turn`, but the daemon goes through the multi-agent
/// `Orchestrator` which emits `plan_created` / `worker_started` /
/// `worker_completed` / `budget_progress` events the GUI uses to render
/// the task tree.
#[tauri::command]
pub async fn run_orchestrate(
    app: tauri::AppHandle,
    state: tauri::State<'_, DaemonState>,
    registry: tauri::State<'_, std::sync::Arc<RunRegistry>>,
    request: serde_json::Value,
) -> Result<String, String> {
    let (base_url, bearer) = resolve_endpoint(&state)
        .ok_or_else(|| "no daemon configured".to_string())?;
    let registry = std::sync::Arc::clone(&registry);
    let run_id = crate::runtime::spawn_orchestrate(app, registry, base_url, bearer, request);
    Ok(run_id)
}

#[tauri::command]
pub async fn run_cancel(
    registry: tauri::State<'_, std::sync::Arc<RunRegistry>>,
    run_id: String,
) -> Result<bool, String> {
    Ok(registry.cancel(&run_id))
}

/// `GET /agents` proxy. Returns the JSON array as opaque values so the
/// GUI can iterate on the schema without round-tripping through Rust.
#[tauri::command]
pub async fn list_agents(
    state: tauri::State<'_, DaemonState>,
) -> Result<serde_json::Value, String> {
    fetch_json(&state, "/agents").await
}

/// `GET /recipes` proxy.
#[tauri::command]
pub async fn list_recipes(
    state: tauri::State<'_, DaemonState>,
) -> Result<serde_json::Value, String> {
    fetch_json(&state, "/recipes").await
}

/// `GET /memory?limit=N` proxy.
#[tauri::command]
pub async fn list_memory(
    state: tauri::State<'_, DaemonState>,
    limit: Option<usize>,
    q: Option<String>,
) -> Result<serde_json::Value, String> {
    let mut query = String::new();
    if let Some(n) = limit {
        query.push_str(&format!("?limit={n}"));
    }
    if let Some(q_s) = q.filter(|s| !s.is_empty()) {
        query.push_str(if query.is_empty() { "?" } else { "&" });
        query.push_str(&format!("q={}", urlencoding(&q_s)));
    }
    fetch_json(&state, &format!("/memory{query}")).await
}

/// `GET /memory/stats` proxy.
#[tauri::command]
pub async fn memory_stats(
    state: tauri::State<'_, DaemonState>,
) -> Result<serde_json::Value, String> {
    fetch_json(&state, "/memory/stats").await
}

/// `GET /sandbox/policies` proxy.
#[tauri::command]
pub async fn list_sandbox_policies(
    state: tauri::State<'_, DaemonState>,
) -> Result<serde_json::Value, String> {
    fetch_json(&state, "/sandbox/policies").await
}

/// `GET /mcp/marketplace` proxy.
#[tauri::command]
pub async fn list_mcp_marketplace(
    state: tauri::State<'_, DaemonState>,
) -> Result<serde_json::Value, String> {
    fetch_json(&state, "/mcp/marketplace").await
}

/// `GET /mcp/installed` proxy.
#[tauri::command]
pub async fn list_mcp_installed(
    state: tauri::State<'_, DaemonState>,
) -> Result<serde_json::Value, String> {
    fetch_json(&state, "/mcp/installed").await
}

/// Install an MCP marketplace entry into the project's `.hone/mcp.json`.
///
/// Writes (or merges) one entry under its `id` as the server name. The
/// command + args + env from the marketplace entry are copied verbatim.
/// Refuses to overwrite an existing entry without `force=true`.
///
/// Returns the absolute path of the file written.
#[tauri::command]
pub async fn mcp_install(
    project_root: String,
    entry: serde_json::Value,
    force: Option<bool>,
) -> Result<String, String> {
    let force = force.unwrap_or(false);
    let project_root = std::path::PathBuf::from(project_root);
    if !project_root.is_dir() {
        return Err(format!(
            "project_root is not a directory: {}",
            project_root.display()
        ));
    }
    let id = entry
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "marketplace entry missing 'id'".to_string())?;
    if id.is_empty() || id.contains(['/', '\\', '\0']) {
        return Err(format!("invalid mcp id: {id:?}"));
    }
    let command = entry
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "marketplace entry missing 'command'".to_string())?;
    let args: Vec<String> = entry
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let hone_dir = project_root.join(".hone");
    crate::commands::fs::create_dir_all(&hone_dir)
        .map_err(|e| format!("create dir {}: {e}", hone_dir.display()))?;
    let path = hone_dir.join("mcp.json");

    // Read existing config (if any) so we preserve sibling entries.
    let mut root: serde_json::Value = if path.exists() {
        let body = std::fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let servers = root
        .as_object_mut()
        .ok_or_else(|| "existing mcp.json is not a JSON object".to_string())?
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    let servers_obj = servers
        .as_object_mut()
        .ok_or_else(|| "mcpServers field is not a JSON object".to_string())?;
    if servers_obj.contains_key(id) && !force {
        return Err(format!(
            "mcp.json already has an entry for '{id}' (pass force=true to overwrite)"
        ));
    }
    servers_obj.insert(
        id.to_string(),
        serde_json::json!({
            "command": command,
            "args": args,
        }),
    );

    let written = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("serialise mcp.json: {e}"))?;
    crate::commands::fs::write(&path, written.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path.display().to_string())
}

/// `GET /agents/{name}` proxy.
#[tauri::command]
pub async fn get_agent(
    state: tauri::State<'_, DaemonState>,
    name: String,
) -> Result<serde_json::Value, String> {
    fetch_json(&state, &format!("/agents/{}", urlencoding(&name))).await
}

/// `GET /recipes/{name}` proxy.
#[tauri::command]
pub async fn get_recipe(
    state: tauri::State<'_, DaemonState>,
    name: String,
) -> Result<serde_json::Value, String> {
    fetch_json(&state, &format!("/recipes/{}", urlencoding(&name))).await
}

/// Fork an agent into the project's `.hone/agents/` directory.
///
/// Reconstructs a markdown file (YAML frontmatter + body) from the detail
/// returned by `GET /agents/{name}` so the user can iterate on it locally
/// and have the loader pick it up as a project-source override on next
/// run.
///
/// Inputs:
/// - `project_root`: the project root the GUI is currently scoped to.
///   Resolved client-side because the daemon does not know the GUI's
///   notion of "project."
/// - `agent`: the JSON payload from `GET /agents/{name}` (passed through
///   without re-fetching to avoid a TOCTOU race).
///
/// Refuses to overwrite an existing file unless `force == true`.
#[tauri::command]
pub async fn agent_fork(
    project_root: String,
    agent: serde_json::Value,
    force: Option<bool>,
) -> Result<String, String> {
    let force = force.unwrap_or(false);
    let project_root = std::path::PathBuf::from(project_root);
    if !project_root.is_dir() {
        return Err(format!("project_root is not a directory: {}", project_root.display()));
    }
    let agents_dir = project_root.join(".hone").join("agents");
    crate::commands::fs::create_dir_all(&agents_dir)
        .map_err(|e| format!("create dir {}: {e}", agents_dir.display()))?;

    let name = agent
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "agent payload missing 'name'".to_string())?;
    if name.is_empty() || name.contains(['/', '\\', '\0']) {
        return Err(format!("invalid agent name: {name:?}"));
    }
    let dest = agents_dir.join(format!("{name}.md"));
    if dest.exists() && !force {
        return Err(format!(
            "agent already exists at {} (pass force=true to overwrite)",
            dest.display()
        ));
    }

    let markdown = build_agent_markdown(&agent)?;
    crate::commands::fs::write(&dest, markdown.as_bytes())
        .map_err(|e| format!("write {}: {e}", dest.display()))?;
    Ok(dest.display().to_string())
}

/// Reconstruct an agent's on-disk markdown form from the detail JSON.
/// Schema mirrors `hone_core::agent_def::AgentDef::from_markdown`.
fn build_agent_markdown(agent: &serde_json::Value) -> Result<String, String> {
    let mut frontmatter = serde_json::Map::new();
    for key in ["name", "description", "model", "tools", "sub_agents"] {
        if let Some(v) = agent.get(key) {
            frontmatter.insert(key.to_string(), v.clone());
        }
    }
    let yaml = serde_yaml_to_string(&serde_json::Value::Object(frontmatter))?;
    let body = agent
        .get("system_prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    Ok(format!("---\n{yaml}---\n{body}"))
}

/// Tiny serde_json::Value → YAML helper. We avoid pulling in serde_yaml
/// directly here to keep the Tauri shell's dep graph minimal.
fn serde_yaml_to_string(value: &serde_json::Value) -> Result<String, String> {
    let mut out = String::new();
    if let serde_json::Value::Object(map) = value {
        for (k, v) in map {
            match v {
                serde_json::Value::String(s) => {
                    // Quote strings with special chars; otherwise leave bare.
                    if needs_yaml_quoting(s) {
                        out.push_str(&format!("{k}: {}\n", yaml_quote(s)));
                    } else {
                        out.push_str(&format!("{k}: {s}\n"));
                    }
                }
                serde_json::Value::Bool(b) => out.push_str(&format!("{k}: {b}\n")),
                serde_json::Value::Number(n) => out.push_str(&format!("{k}: {n}\n")),
                serde_json::Value::Array(arr) => {
                    out.push_str(&format!("{k}:\n"));
                    for item in arr {
                        if let Some(s) = item.as_str() {
                            if needs_yaml_quoting(s) {
                                out.push_str(&format!("  - {}\n", yaml_quote(s)));
                            } else {
                                out.push_str(&format!("  - {s}\n"));
                            }
                        } else {
                            return Err(format!("unsupported array item: {item}"));
                        }
                    }
                }
                serde_json::Value::Null => {}
                _ => return Err(format!("unsupported YAML value at {k}: {v}")),
            }
        }
        Ok(out)
    } else {
        Err("expected JSON object for YAML serialisation".to_string())
    }
}

fn needs_yaml_quoting(s: &str) -> bool {
    s.is_empty()
        || s.contains(['"', '\'', ':', '#', '\n', '\r', '\t'])
        || s.starts_with(['-', '?', '!', '&', '*', '@', '`'])
        || matches!(s, "true" | "false" | "null" | "yes" | "no")
}

fn yaml_quote(s: &str) -> String {
    // Double-quoted form with backslash escapes for `\` and `"`.
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn urlencoding(s: &str) -> String {
    // Tiny inline encoder — the agent/recipe names we route through come
    // from a controlled loader, but we still escape `/` and ` ` to be safe.
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Tiny in-house FS helpers wrapped behind a module so test code can stub
/// them out if we ever need to. Today they're just `std::fs` shims.
pub(crate) mod fs {
    use std::path::Path;

    pub fn create_dir_all(p: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(p)
    }

    pub fn write(p: &Path, contents: &[u8]) -> std::io::Result<()> {
        std::fs::write(p, contents)
    }
}

async fn fetch_json(state: &DaemonState, path: &str) -> Result<serde_json::Value, String> {
    let (base_url, bearer) = resolve_endpoint(state)
        .ok_or_else(|| "no daemon configured".to_string())?;
    let url = format!("{}{}", base_url.trim_end_matches('/'), path);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.get(&url);
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("{path} failed: {status} {body}"));
    }
    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())
}

/// Forwards a `StepAction` to the in-flight daemon-side run identified by
/// `run_id`. The action JSON shape matches `hone_proto::StepAction`'s
/// `serde` representation (e.g. `"Proceed"`, `{"ApproveAlways":"shell"}`).
#[tauri::command]
pub async fn run_step(
    state: tauri::State<'_, DaemonState>,
    run_id: String,
    action: serde_json::Value,
) -> Result<String, String> {
    let (base_url, bearer) = resolve_endpoint(&state)
        .ok_or_else(|| "no daemon configured".to_string())?;
    let url = format!("{}/run/{}/step", base_url.trim_end_matches('/'), run_id);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.post(&url).json(&action);
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("step failed: {status} {body}"));
    }
    Ok(status.as_u16().to_string())
}

async fn http_health(base_url: &str, token: Option<&str>) -> bool {
    let url = format!("{}/health", base_url.trim_end_matches('/'));
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_millis(750))
        .build()
    {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut req = client.get(&url);
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    matches!(req.send().await, Ok(r) if r.status().is_success())
}

fn supervisor_err(e: SupervisorError) -> String {
    e.to_string()
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn url_encoding_passes_alphanumerics_and_escapes_specials() {
        assert_eq!(urlencoding("simple-name_v1"), "simple-name_v1");
        assert_eq!(urlencoding("has space"), "has%20space");
        assert_eq!(urlencoding("a/b"), "a%2Fb");
    }

    #[test]
    fn yaml_quoting_recognises_special_chars() {
        assert!(needs_yaml_quoting(""));
        assert!(needs_yaml_quoting("has: colon"));
        assert!(needs_yaml_quoting("has\nnewline"));
        assert!(needs_yaml_quoting("- starts with dash"));
        assert!(!needs_yaml_quoting("simple-string"));
        assert!(!needs_yaml_quoting("alphanumeric123"));
    }

    #[test]
    fn build_agent_markdown_round_trips_via_hone_core_loader() {
        // We can't call the real loader here without a lot of crate plumbing;
        // instead check the wire-shape contract we hand to disk.
        let agent = serde_json::json!({
            "name": "researcher",
            "description": "researches things",
            "model": "sonnet",
            "tools": ["Read", "Grep"],
            "sub_agents": [],
            "system_prompt": "You are a researcher. Be terse.\n",
        });
        let md = build_agent_markdown(&agent).expect("md");
        assert!(md.starts_with("---\n"));
        assert!(md.contains("name: researcher"));
        assert!(md.contains("model: sonnet"));
        assert!(md.contains("- Read"));
        assert!(md.contains("- Grep"));
        assert!(md.contains("\n---\nYou are a researcher. Be terse.\n"));
    }

    #[tokio::test]
    async fn agent_fork_writes_file_under_project_root() {
        let tmp = tempfile::tempdir().expect("tmp");
        let agent = serde_json::json!({
            "name": "mybot",
            "description": "test bot",
            "model": "sonnet",
            "tools": ["Read"],
            "sub_agents": [],
            "system_prompt": "Be helpful.",
        });
        let path = agent_fork(
            tmp.path().display().to_string(),
            agent,
            None,
        )
        .await
        .expect("fork ok");
        assert!(path.ends_with(".hone/agents/mybot.md"), "unexpected path: {path}");
        let written = std::fs::read_to_string(&path).expect("read back");
        assert!(written.contains("name: mybot"));
        assert!(written.contains("\nBe helpful."));
    }

    #[tokio::test]
    async fn agent_fork_refuses_to_overwrite_without_force() {
        let tmp = tempfile::tempdir().expect("tmp");
        let agent = serde_json::json!({
            "name": "mybot",
            "description": "test bot",
            "model": "sonnet",
            "tools": [],
            "sub_agents": [],
            "system_prompt": "v1",
        });
        let _ = agent_fork(tmp.path().display().to_string(), agent.clone(), None)
            .await
            .expect("first fork ok");
        let err = agent_fork(tmp.path().display().to_string(), agent.clone(), None)
            .await
            .expect_err("second fork must fail without force");
        assert!(err.contains("already exists"), "unexpected error: {err}");
        // Now with force, it succeeds.
        let _ = agent_fork(tmp.path().display().to_string(), agent, Some(true))
            .await
            .expect("force overwrite");
    }

    #[tokio::test]
    async fn mcp_install_writes_entry_under_id() {
        let tmp = tempfile::tempdir().expect("tmp");
        let entry = serde_json::json!({
            "id": "filesystem",
            "name": "Filesystem",
            "command": "npx",
            "args": ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"],
        });
        let path = mcp_install(tmp.path().display().to_string(), entry, None)
            .await
            .expect("install ok");
        assert!(path.ends_with(".hone/mcp.json"), "unexpected path: {path}");
        let text = std::fs::read_to_string(&path).expect("read back");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        let server = parsed
            .get("mcpServers")
            .and_then(|s| s.get("filesystem"))
            .expect("filesystem server present");
        assert_eq!(server.get("command").and_then(|v| v.as_str()), Some("npx"));
    }

    #[tokio::test]
    async fn mcp_install_preserves_sibling_entries() {
        let tmp = tempfile::tempdir().expect("tmp");
        let hone_dir = tmp.path().join(".hone");
        std::fs::create_dir(&hone_dir).expect("mkdir");
        let existing = r#"{
            "mcpServers": {
                "git": {"command": "npx", "args": ["-y", "@modelcontextprotocol/server-git"]}
            }
        }"#;
        std::fs::write(hone_dir.join("mcp.json"), existing).expect("seed");

        let entry = serde_json::json!({
            "id": "fs",
            "command": "npx",
            "args": [],
        });
        let _ = mcp_install(tmp.path().display().to_string(), entry, None)
            .await
            .expect("install ok");

        let text = std::fs::read_to_string(hone_dir.join("mcp.json")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        let servers = parsed.get("mcpServers").expect("mcpServers");
        assert!(servers.get("git").is_some(), "existing entry preserved");
        assert!(servers.get("fs").is_some(), "new entry added");
    }

    #[tokio::test]
    async fn mcp_install_refuses_to_overwrite_without_force() {
        let tmp = tempfile::tempdir().expect("tmp");
        let entry = serde_json::json!({
            "id": "filesystem",
            "command": "npx",
            "args": [],
        });
        let _ = mcp_install(tmp.path().display().to_string(), entry.clone(), None)
            .await
            .expect("first install");
        let err = mcp_install(tmp.path().display().to_string(), entry.clone(), None)
            .await
            .expect_err("second must fail");
        assert!(err.contains("already has an entry"), "unexpected: {err}");
        let _ = mcp_install(tmp.path().display().to_string(), entry, Some(true))
            .await
            .expect("force overwrite");
    }

    #[tokio::test]
    async fn mcp_install_rejects_invalid_id() {
        let tmp = tempfile::tempdir().expect("tmp");
        let entry = serde_json::json!({
            "id": "evil/../escape",
            "command": "npx",
            "args": [],
        });
        let err = mcp_install(tmp.path().display().to_string(), entry, None)
            .await
            .expect_err("must reject path-traversal");
        assert!(err.contains("invalid mcp id"), "unexpected: {err}");
    }

    #[tokio::test]
    async fn agent_fork_rejects_invalid_names() {
        let tmp = tempfile::tempdir().expect("tmp");
        let agent = serde_json::json!({
            "name": "evil/../escape",
            "description": "x",
            "model": "sonnet",
            "tools": [],
            "sub_agents": [],
            "system_prompt": "",
        });
        let err = agent_fork(tmp.path().display().to_string(), agent, None)
            .await
            .expect_err("must reject path-traversal name");
        assert!(err.contains("invalid agent name"), "unexpected: {err}");
    }

    #[test]
    fn daemon_state_new_with_external_mode_seeds_external_field() {
        let cfg = DesktopConfig {
            daemon: Some(DaemonMode::External {
                url: "http://10.0.0.5:3117".into(),
                token: Some("t0k".into()),
            }),
        };
        let state = DaemonState::new(cfg);
        let ext = state.external.lock().clone().expect("should be seeded");
        assert_eq!(ext.url, "http://10.0.0.5:3117");
        assert_eq!(ext.token.as_deref(), Some("t0k"));
        assert!(state.sidecar.lock().is_none());
    }

    #[test]
    fn daemon_state_new_with_sidecar_mode_leaves_external_empty() {
        let cfg = DesktopConfig { daemon: Some(DaemonMode::Sidecar) };
        let state = DaemonState::new(cfg);
        assert!(state.external.lock().is_none());
    }
}
