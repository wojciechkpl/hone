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
