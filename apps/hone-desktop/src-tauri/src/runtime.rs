//! In-flight run registry: spawns a streaming POST to honed's `/run`
//! endpoint, parses the SSE stream, and forwards every parsed `AgentEvent`
//! to the frontend via Tauri's window event bus.
//!
//! Cancellation is best-effort — we abort the local task; the daemon will
//! continue running until it finishes, but its output is dropped on the
//! floor. Until honed exposes a `DELETE /run/:id` endpoint, this is the
//! best the GUI can do.

use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::sse::SseFramer;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRequest {
    pub prompt: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub history: Vec<HistoryEntry>,
    #[serde(default)]
    pub working_dir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub role: String,
    pub content: String,
}

/// What we forward to the frontend as the payload of `hone:event`.
#[derive(Debug, Clone, Serialize)]
pub struct WirePayload {
    pub run_id: String,
    /// The raw `AgentEvent` JSON. We deliberately do NOT type it strongly
    /// here — the frontend already has all the variant knowledge it needs
    /// and re-typing every variant in Rust would couple the GUI to every
    /// future event addition.
    pub event: serde_json::Value,
}

/// The `hone:event` window event channel. Frontend listens on this name.
pub const EVENT_CHANNEL: &str = "hone:event";

#[derive(Default)]
pub struct RunRegistry {
    inner: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl RunRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&self, run_id: String, handle: JoinHandle<()>) {
        self.inner.lock().insert(run_id, handle);
    }

    pub fn cancel(&self, run_id: &str) -> bool {
        if let Some(handle) = self.inner.lock().remove(run_id) {
            handle.abort();
            true
        } else {
            false
        }
    }

    pub fn forget(&self, run_id: &str) {
        self.inner.lock().remove(run_id);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.inner.lock().len()
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.inner.lock().is_empty()
    }
}

/// Spawn a background task that streams `POST {base_url}/run` and emits
/// every parsed event as `hone:event` on the main window.
///
/// Returns the run_id immediately so the frontend can correlate.
pub fn spawn_run(
    app: AppHandle,
    registry: Arc<RunRegistry>,
    base_url: String,
    bearer: Option<String>,
    body: RunRequest,
) -> String {
    let run_id = Uuid::now_v7().to_string();
    let run_id_for_task = run_id.clone();
    let run_id_for_registry = run_id.clone();

    let registry_for_cleanup = Arc::clone(&registry);
    let handle = tokio::spawn(async move {
        let result = stream_run(&app, &run_id_for_task, &base_url, bearer.as_deref(), body).await;
        if let Err(err) = result {
            let _ = app.emit(
                EVENT_CHANNEL,
                WirePayload {
                    run_id: run_id_for_task.clone(),
                    event: serde_json::json!({
                        "type": "error",
                        "message": err,
                    }),
                },
            );
        }
        // Drop ourselves from the registry on natural completion.
        registry_for_cleanup.forget(&run_id_for_task);
    });

    registry.insert(run_id_for_registry, handle);
    run_id
}

async fn stream_run(
    app: &AppHandle,
    run_id: &str,
    base_url: &str,
    bearer: Option<&str>,
    body: RunRequest,
) -> Result<(), String> {
    let url = format!("{}/run", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| e.to_string())?;
    let mut req = client.post(&url).json(&body);
    if let Some(t) = bearer {
        req = req.bearer_auth(t);
    }
    let resp = req.send().await.map_err(|e| e.to_string())?;
    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("run failed: {status} {body}"));
    }

    let mut framer = SseFramer::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| e.to_string())?;
        let text = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
        framer.push(text);
        while let Some(frame) = framer.next_frame() {
            if frame.data.is_empty() {
                continue;
            }
            let event: serde_json::Value = match serde_json::from_str(&frame.data) {
                Ok(v) => v,
                Err(err) => serde_json::json!({
                    "type": "error",
                    "message": format!("bad sse json: {err}"),
                    "raw": frame.data,
                }),
            };
            let _ = app.emit(
                EVENT_CHANNEL,
                WirePayload {
                    run_id: run_id.to_owned(),
                    event,
                },
            );
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn registry_cancel_returns_true_for_known_id_false_otherwise() {
        let reg = RunRegistry::new();
        let handle = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt")
            .spawn(async {});
        reg.insert("abc".into(), handle);
        assert_eq!(reg.len(), 1);
        assert!(reg.cancel("abc"));
        assert_eq!(reg.len(), 0);
        assert!(!reg.cancel("does-not-exist"));
    }

    #[test]
    fn wire_payload_serialises_with_run_id_and_event() {
        let p = WirePayload {
            run_id: "r1".into(),
            event: serde_json::json!({"type": "token", "text": "hi"}),
        };
        let s = serde_json::to_string(&p).expect("ser");
        assert!(s.contains("\"run_id\":\"r1\""));
        assert!(s.contains("\"type\":\"token\""));
    }
}
