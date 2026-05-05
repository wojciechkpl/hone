//! WorkerPool — long-running poll loop for a distributed hone worker.
//!
//! See Phase 3a design: docs/research/p2-4-distributed-orchestrator-2026-05.md §10.
//!
//! The public surface is intentionally minimal so the test-architect and
//! implementer work from the same contract without drift.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::task::JoinHandle;

use hone_core::queue_client::{CompletionRequestBody, QueueClient};
use hone_core::task_plan::SubTask;

// ── ExecutionOutcome ──────────────────────────────────────────────────────────

/// Result of executing one claimed task.
#[derive(Debug, Clone, PartialEq)]
pub struct ExecutionOutcome {
    /// "completed" | "failed" | "timed_out"
    pub status_str: String,
    pub response: String,
    pub error_msg: Option<String>,
    pub tokens_input: u32,
    pub tokens_output: u32,
    pub cost_usd: f64,
    /// SHA-256 hex head of the per-task ProvenanceLog. None when the executor
    /// does not support provenance.
    pub provenance_head: Option<String>,
}

impl ExecutionOutcome {
    pub fn completed(response: impl Into<String>) -> Self {
        Self {
            status_str: "completed".into(),
            response: response.into(),
            error_msg: None,
            tokens_input: 0,
            tokens_output: 0,
            cost_usd: 0.0,
            provenance_head: None,
        }
    }

    pub fn failed(msg: impl Into<String>) -> Self {
        let m = msg.into();
        Self {
            status_str: "failed".into(),
            response: String::new(),
            error_msg: Some(m),
            tokens_input: 0,
            tokens_output: 0,
            cost_usd: 0.0,
            provenance_head: None,
        }
    }

    pub fn timed_out(msg: impl Into<String>) -> Self {
        let m = msg.into();
        Self {
            status_str: "timed_out".into(),
            response: String::new(),
            error_msg: Some(m),
            tokens_input: 0,
            tokens_output: 0,
            cost_usd: 0.0,
            provenance_head: None,
        }
    }

    /// Builder to attach a provenance head hash to this outcome.
    pub fn with_provenance_head(mut self, head: String) -> Self {
        self.provenance_head = Some(head);
        self
    }
}

impl From<ExecutionOutcome> for CompletionRequestBody {
    fn from(o: ExecutionOutcome) -> Self {
        Self {
            status_str: o.status_str,
            response: o.response,
            error_msg: o.error_msg,
            usage: hone_proto::TokenUsage {
                input: o.tokens_input,
                output: o.tokens_output,
                ..Default::default()
            },
            cost_usd: o.cost_usd,
            provenance_head: o.provenance_head,
        }
    }
}

// ── TaskExecutor ──────────────────────────────────────────────────────────────

/// Trait the worker pool delegates task execution to.
/// Production implementation (AgentTaskExecutor) is Phase 3b.
#[async_trait]
pub trait TaskExecutor: Send + Sync {
    async fn execute(&self, task: &SubTask) -> ExecutionOutcome;
}

// ── WorkerPool ────────────────────────────────────────────────────────────────

/// Long-running poll loop for one worker.
pub struct WorkerPool {
    client: Arc<QueueClient>,
    worker_id: String,
    capabilities: Vec<String>,
    sandbox_profile_json: Option<String>,
    executor: Arc<dyn TaskExecutor>,
    poll_interval: Duration,
    heartbeat_interval: Duration,
    lease_secs: u64,
}

impl WorkerPool {
    pub fn new(
        client: Arc<QueueClient>,
        worker_id: impl Into<String>,
        capabilities: Vec<String>,
        executor: Arc<dyn TaskExecutor>,
    ) -> Self {
        Self {
            client,
            worker_id: worker_id.into(),
            capabilities,
            sandbox_profile_json: None,
            executor,
            poll_interval: Duration::from_secs(1),
            heartbeat_interval: Duration::from_secs(30),
            lease_secs: 60,
        }
    }

    pub fn with_poll_interval(mut self, d: Duration) -> Self {
        self.poll_interval = d;
        self
    }

    pub fn with_heartbeat_interval(mut self, d: Duration) -> Self {
        self.heartbeat_interval = d;
        self
    }

    pub fn with_lease_secs(mut self, s: u64) -> Self {
        self.lease_secs = s;
        self
    }

    pub fn with_sandbox_profile(mut self, profile_json: impl Into<String>) -> Self {
        self.sandbox_profile_json = Some(profile_json.into());
        self
    }

    /// Spawn the poll loop. Returns a JoinHandle the caller can `.abort()` on
    /// shutdown.
    pub fn spawn(self) -> JoinHandle<()> {
        tokio::spawn(async move { self.run_loop().await })
    }

    async fn run_loop(self) {
        // Fire heartbeat immediately on startup so tests see at least one call.
        if let Err(e) = self
            .client
            .heartbeat(
                &self.worker_id,
                &self.capabilities,
                self.sandbox_profile_json.as_deref(),
            )
            .await
        {
            tracing::warn!("initial heartbeat failed: {e}");
        }

        let mut heartbeat_ticker = tokio::time::interval(self.heartbeat_interval);
        // Skip the immediate tick — we already fired above.
        heartbeat_ticker.tick().await;

        let mut poll_ticker = tokio::time::interval(self.poll_interval);
        // poll_ticker fires immediately on first .tick() — that's intentional.

        loop {
            tokio::select! {
                _ = heartbeat_ticker.tick() => {
                    if let Err(e) = self
                        .client
                        .heartbeat(
                            &self.worker_id,
                            &self.capabilities,
                            self.sandbox_profile_json.as_deref(),
                        )
                        .await
                    {
                        tracing::warn!("heartbeat failed: {e}");
                    }
                }
                _ = poll_ticker.tick() => {
                    match self
                        .client
                        .claim(&self.worker_id, &self.capabilities, Some(self.lease_secs))
                        .await
                    {
                        Ok(Some(claimed)) => {
                            let outcome = self.executor.execute(&claimed.subtask).await;
                            let body = CompletionRequestBody {
                                status_str: outcome.status_str,
                                response: outcome.response,
                                error_msg: outcome.error_msg,
                                usage: hone_proto::TokenUsage {
                                    input: outcome.tokens_input,
                                    output: outcome.tokens_output,
                                    ..Default::default()
                                },
                                cost_usd: outcome.cost_usd,
                                provenance_head: outcome.provenance_head,
                            };
                            if let Err(e) = self
                                .client
                                .complete(&self.worker_id, &claimed.task_id, body)
                                .await
                            {
                                tracing::warn!("complete failed: {e}");
                            }
                        }
                        Ok(None) => {} // no work; sleep until next tick
                        Err(e) => {
                            tracing::warn!("claim failed: {e}");
                        }
                    }
                }
            }
        }
    }
}

// ── MockExecutor ──────────────────────────────────────────────────────────────

#[cfg(any(test, feature = "test-stub"))]
pub struct MockExecutor {
    outcomes: tokio::sync::Mutex<Vec<ExecutionOutcome>>,
    pub call_count: Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(any(test, feature = "test-stub"))]
impl MockExecutor {
    pub fn new(canned: Vec<ExecutionOutcome>) -> Self {
        Self {
            outcomes: tokio::sync::Mutex::new(canned),
            call_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        }
    }
}

#[cfg(any(test, feature = "test-stub"))]
#[async_trait]
impl TaskExecutor for MockExecutor {
    async fn execute(&self, _task: &SubTask) -> ExecutionOutcome {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut outcomes = self.outcomes.lock().await;
        if !outcomes.is_empty() {
            outcomes.remove(0)
        } else {
            ExecutionOutcome {
                status_str: "completed".to_owned(),
                response: "default mock response".to_owned(),
                error_msg: None,
                tokens_input: 0,
                tokens_output: 0,
                cost_usd: 0.0,
                provenance_head: None,
            }
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    use httpmock::prelude::*;

    // ── helper ────────────────────────────────────────────────────────────────

    /// Build a WorkerPool pointing at the httpmock server with tight timing so
    /// tests complete in milliseconds.
    fn build_pool(server: &MockServer, executor: Arc<dyn TaskExecutor>) -> WorkerPool {
        let client = Arc::new(QueueClient::new(server.base_url(), "test-token"));
        WorkerPool::new(client, "worker-test", vec!["rust".to_owned()], executor)
            .with_poll_interval(Duration::from_millis(10))
            .with_heartbeat_interval(Duration::from_millis(10))
            .with_lease_secs(60)
    }

    fn make_claimed_task_json(task_id: &str) -> serde_json::Value {
        let expires = chrono::Utc::now() + chrono::Duration::seconds(60);
        serde_json::json!({
            "task_id": task_id,
            "plan_id": "plan-test",
            "subtask": {
                "id": "s1",
                "description": "do the thing",
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
        })
    }

    fn mock_heartbeat_204(server: &MockServer) -> httpmock::Mock {
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/heartbeat");
            then.status(204);
        })
    }

    fn mock_claim_204(server: &MockServer) -> httpmock::Mock {
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(204);
        })
    }

    // ── Heartbeat path ────────────────────────────────────────────────────────

    /// Pool must call heartbeat at least once shortly after spawn.
    #[tokio::test]
    async fn pool_calls_heartbeat_on_startup() {
        let server = MockServer::start_async().await;
        let hb_mock = mock_heartbeat_204(&server);
        mock_claim_204(&server);

        let executor = Arc::new(MockExecutor::new(vec![]));
        let handle = build_pool(&server, executor)
            .with_heartbeat_interval(Duration::from_millis(10))
            .spawn();

        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.abort();

        assert!(
            hb_mock.hits() >= 1,
            "heartbeat must be called at least once on startup; hits={}",
            hb_mock.hits()
        );
    }

    /// With a 50ms heartbeat interval the pool must heartbeat at least 3 times
    /// in 200ms.
    #[tokio::test]
    async fn pool_calls_heartbeat_on_interval_repeatedly() {
        let server = MockServer::start_async().await;
        let hb_mock = mock_heartbeat_204(&server);
        mock_claim_204(&server);

        let executor = Arc::new(MockExecutor::new(vec![]));
        let handle = build_pool(&server, executor)
            .with_heartbeat_interval(Duration::from_millis(50))
            .spawn();

        tokio::time::sleep(Duration::from_millis(250)).await;
        handle.abort();

        assert!(
            hb_mock.hits() >= 3,
            "heartbeat must fire >= 3 times with 50ms interval over 250ms; hits={}",
            hb_mock.hits()
        );
    }

    // ── Claim + execute path ──────────────────────────────────────────────────

    /// Pool claims a task, passes it to the executor, then posts results.
    #[tokio::test]
    async fn pool_claims_and_executes_task() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);

        // First claim returns a task; subsequent calls return 204.
        let claim_body = make_claimed_task_json("task-abc");
        let claim_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(claim_body);
        });
        // After the first task is consumed the pool keeps polling — respond 204.
        let claim_empty = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(204);
        });
        let results_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/results/task-abc");
            then.status(204);
        });

        let executor = Arc::new(MockExecutor::new(vec![ExecutionOutcome {
            status_str: "completed".to_owned(),
            response: "result-text".to_owned(),
            error_msg: None,
            tokens_input: 10,
            tokens_output: 20,
            cost_usd: 0.001,
            provenance_head: None,
        }]));
        let call_count = executor.call_count.clone();

        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.abort();

        assert!(
            call_count.load(Ordering::Relaxed) >= 1,
            "executor must be called at least once"
        );
        assert!(
            results_mock.hits() >= 1,
            "results endpoint must be hit at least once"
        );
        // Suppress unused-variable warning — we care it was set up correctly.
        let _ = (claim_mock.hits(), claim_empty.hits());
    }

    /// When claim always returns 204 (no eligible task) the pool keeps running
    /// without calling the executor.
    #[tokio::test]
    async fn pool_handles_204_no_eligible_task() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);
        mock_claim_204(&server);

        let executor = Arc::new(MockExecutor::new(vec![]));
        let call_count = executor.call_count.clone();

        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.abort();

        assert_eq!(
            call_count.load(Ordering::Relaxed),
            0,
            "executor must not be called when no task is available"
        );
    }

    /// The claim request body must contain the configured capabilities.
    #[tokio::test]
    async fn pool_passes_capabilities_to_claim_request() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);

        let caps_claim = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim")
                .body_contains("rust");
            then.status(204);
        });

        let executor = Arc::new(MockExecutor::new(vec![]));
        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(80)).await;
        handle.abort();

        assert!(
            caps_claim.hits() >= 1,
            "claim request must include capabilities in body; hits={}",
            caps_claim.hits()
        );
    }

    /// The claim body must carry the configured lease_secs value.
    #[tokio::test]
    async fn pool_uses_configured_lease_secs_on_claim() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);

        // lease_secs=120 set via builder
        let lease_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim")
                .body_contains("120");
            then.status(204);
        });

        let executor = Arc::new(MockExecutor::new(vec![]));
        let client = Arc::new(QueueClient::new(server.base_url(), "test-token"));
        let handle = WorkerPool::new(client, "worker-test", vec!["rust".to_owned()], executor)
            .with_poll_interval(Duration::from_millis(10))
            .with_heartbeat_interval(Duration::from_millis(10))
            .with_lease_secs(120)
            .spawn();

        tokio::time::sleep(Duration::from_millis(80)).await;
        handle.abort();

        assert!(
            lease_mock.hits() >= 1,
            "claim body must contain configured lease_secs=120; hits={}",
            lease_mock.hits()
        );
    }

    // ── Result reporting ──────────────────────────────────────────────────────

    /// A completed executor outcome must result in status="completed" + correct
    /// response text in the results POST body.
    #[tokio::test]
    async fn pool_posts_completed_result_with_executor_output() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);

        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(make_claimed_task_json("task-comp"));
        });
        // Drain subsequent claim calls.
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(204);
        });

        let results_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/results/task-comp")
                .body_contains("completed")
                .body_contains("result-text");
            then.status(204);
        });

        let executor = Arc::new(MockExecutor::new(vec![ExecutionOutcome::completed(
            "result-text",
        )]));
        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.abort();

        assert!(
            results_mock.hits() >= 1,
            "results endpoint must be hit with status=completed"
        );
    }

    /// A failed executor outcome must result in status="failed" in the results body.
    #[tokio::test]
    async fn pool_posts_failed_result_when_executor_returns_failed() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);

        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(make_claimed_task_json("task-fail"));
        });
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(204);
        });

        let results_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/results/task-fail")
                .body_contains("failed");
            then.status(204);
        });

        let executor = Arc::new(MockExecutor::new(vec![ExecutionOutcome::failed(
            "something exploded",
        )]));
        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.abort();

        assert!(
            results_mock.hits() >= 1,
            "results body must contain status=failed"
        );
    }

    /// A timed_out executor outcome must result in status="timed_out" in the
    /// results body.
    #[tokio::test]
    async fn pool_posts_timed_out_result_when_executor_returns_timed_out() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);

        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(make_claimed_task_json("task-to"));
        });
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(204);
        });

        let results_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/results/task-to")
                .body_contains("timed_out");
            then.status(204);
        });

        let executor = Arc::new(MockExecutor::new(vec![ExecutionOutcome::timed_out(
            "exceeded 300s",
        )]));
        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.abort();

        assert!(
            results_mock.hits() >= 1,
            "results body must contain status=timed_out"
        );
    }

    // ── Error resilience ──────────────────────────────────────────────────────

    /// When the claim endpoint returns 500, the pool logs a warning and keeps
    /// polling. The executor must never be called.
    #[tokio::test]
    async fn pool_continues_after_claim_error() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);

        let error_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(500).body("internal error");
        });

        let executor = Arc::new(MockExecutor::new(vec![]));
        let call_count = executor.call_count.clone();

        let handle = build_pool(&server, executor).spawn();
        // Give enough time for multiple poll cycles.
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.abort();

        // Pool must have retried (multiple 500s received) without panicking.
        assert!(
            error_mock.hits() >= 2,
            "pool must keep retrying after 500 on claim; hits={}",
            error_mock.hits()
        );
        assert_eq!(
            call_count.load(Ordering::Relaxed),
            0,
            "executor must not be called when claim fails"
        );
    }

    /// When the results endpoint returns 500, the pool logs but does not crash;
    /// another claim cycle happens.
    #[tokio::test]
    async fn pool_continues_after_results_post_error() {
        let server = MockServer::start_async().await;
        let hb_mock = mock_heartbeat_204(&server);

        // Claim always returns the same task — pool will execute it,
        // post a result, see the result POST fail, then loop again.  We
        // verify resilience by checking the pool kept heartbeating
        // throughout (heartbeat ticks are independent of the failing
        // results path, so a crashed loop would stop them).
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(make_claimed_task_json("task-results-err"));
        });

        // Results endpoint always fails.
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/results/task-results-err");
            then.status(500).body("results server error");
        });

        let executor = Arc::new(MockExecutor::new(vec![ExecutionOutcome::completed("ok")]));
        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(200)).await;
        handle.abort();

        // Heartbeat keeps firing -> the loop survived the results 500.
        assert!(
            hb_mock.hits() >= 2,
            "pool must continue heartbeating after results POST failure; hits={}",
            hb_mock.hits()
        );
    }

    /// When the heartbeat endpoint returns 500, the pool logs but keeps polling
    /// for claims.
    #[tokio::test]
    async fn pool_continues_after_heartbeat_error() {
        let server = MockServer::start_async().await;

        let hb_error = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/heartbeat");
            then.status(500).body("heartbeat error");
        });

        let claim_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(204);
        });

        let executor = Arc::new(MockExecutor::new(vec![]));
        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.abort();

        // Heartbeat errored but did not crash the loop — claim was still polled.
        assert!(
            hb_error.hits() >= 1,
            "heartbeat 500 must be hit at least once"
        );
        assert!(
            claim_mock.hits() >= 1,
            "claim must still be polled after heartbeat error; hits={}",
            claim_mock.hits()
        );
    }

    // ── Sandbox profile ───────────────────────────────────────────────────────

    /// When built with `.with_sandbox_profile(...)`, the heartbeat body must
    /// contain the sandbox_profile JSON string.
    #[tokio::test]
    async fn pool_includes_sandbox_profile_in_heartbeat() {
        let server = MockServer::start_async().await;

        // The QueueClient serializes sandbox_profile as a JSON string field,
        // so the literal "{\"strict\":true}" appears in the body with the
        // inner quotes escaped (the outer "{...}" is wrapped in another
        // string).  body_contains matches a substring, so search for the
        // escaped form that's actually present in the JSON wire payload.
        let hb_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/heartbeat")
                .body_contains(r#"\"strict\":true"#);
            then.status(204);
        });
        mock_claim_204(&server);

        let executor = Arc::new(MockExecutor::new(vec![]));
        let client = Arc::new(QueueClient::new(server.base_url(), "test-token"));
        let handle =
            WorkerPool::new(client, "worker-test", vec!["rust".to_owned()], executor)
                .with_poll_interval(Duration::from_millis(10))
                .with_heartbeat_interval(Duration::from_millis(10))
                .with_sandbox_profile(r#"{"strict":true}"#)
                .spawn();

        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.abort();

        assert!(
            hb_mock.hits() >= 1,
            "heartbeat must include sandbox_profile; hits={}",
            hb_mock.hits()
        );
    }

    /// When no sandbox profile is configured the heartbeat body must contain a
    /// null sandbox_profile field.
    #[tokio::test]
    async fn pool_with_no_sandbox_profile_sends_null_in_heartbeat() {
        let server = MockServer::start_async().await;

        let hb_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/heartbeat")
                .body_contains("null");
            then.status(204);
        });
        mock_claim_204(&server);

        let executor = Arc::new(MockExecutor::new(vec![]));
        // Do NOT call .with_sandbox_profile() — default is None.
        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.abort();

        assert!(
            hb_mock.hits() >= 1,
            "heartbeat with no sandbox profile must send null; hits={}",
            hb_mock.hits()
        );
    }

    // ── Lifecycle ─────────────────────────────────────────────────────────────

    /// Aborting the JoinHandle must stop the pool within 100ms.
    #[tokio::test]
    async fn pool_can_be_aborted() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);
        mock_claim_204(&server);

        let executor = Arc::new(MockExecutor::new(vec![]));
        let handle = build_pool(&server, executor).spawn();

        handle.abort();
        tokio::time::sleep(Duration::from_millis(100)).await;

        assert!(
            handle.is_finished(),
            "pool JoinHandle must be finished within 100ms of abort()"
        );
    }

    // ── Provenance head propagation ───────────────────────────────────────────

    /// MockExecutor returns an outcome with a provenance_head; the body posted
    /// to /results must include that head in the JSON payload.
    #[tokio::test]
    async fn pool_passes_provenance_head_to_complete_endpoint() {
        let server = MockServer::start_async().await;
        mock_heartbeat_204(&server);

        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(200)
                .header("Content-Type", "application/json")
                .json_body(make_claimed_task_json("task-prov"));
        });
        // Drain subsequent claims.
        server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/claim");
            then.status(204);
        });

        let expected_head = "a".repeat(64);
        let results_mock = server.mock(|when, then| {
            when.method(POST)
                .path("/distributed/workers/worker-test/results/task-prov")
                .body_contains(&"a".repeat(64));
            then.status(204);
        });

        let executor = Arc::new(MockExecutor::new(vec![ExecutionOutcome {
            status_str: "completed".to_owned(),
            response: "done".to_owned(),
            error_msg: None,
            tokens_input: 0,
            tokens_output: 0,
            cost_usd: 0.0,
            provenance_head: Some(expected_head),
        }]));
        let handle = build_pool(&server, executor).spawn();
        tokio::time::sleep(Duration::from_millis(150)).await;
        handle.abort();

        assert!(
            results_mock.hits() >= 1,
            "results endpoint must be called with provenance_head in body; hits={}",
            results_mock.hits()
        );
    }
}
