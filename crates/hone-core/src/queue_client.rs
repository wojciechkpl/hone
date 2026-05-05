//! HTTP client for the /distributed/* endpoints on a remote `honed` coordinator.
//!
//! This module ships the **library** half of Phase 2 distributed orchestration.
//! The `--distributed` CLI flag that wires this into the Orchestrator is Phase 2b.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::task_plan::SubTask;

// ── Public types ──────────────────────────────────────────────────────────────

/// JSON shape inside each SSE `data:` line emitted by
/// `GET /distributed/plans/:id/events`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlanEvent {
    pub task_id: String,
    /// One of "completed" | "failed" | "timed_out".
    pub status: String,
}

/// Parse SSE frames from a raw byte buffer and return all `PlanEvent`s that
/// can be decoded from `data:` lines.  Lines beginning with `event:` or `:`
/// (keep-alive comments) are accepted and ignored.
///
/// This is intentionally a pure function with no async / HTTP so that it can
/// be unit-tested directly.
pub fn parse_sse_data_lines(raw: &[u8]) -> Vec<PlanEvent> {
    let text = std::str::from_utf8(raw).unwrap_or("");
    let mut events = vec![];
    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data:") {
            let data = data.trim();
            if let Ok(ev) = serde_json::from_str::<PlanEvent>(data) {
                events.push(ev);
            }
        }
    }
    events
}

/// Snapshot of state counts for a plan.
/// Mirrors `PlanStatusResponse` in hone-server without pulling in that crate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanStatusSnapshot {
    pub queued: usize,
    pub leased: usize,
    pub done: usize,
    pub failed: usize,
    pub timed_out: usize,
}

/// A task received from the coordinator after a successful claim.
/// Mirrors `ClaimedTask` in hone-server's `distributed` module.
#[derive(Debug, Clone, Deserialize)]
pub struct ClaimedTaskWire {
    pub task_id: String,
    pub plan_id: String,
    pub subtask: SubTask,
    pub lease_expires_at: chrono::DateTime<chrono::Utc>,
    pub attempts: u32,
}

/// JSON body sent to `POST /distributed/workers/{id}/results/{task_id}`.
/// Matches `ResultsRequest` in hone-server.
#[derive(Debug, Clone, Serialize)]
pub struct CompletionRequestBody {
    pub status_str: String,
    pub response: String,
    pub error_msg: Option<String>,
    pub usage: hone_proto::TokenUsage,
    pub cost_usd: f64,
}

/// Errors returned by every `QueueClient` method.
#[derive(Debug, thiserror::Error)]
pub enum QueueClientError {
    /// Low-level transport / connection error.
    #[error("http: {0}")]
    Http(String),

    /// The server returned an unexpected non-2xx status.
    #[error("server returned {status}: {body}")]
    ServerError { status: u16, body: String },

    /// The coordinator returned 409 — lease is no longer owned by this worker.
    #[error("lease lost (server returned 409)")]
    LeaseLost,

    /// The coordinator returned 503 — distributed mode is not configured.
    #[error("distributed mode not configured on coordinator (503)")]
    NotConfigured,

    /// The coordinator returned 401 — bearer token rejected.
    #[error("auth rejected (401)")]
    Unauthorized,

    /// JSON (de)serialisation error.
    #[error("json: {0}")]
    Json(String),
}

// ── QueueClient ───────────────────────────────────────────────────────────────

/// Mirror of TaskResultRow from hone-server (intentional duplication, same
/// reasoning as ClaimedTaskWire).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerResultWire {
    pub task_id: String,
    pub plan_id: String,
    pub agent: String,
    pub status: String,
    pub response: String,
    pub error_msg: Option<String>,
    pub tokens_input: u32,
    pub tokens_output: u32,
    pub cost_usd: f64,
}

/// Reqwest-based HTTP client for the coordinator's `/distributed/*` endpoints.
pub struct QueueClient {
    coordinator_url: String,
    auth_token: String,
    http: reqwest::Client,
    request_timeout: Duration,
}

impl QueueClient {
    /// Build a client with the default 30-second per-request timeout.
    pub fn new(coordinator_url: impl Into<String>, auth_token: impl Into<String>) -> Self {
        let timeout = Duration::from_secs(30);
        Self {
            coordinator_url: coordinator_url.into(),
            auth_token: auth_token.into(),
            http: reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .expect("reqwest client builder cannot fail with valid timeout"),
            request_timeout: timeout,
        }
    }

    /// Override the per-request timeout.
    pub fn with_timeout(self, timeout: Duration) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(timeout)
                .build()
                .expect("reqwest client builder cannot fail with valid timeout"),
            request_timeout: timeout,
            ..self
        }
    }

    // ── Coordinator-side calls ────────────────────────────────────────────────

    /// POST /distributed/plans/{plan_id}/enqueue
    /// Enqueues subtasks under a plan_id and returns the assigned task_ids.
    pub async fn enqueue(
        &self,
        plan_id: &str,
        subtasks: Vec<SubTask>,
    ) -> Result<Vec<String>, QueueClientError> {
        let url = format!("{}/distributed/plans/{}/enqueue", self.coordinator_url, plan_id);
        let body = json!({ "subtasks": subtasks });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.auth_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| QueueClientError::Http(e.to_string()))?;

        match resp.status().as_u16() {
            200 => {
                let parsed: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| QueueClientError::Json(e.to_string()))?;
                let ids: Vec<String> = serde_json::from_value(parsed["task_ids"].clone())
                    .map_err(|e| QueueClientError::Json(e.to_string()))?;
                Ok(ids)
            }
            401 => Err(QueueClientError::Unauthorized),
            503 => Err(QueueClientError::NotConfigured),
            409 => Err(QueueClientError::LeaseLost),
            s => {
                let body = resp.text().await.unwrap_or_default();
                Err(QueueClientError::ServerError { status: s, body })
            }
        }
    }

    /// GET /distributed/plans/{plan_id}/status
    /// Returns a snapshot of state counts for a plan.
    pub async fn plan_status(
        &self,
        plan_id: &str,
    ) -> Result<PlanStatusSnapshot, QueueClientError> {
        let url = format!("{}/distributed/plans/{}/status", self.coordinator_url, plan_id);

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.auth_token)
            .send()
            .await
            .map_err(|e| QueueClientError::Http(e.to_string()))?;

        match resp.status().as_u16() {
            200 => {
                let snap: PlanStatusSnapshot = resp
                    .json()
                    .await
                    .map_err(|e| QueueClientError::Json(e.to_string()))?;
                Ok(snap)
            }
            401 => Err(QueueClientError::Unauthorized),
            503 => Err(QueueClientError::NotConfigured),
            409 => Err(QueueClientError::LeaseLost),
            s => {
                let body = resp.text().await.unwrap_or_default();
                Err(QueueClientError::ServerError { status: s, body })
            }
        }
    }

    /// GET /distributed/plans/{plan_id}/results
    /// Pull all completed results for a plan.
    pub async fn pull_results(
        &self,
        plan_id: &str,
    ) -> Result<Vec<WorkerResultWire>, QueueClientError> {
        let url = format!("{}/distributed/plans/{}/results", self.coordinator_url, plan_id);

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.auth_token)
            .send()
            .await
            .map_err(|e| QueueClientError::Http(e.to_string()))?;

        match resp.status().as_u16() {
            200 => {
                let rows: Vec<WorkerResultWire> = resp
                    .json()
                    .await
                    .map_err(|e| QueueClientError::Json(e.to_string()))?;
                Ok(rows)
            }
            401 => Err(QueueClientError::Unauthorized),
            503 => Err(QueueClientError::NotConfigured),
            s => {
                let body = resp.text().await.unwrap_or_default();
                Err(QueueClientError::ServerError { status: s, body })
            }
        }
    }

    // ── Worker-side calls ─────────────────────────────────────────────────────

    /// POST /distributed/workers/{worker_id}/heartbeat
    /// Refreshes (or creates) the worker row on the coordinator. Idempotent.
    pub async fn heartbeat(
        &self,
        worker_id: &str,
        capabilities: &[String],
        sandbox_profile: Option<&str>,
    ) -> Result<(), QueueClientError> {
        let url = format!(
            "{}/distributed/workers/{}/heartbeat",
            self.coordinator_url, worker_id
        );
        let body = json!({
            "capabilities": capabilities,
            "sandbox_profile": sandbox_profile,
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.auth_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| QueueClientError::Http(e.to_string()))?;

        match resp.status().as_u16() {
            204 => Ok(()),
            401 => Err(QueueClientError::Unauthorized),
            503 => Err(QueueClientError::NotConfigured),
            409 => Err(QueueClientError::LeaseLost),
            s => {
                let body = resp.text().await.unwrap_or_default();
                Err(QueueClientError::ServerError { status: s, body })
            }
        }
    }

    /// POST /distributed/workers/{worker_id}/claim
    /// Tries to claim one eligible task. Returns `None` when no task is available (204).
    pub async fn claim(
        &self,
        worker_id: &str,
        capabilities: &[String],
        lease_secs: Option<u64>,
    ) -> Result<Option<ClaimedTaskWire>, QueueClientError> {
        let url = format!(
            "{}/distributed/workers/{}/claim",
            self.coordinator_url, worker_id
        );
        let body = json!({
            "capabilities": capabilities,
            "lease_secs": lease_secs,
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.auth_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| QueueClientError::Http(e.to_string()))?;

        match resp.status().as_u16() {
            200 => {
                let task: ClaimedTaskWire = resp
                    .json()
                    .await
                    .map_err(|e| QueueClientError::Json(e.to_string()))?;
                Ok(Some(task))
            }
            204 => Ok(None),
            401 => Err(QueueClientError::Unauthorized),
            503 => Err(QueueClientError::NotConfigured),
            409 => Err(QueueClientError::LeaseLost),
            s => {
                let body = resp.text().await.unwrap_or_default();
                Err(QueueClientError::ServerError { status: s, body })
            }
        }
    }

    /// POST /distributed/workers/{worker_id}/results/{task_id}
    /// Reports completion of a claimed task.
    pub async fn complete(
        &self,
        worker_id: &str,
        task_id: &str,
        body: CompletionRequestBody,
    ) -> Result<(), QueueClientError> {
        let url = format!(
            "{}/distributed/workers/{}/results/{}",
            self.coordinator_url, worker_id, task_id
        );

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.auth_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| QueueClientError::Http(e.to_string()))?;

        match resp.status().as_u16() {
            204 => Ok(()),
            401 => Err(QueueClientError::Unauthorized),
            503 => Err(QueueClientError::NotConfigured),
            409 => Err(QueueClientError::LeaseLost),
            s => {
                let body = resp.text().await.unwrap_or_default();
                Err(QueueClientError::ServerError { status: s, body })
            }
        }
    }

    /// POST /distributed/plans/{plan_id}/cancel
    /// Cancels all queued + leased tasks and returns the count transitioned.
    ///
    /// # Errors
    /// Returns `Err(NotConfigured)` on 503, `Err(Unauthorized)` on 401.
    pub async fn cancel_plan(
        &self,
        plan_id: &str,
    ) -> Result<usize, QueueClientError> {
        let url = format!("{}/distributed/plans/{}/cancel", self.coordinator_url, plan_id);
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.auth_token)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|e| QueueClientError::Http(e.to_string()))?;

        match resp.status().as_u16() {
            200 => {
                let body: serde_json::Value = resp
                    .json()
                    .await
                    .map_err(|e| QueueClientError::Json(e.to_string()))?;
                Ok(body["cancelled"].as_u64().unwrap_or(0) as usize)
            }
            401 => Err(QueueClientError::Unauthorized),
            503 => Err(QueueClientError::NotConfigured),
            s => {
                let body = resp.text().await.unwrap_or_default();
                Err(QueueClientError::ServerError { status: s, body })
            }
        }
    }

    /// GET /distributed/plans/{plan_id}/events
    /// Opens the SSE event stream and returns a stream of `PlanEvent` items.
    /// The stream ends when the server closes the connection (plan terminal).
    ///
    /// # Errors
    /// Returns `Err(NotConfigured)` on 503, `Err(Unauthorized)` on 401.
    pub async fn stream_events(
        &self,
        plan_id: &str,
    ) -> Result<impl futures::Stream<Item = PlanEvent>, QueueClientError> {
        use bytes::BytesMut;
        use futures::StreamExt;

        let url = format!("{}/distributed/plans/{}/events", self.coordinator_url, plan_id);
        let resp = self
            .http
            .get(&url)
            .bearer_auth(&self.auth_token)
            .send()
            .await
            .map_err(|e| QueueClientError::Http(e.to_string()))?;

        let status = resp.status().as_u16();
        if status != 200 {
            let body = resp.text().await.unwrap_or_default();
            return Err(QueueClientError::ServerError { status, body });
        }

        let bytes_stream = resp.bytes_stream();

        let stream = async_stream::stream! {
            let mut buf = BytesMut::new();
            let mut bs = bytes_stream;
            while let Some(chunk_res) = bs.next().await {
                let Ok(chunk) = chunk_res else { break; };
                buf.extend_from_slice(&chunk);

                // Look for complete events (terminated by \n\n).
                while let Some(pos) = buf.windows(2).position(|w| w == b"\n\n") {
                    let frame = buf.split_to(pos + 2);
                    for evt in parse_sse_data_lines(&frame) {
                        yield evt;
                    }
                }
            }
        };

        Ok(stream)
    }

    /// POST /distributed/workers/{worker_id}/extend-lease/{task_id}
    /// Extends the lease. Returns `Err(LeaseLost)` when the coordinator returns 409.
    pub async fn extend_lease(
        &self,
        worker_id: &str,
        task_id: &str,
        extend_secs: u64,
    ) -> Result<(), QueueClientError> {
        let url = format!(
            "{}/distributed/workers/{}/extend-lease/{}",
            self.coordinator_url, worker_id, task_id
        );
        let body = json!({ "extend_secs": extend_secs });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.auth_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| QueueClientError::Http(e.to_string()))?;

        match resp.status().as_u16() {
            204 => Ok(()),
            401 => Err(QueueClientError::Unauthorized),
            503 => Err(QueueClientError::NotConfigured),
            409 => Err(QueueClientError::LeaseLost),
            s => {
                let body = resp.text().await.unwrap_or_default();
                Err(QueueClientError::ServerError { status: s, body })
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::prelude::*;

    // ── helpers ───────────────────────────────────────────────────────────────

    async fn mock_coordinator() -> httpmock::MockServer {
        httpmock::MockServer::start_async().await
    }

    fn client_for(server: &httpmock::MockServer) -> QueueClient {
        QueueClient::new(server.base_url(), "test-token")
    }

    fn make_subtask(id: &str) -> SubTask {
        SubTask {
            id: id.to_owned(),
            description: format!("task {id}"),
            agent: "test-agent".to_owned(),
            context: String::new(),
            depends_on: vec![],
            acceptance_tests: vec![],
            budget_usd: None,
            scope: vec![],
            requires_capabilities: vec![],
        }
    }

    fn make_completion_body() -> CompletionRequestBody {
        CompletionRequestBody {
            status_str: "completed".to_owned(),
            response: "great output".to_owned(),
            error_msg: None,
            usage: hone_proto::TokenUsage {
                input: 10,
                output: 20,
                cache_read: 0,
                cache_creation: 0,
            },
            cost_usd: 0.001,
        }
    }

    // ── Happy paths ───────────────────────────────────────────────────────────

    /// POST /distributed/plans/p1/enqueue returns task_ids; client returns them.
    #[tokio::test]
    async fn enqueue_posts_subtasks_and_returns_task_ids() {
        let server = mock_coordinator().await;
        let mock = server.mock(|when, then| {
            when.method(POST).path("/distributed/plans/p1/enqueue");
            then.status(200)
                .header("Content-Type", "application/json")
                .body(r#"{"task_ids":["a","b"]}"#);
        });

        let client = client_for(&server);
        let result = client
            .enqueue("p1", vec![make_subtask("s1"), make_subtask("s2")])
            .await
            .expect("enqueue should succeed");

        assert_eq!(result, vec!["a", "b"]);
        mock.assert();
    }

    /// The Authorization header must be `Bearer test-token`.
    #[tokio::test]
    async fn enqueue_sends_authorization_bearer_header() {
        let server = mock_coordinator().await;
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/plans/p1/enqueue")
                .header("Authorization", "Bearer test-token");
            then.status(200)
                .header("Content-Type", "application/json")
                .body(r#"{"task_ids":["x"]}"#);
        });

        let client = client_for(&server);
        client
            .enqueue("p1", vec![make_subtask("s1")])
            .await
            .expect("enqueue should succeed with correct bearer token");

        mock.assert();
    }

    /// The request body must contain the subtasks array.
    #[tokio::test]
    async fn enqueue_sends_subtasks_in_request_body() {
        let server = mock_coordinator().await;
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/plans/plan-body/enqueue")
                .body_contains("st-1");
            then.status(200)
                .header("Content-Type", "application/json")
                .body(r#"{"task_ids":["t-1"]}"#);
        });

        let client = client_for(&server);
        client
            .enqueue("plan-body", vec![make_subtask("st-1")])
            .await
            .expect("enqueue should succeed");

        mock.assert();
    }

    /// GET /distributed/plans/p1/status — client deserialises all five counters.
    #[tokio::test]
    async fn plan_status_returns_counts() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(GET).path("/distributed/plans/p1/status");
            then.status(200)
                .header("Content-Type", "application/json")
                .body(r#"{"queued":1,"leased":2,"done":3,"failed":0,"timed_out":0}"#);
        });

        let client = client_for(&server);
        let snap = client.plan_status("p1").await.expect("plan_status should succeed");

        assert_eq!(snap.queued, 1);
        assert_eq!(snap.leased, 2);
        assert_eq!(snap.done, 3);
        assert_eq!(snap.failed, 0);
        assert_eq!(snap.timed_out, 0);
    }

    /// POST /distributed/workers/w1/heartbeat → 204 should return Ok(()).
    #[tokio::test]
    async fn heartbeat_204_returns_ok() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST).path("/distributed/workers/w1/heartbeat");
            then.status(204);
        });

        let client = client_for(&server);
        client
            .heartbeat("w1", &["rust".to_owned()], None)
            .await
            .expect("heartbeat 204 should be Ok");
    }

    /// The heartbeat body must include capabilities and sandbox_profile.
    #[tokio::test]
    async fn heartbeat_sends_capabilities_and_sandbox_profile() {
        let server = mock_coordinator().await;
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/w-caps/heartbeat")
                .body_contains("rust")
                .body_contains("strict");
            then.status(204);
        });

        let client = client_for(&server);
        client
            .heartbeat(
                "w-caps",
                &["rust".to_owned(), "python".to_owned()],
                Some("strict"),
            )
            .await
            .expect("heartbeat should succeed");

        mock.assert();
    }

    /// POST claim → 204 means no task available; client returns Ok(None).
    #[tokio::test]
    async fn claim_with_204_returns_none() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST).path("/distributed/workers/w1/claim");
            then.status(204);
        });

        let client = client_for(&server);
        let result = client
            .claim("w1", &[], None)
            .await
            .expect("claim 204 should be Ok");
        assert!(result.is_none(), "204 must produce Ok(None)");
    }

    /// POST claim → 200 with ClaimedTaskWire JSON; client deserialises it.
    #[tokio::test]
    async fn claim_with_200_returns_claimed_task() {
        let server = mock_coordinator().await;
        let expires = chrono::Utc::now() + chrono::Duration::seconds(60);
        let body = serde_json::json!({
            "task_id": "task-xyz",
            "plan_id": "plan-1",
            "subtask": {
                "id": "s1",
                "description": "do something",
                "agent": "rust-expert",
                "context": "",
                "depends_on": [],
                "acceptance_tests": [],
                "budget_usd": null,
                "scope": [],
                "requires_capabilities": []
            },
            "lease_expires_at": expires.to_rfc3339(),
            "attempts": 1
        });

        server.mock(|when, then| {
            when.method(POST).path("/distributed/workers/w1/claim");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(body);
        });

        let client = client_for(&server);
        let claimed = client
            .claim("w1", &[], None)
            .await
            .expect("claim 200 should be Ok")
            .expect("should have a task");

        assert_eq!(claimed.task_id, "task-xyz");
        assert_eq!(claimed.plan_id, "plan-1");
        assert_eq!(claimed.attempts, 1);
    }

    /// The claim body must include the lease_secs override when supplied.
    #[tokio::test]
    async fn claim_passes_lease_secs_override_in_body() {
        let server = mock_coordinator().await;
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/w1/claim")
                .body_contains("lease_secs")
                .body_contains("120");
            then.status(204);
        });

        let client = client_for(&server);
        client
            .claim("w1", &[], Some(120))
            .await
            .expect("claim with lease_secs should succeed");

        mock.assert();
    }

    /// POST results → 204 should return Ok(()).
    #[tokio::test]
    async fn complete_204_returns_ok() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/w1/results/task-abc");
            then.status(204);
        });

        let client = client_for(&server);
        client
            .complete("w1", "task-abc", make_completion_body())
            .await
            .expect("complete 204 should be Ok");
    }

    /// The results body must include status_str, response and usage fields.
    #[tokio::test]
    async fn complete_passes_status_response_usage_in_body() {
        let server = mock_coordinator().await;
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/w1/results/task-1")
                .body_contains("status_str")
                .body_contains("completed")
                .body_contains("great output");
            then.status(204);
        });

        let client = client_for(&server);
        client
            .complete("w1", "task-1", make_completion_body())
            .await
            .expect("complete should succeed");

        mock.assert();
    }

    /// POST extend-lease → 204 should return Ok(()).
    #[tokio::test]
    async fn extend_lease_204_returns_ok() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/w1/extend-lease/task-abc");
            then.status(204);
        });

        let client = client_for(&server);
        client
            .extend_lease("w1", "task-abc", 60)
            .await
            .expect("extend_lease 204 should be Ok");
    }

    /// The extend-lease body must contain extend_secs.
    #[tokio::test]
    async fn extend_lease_sends_extend_secs_in_body() {
        let server = mock_coordinator().await;
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/w1/extend-lease/task-1")
                .body_contains("extend_secs")
                .body_contains("90");
            then.status(204);
        });

        let client = client_for(&server);
        client
            .extend_lease("w1", "task-1", 90)
            .await
            .expect("extend_lease should succeed");

        mock.assert();
    }

    // ── Error mapping ─────────────────────────────────────────────────────────

    /// 503 from enqueue → QueueClientError::NotConfigured.
    #[tokio::test]
    async fn enqueue_503_returns_not_configured() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST).path("/distributed/plans/p1/enqueue");
            then.status(503).body("distributed orchestrator not configured");
        });

        let client = client_for(&server);
        let err = client
            .enqueue("p1", vec![make_subtask("s1")])
            .await
            .expect_err("503 must be an error");
        assert!(
            matches!(err, QueueClientError::NotConfigured),
            "expected NotConfigured, got {err:?}"
        );
    }

    /// 401 from enqueue → QueueClientError::Unauthorized.
    #[tokio::test]
    async fn enqueue_401_returns_unauthorized() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST).path("/distributed/plans/p1/enqueue");
            then.status(401).body("unauthorized");
        });

        let client = client_for(&server);
        let err = client
            .enqueue("p1", vec![make_subtask("s1")])
            .await
            .expect_err("401 must be an error");
        assert!(
            matches!(err, QueueClientError::Unauthorized),
            "expected Unauthorized, got {err:?}"
        );
    }

    /// 500 from enqueue → QueueClientError::ServerError with body text.
    #[tokio::test]
    async fn enqueue_500_returns_server_error_with_body() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST).path("/distributed/plans/p1/enqueue");
            then.status(500).body("internal server error details");
        });

        let client = client_for(&server);
        let err = client
            .enqueue("p1", vec![make_subtask("s1")])
            .await
            .expect_err("500 must be an error");
        match err {
            QueueClientError::ServerError { status, body } => {
                assert_eq!(status, 500);
                assert!(
                    body.contains("internal server error details"),
                    "body should contain server message"
                );
            }
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    /// 409 from complete → QueueClientError::LeaseLost.
    #[tokio::test]
    async fn complete_409_returns_lease_lost() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/w1/results/task-abc");
            then.status(409).body(r#"{"error":"lease lost"}"#);
        });

        let client = client_for(&server);
        let err = client
            .complete("w1", "task-abc", make_completion_body())
            .await
            .expect_err("409 must be an error");
        assert!(
            matches!(err, QueueClientError::LeaseLost),
            "expected LeaseLost, got {err:?}"
        );
    }

    /// 409 from extend_lease → QueueClientError::LeaseLost.
    #[tokio::test]
    async fn extend_lease_409_returns_lease_lost() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/w1/extend-lease/task-abc");
            then.status(409).body(r#"{"error":"lease lost"}"#);
        });

        let client = client_for(&server);
        let err = client
            .extend_lease("w1", "task-abc", 60)
            .await
            .expect_err("409 must be an error");
        assert!(
            matches!(err, QueueClientError::LeaseLost),
            "expected LeaseLost, got {err:?}"
        );
    }

    /// 503 from claim → QueueClientError::NotConfigured.
    #[tokio::test]
    async fn claim_503_returns_not_configured() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST).path("/distributed/workers/w1/claim");
            then.status(503).body("not configured");
        });

        let client = client_for(&server);
        let err = client
            .claim("w1", &[], None)
            .await
            .expect_err("503 must be an error");
        assert!(
            matches!(err, QueueClientError::NotConfigured),
            "expected NotConfigured, got {err:?}"
        );
    }

    // ── cancel_plan ───────────────────────────────────────────────────────────

    /// POST /distributed/plans/p1/cancel → 200 {"cancelled": 3} deserialises count.
    #[tokio::test]
    async fn cancel_plan_returns_count_from_body() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST).path("/distributed/plans/p1/cancel");
            then.status(200)
                .header("Content-Type", "application/json")
                .body(r#"{"cancelled":3}"#);
        });

        let client = client_for(&server);
        let count = client
            .cancel_plan("p1")
            .await
            .expect("cancel_plan should succeed on 200");
        assert_eq!(count, 3, "count must match JSON field");
    }

    /// POST /distributed/plans/p1/cancel → 503 maps to NotConfigured.
    #[tokio::test]
    async fn cancel_plan_503_returns_not_configured() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST).path("/distributed/plans/p1/cancel");
            then.status(503).body("not configured");
        });

        let client = client_for(&server);
        let err = client
            .cancel_plan("p1")
            .await
            .expect_err("503 must be an error");
        assert!(
            matches!(err, QueueClientError::NotConfigured),
            "expected NotConfigured, got {err:?}"
        );
    }

    // ── stream_events SSE parser ──────────────────────────────────────────────

    /// Unit-test the hand-rolled SSE frame parser via a byte-slice fixture.
    /// No HTTP server needed — we call the parser helper directly.
    #[test]
    fn stream_events_parses_sse_frames() {
        // Hand-built SSE payload: two frames.
        let raw = concat!(
            "event: task_completion\n",
            "data: {\"task_id\":\"a\",\"status\":\"completed\"}\n",
            "\n",
            "event: task_completion\n",
            "data: {\"task_id\":\"b\",\"status\":\"failed\"}\n",
            "\n",
        );

        let events = crate::queue_client::parse_sse_data_lines(raw.as_bytes());

        assert_eq!(events.len(), 2, "should parse exactly 2 PlanEvents");
        assert_eq!(events[0].task_id, "a");
        assert_eq!(events[0].status, "completed");
        assert_eq!(events[1].task_id, "b");
        assert_eq!(events[1].status, "failed");
    }

    /// Pointing the client at a port with nothing listening produces Http(_).
    #[tokio::test]
    async fn network_error_returns_http_variant() {
        // Port 1 is reserved and will always be refused on localhost.
        let client = QueueClient::new("http://127.0.0.1:1", "tok");
        let err = client
            .enqueue("p", vec![make_subtask("s")])
            .await
            .expect_err("connection refused must be an error");
        assert!(
            matches!(err, QueueClientError::Http(_)),
            "expected Http variant, got {err:?}"
        );
    }

    /// When the server is slow and the client timeout fires, we get Http(_).
    #[tokio::test]
    async fn request_timeout_returns_http_variant() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(POST).path("/distributed/plans/p1/enqueue");
            // Delay 5 seconds — longer than the 100 ms client timeout.
            then.status(200)
                .delay(std::time::Duration::from_secs(5))
                .header("Content-Type", "application/json")
                .body(r#"{"task_ids":[]}"#);
        });

        let client =
            QueueClient::new(server.base_url(), "tok").with_timeout(Duration::from_millis(100));
        let err = client
            .enqueue("p1", vec![make_subtask("s1")])
            .await
            .expect_err("timeout must be an error");
        assert!(
            matches!(err, QueueClientError::Http(_)),
            "expected Http(timeout), got {err:?}"
        );
    }

    // ── pull_results ──────────────────────────────────────────────────────────

    /// GET /distributed/plans/p1/results → 200 with empty array.
    #[tokio::test]
    async fn pull_results_returns_empty_vec_for_empty_response() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(GET).path("/distributed/plans/p1/results");
            then.status(200)
                .header("Content-Type", "application/json")
                .body("[]");
        });

        let client = client_for(&server);
        let results = client.pull_results("p1").await.expect("pull_results should succeed");
        assert!(results.is_empty(), "empty JSON array should deserialise to empty Vec");
    }

    /// GET /distributed/plans/p1/results → 200 with two WorkerResultWire rows.
    #[tokio::test]
    async fn pull_results_deserialises_array_of_worker_result_wire() {
        let server = mock_coordinator().await;
        let body = serde_json::json!([
            {
                "task_id": "t1",
                "plan_id": "p1",
                "agent": "rust-expert",
                "status": "completed",
                "response": "done",
                "error_msg": null,
                "tokens_input": 100,
                "tokens_output": 200,
                "cost_usd": 0.01
            },
            {
                "task_id": "t2",
                "plan_id": "p1",
                "agent": "python-expert",
                "status": "failed",
                "response": "",
                "error_msg": "boom",
                "tokens_input": 50,
                "tokens_output": 0,
                "cost_usd": 0.0
            }
        ]);
        server.mock(|when, then| {
            when.method(GET).path("/distributed/plans/p1/results");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(body);
        });

        let client = client_for(&server);
        let results = client.pull_results("p1").await.expect("pull_results should succeed");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].task_id, "t1");
        assert_eq!(results[0].agent, "rust-expert");
        assert_eq!(results[0].status, "completed");
        assert_eq!(results[0].tokens_input, 100);
        assert_eq!(results[0].tokens_output, 200);
        assert_eq!(results[1].task_id, "t2");
        assert_eq!(results[1].error_msg.as_deref(), Some("boom"));
    }

    /// GET /distributed/plans/p1/results → 503 should map to NotConfigured.
    #[tokio::test]
    async fn pull_results_503_returns_not_configured() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(GET).path("/distributed/plans/p1/results");
            then.status(503).body("not configured");
        });

        let client = client_for(&server);
        let err = client.pull_results("p1").await.expect_err("503 must be an error");
        assert!(
            matches!(err, QueueClientError::NotConfigured),
            "expected NotConfigured, got {err:?}"
        );
    }

    /// GET /distributed/plans/p1/results → 500 should map to ServerError.
    #[tokio::test]
    async fn pull_results_500_returns_server_error() {
        let server = mock_coordinator().await;
        server.mock(|when, then| {
            when.method(GET).path("/distributed/plans/p1/results");
            then.status(500).body("internal error detail");
        });

        let client = client_for(&server);
        let err = client.pull_results("p1").await.expect_err("500 must be an error");
        match err {
            QueueClientError::ServerError { status, body } => {
                assert_eq!(status, 500);
                assert!(body.contains("internal error detail"));
            }
            other => panic!("expected ServerError, got {other:?}"),
        }
    }

    // ── Shape validation ──────────────────────────────────────────────────────

    /// A SubTask with non-empty `requires_capabilities` survives the client round-trip.
    #[tokio::test]
    async fn claim_returns_subtask_with_requires_capabilities_field() {
        let server = mock_coordinator().await;
        let expires = chrono::Utc::now() + chrono::Duration::seconds(60);
        let body = serde_json::json!({
            "task_id": "task-caps",
            "plan_id": "plan-caps",
            "subtask": {
                "id": "s-caps",
                "description": "needs rust and python",
                "agent": "poly-agent",
                "context": "",
                "depends_on": [],
                "acceptance_tests": [],
                "budget_usd": null,
                "scope": [],
                "requires_capabilities": ["rust", "python"]
            },
            "lease_expires_at": expires.to_rfc3339(),
            "attempts": 1
        });

        server.mock(|when, then| {
            when.method(POST).path("/distributed/workers/w1/claim");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(body);
        });

        let client = client_for(&server);
        let claimed = client
            .claim("w1", &["rust".to_owned(), "python".to_owned()], None)
            .await
            .expect("claim should succeed")
            .expect("should have a task");

        assert_eq!(
            claimed.subtask.requires_capabilities,
            vec!["rust".to_owned(), "python".to_owned()],
            "requires_capabilities must survive client deserialisation"
        );
    }
}
