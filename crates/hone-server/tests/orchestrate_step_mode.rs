#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! W35 of `docs/plans/worker-tools-spec.md`: `POST /orchestrate` step mode.
//!
//! Assumed request fields (same shape and rules as `/run`):
//! `request_id: Option<String>` and `approval_policy: Option<ToolApprovalPolicy>`
//! on the body, e.g. `{"approval_policy": {"AskRisky": {"always_allowed": []}}}`.
//! Step mode = policy given and not `Auto`. Answers go to
//! `POST /run/{request_id}/step`.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::{Stream, StreamExt, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, ToolCallRequest};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_server::{AppState, router};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Scripted provider: decompose -> one rust-expert subtask; the worker calls
// `shell` once (`echo ok`), then answers; merge answers "merged".
// ---------------------------------------------------------------------------

const MARKER: &str = "SRV-WORKER";

struct Scripted {
    worker_tools: Mutex<Vec<Vec<String>>>,
    worker_requests: Mutex<usize>,
    /// Sleep (tokio clock) before answering a worker request.
    worker_delay: Option<Duration>,
}

impl Scripted {
    fn new(worker_delay: Option<Duration>) -> Arc<Self> {
        Arc::new(Self {
            worker_tools: Mutex::new(vec![]),
            worker_requests: Mutex::new(0),
            worker_delay,
        })
    }
    fn first_worker_tools(&self) -> Vec<String> {
        self.worker_tools
            .lock()
            .unwrap()
            .first()
            .cloned()
            .expect("the worker called the provider")
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    fn id(&self) -> &str {
        "scripted-orchestrate"
    }
    fn display_name(&self) -> &str {
        "Scripted"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "Test".to_owned(),
            context_length: 4096,
            supports_tools: true,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let text = request
            .messages
            .iter()
            .map(|m| m.content.as_text())
            .collect::<Vec<_>>()
            .join("\n");
        let token = |t: String| vec![ProviderEvent::Token { text: t }, ProviderEvent::Done];
        let events = if text.contains("You are a lead agent. Decompose") {
            token(
                json!({"goal": "g", "subtasks": [{
                    "id": "1", "description": MARKER, "agent": "rust-expert",
                    "context": "", "depends_on": [], "acceptance_tests": [],
                    "budget_usd": null, "scope": []
                }]})
                .to_string(),
            )
        } else if text.contains("You are the lead agent. Based on") {
            token("merged".to_owned())
        } else {
            if let Some(d) = self.worker_delay {
                tokio::time::sleep(d).await;
            }
            let mut names: Vec<String> = request.tools.iter().map(|t| t.name.clone()).collect();
            names.sort();
            self.worker_tools.lock().unwrap().push(names);
            let n = {
                let mut n = self.worker_requests.lock().unwrap();
                *n += 1;
                *n
            };
            if n == 1 {
                vec![
                    ProviderEvent::ToolCall(ToolCallRequest {
                        id: "c1".to_owned(),
                        name: "shell".to_owned(),
                        arguments: json!({ "command": "echo ok" }),
                    }),
                    ProviderEvent::Done,
                ]
            } else {
                token("worker done".to_owned())
            }
        };
        Ok(Box::pin(stream::iter(events)))
    }
    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        10
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

// ---------------------------------------------------------------------------
// HTTP + SSE helpers
// ---------------------------------------------------------------------------

fn post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn ask_risky() -> Value {
    json!({ "AskRisky": { "always_allowed": [] } })
}

fn state_with(provider: Arc<Scripted>, dir: &std::path::Path) -> AppState {
    AppState::new(None)
        .with_workspace_root(std::fs::canonicalize(dir).unwrap())
        .with_mock_provider(provider)
}

/// A live SSE stream: events arrive on `rx` as they are produced.
struct Sse {
    rx: mpsc::UnboundedReceiver<(String, Value)>,
    seen: Vec<(String, Value)>,
}

impl Sse {
    fn start(resp: axum::response::Response) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut data = resp.into_body().into_data_stream();
            let mut buf = String::new();
            while let Some(Ok(chunk)) = data.next().await {
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(pos) = buf.find("\n\n") {
                    let frame: String = buf.drain(..pos + 2).collect();
                    let mut name = String::new();
                    let mut payload = String::new();
                    for line in frame.lines() {
                        if let Some(n) = line.strip_prefix("event: ") {
                            name = n.trim().to_owned();
                        } else if let Some(d) = line.strip_prefix("data: ") {
                            payload.push_str(d);
                        }
                    }
                    if !name.is_empty() {
                        let v = serde_json::from_str(&payload).unwrap_or(Value::Null);
                        let _ = tx.send((name, v));
                    }
                }
            }
        });
        Self { rx, seen: vec![] }
    }

    /// Next event named `name` (others are kept in `seen`). Gives up after
    /// `limit` of the tokio clock.
    async fn wait_for(&mut self, name: &str, limit: Duration) -> Value {
        let found = tokio::time::timeout(limit, async {
            while let Some((n, v)) = self.rx.recv().await {
                self.seen.push((n.clone(), v.clone()));
                if n == name {
                    return Some(v);
                }
            }
            None
        })
        .await;
        match found {
            Ok(Some(v)) => v,
            Ok(None) => panic!("stream ended before `{name}`; seen: {:?}", self.names()),
            Err(_) => panic!("no `{name}` event; seen: {:?}", self.names()),
        }
    }

    /// Read until the stream ends.
    async fn finish(&mut self, limit: Duration) {
        let ended = tokio::time::timeout(limit, async {
            while let Some((n, v)) = self.rx.recv().await {
                self.seen.push((n, v));
            }
        })
        .await;
        assert!(
            ended.is_ok(),
            "stream did not end; seen: {:?}",
            self.names()
        );
    }

    fn names(&self) -> Vec<&str> {
        self.seen.iter().map(|(n, _)| n.as_str()).collect()
    }

    fn data_of(&self, name: &str) -> Vec<&Value> {
        self.seen
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v)
            .collect()
    }
}

/// Whether `id` can be registered (the registry's size is test-only API).
fn is_free(state: &AppState, id: &str) -> bool {
    state.run_registry.register(id.to_owned()).is_ok()
}

const WAIT: Duration = Duration::from_secs(10);

async fn start_run(state: &AppState, uri: &str, body: Value) -> Sse {
    let resp = router(state.clone())
        .oneshot(post(uri, body))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    Sse::start(resp)
}

async fn step(state: &AppState, id: &str, action: Value) {
    let resp = router(state.clone())
        .oneshot(post(&format!("/run/{id}/step"), action))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
}

fn step_body(id: &str) -> Value {
    json!({ "prompt": "do it", "request_id": id, "approval_policy": ask_risky() })
}

// ---------------------------------------------------------------------------
// Step mode on
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w35_step_mode_worker_pause_should_name_the_worker_and_proceed_should_run_the_call() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Scripted::new(None);
    let state = state_with(Arc::clone(&provider), dir.path());

    let mut sse = start_run(&state, "/orchestrate", step_body("orch-1")).await;
    let paused = sse.wait_for("tool_call_paused", WAIT).await;
    assert_eq!(paused["id"], "c1");
    assert_eq!(
        paused["worker"],
        json!({ "task_id": "1", "agent": "rust-expert" })
    );
    step(&state, "orch-1", json!("Proceed")).await;
    sse.finish(WAIT).await;

    let ends: Vec<&Value> = sse.data_of("tool_call_end");
    let c1 = ends.iter().find(|e| e["id"] == "c1").expect("c1 ended");
    assert_eq!(c1["is_error"], false, "{c1}");
    assert!(
        c1["result"]["stdout"].as_str().unwrap().contains("ok"),
        "{c1}"
    );
    assert!(sse.names().contains(&"done"), "{:?}", sse.names());
    // The worker was given its agent's tools from the server's set.
    assert_eq!(
        provider.first_worker_tools(),
        vec![
            "edit_file",
            "glob",
            "grep",
            "read_file",
            "shell",
            "write_file"
        ]
    );
}

#[tokio::test]
async fn test_w35_step_mode_deny_should_not_run_the_call_and_the_run_should_still_end() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Scripted::new(None);
    let state = state_with(provider, dir.path());

    let mut sse = start_run(&state, "/orchestrate", step_body("orch-2")).await;
    sse.wait_for("tool_call_paused", WAIT).await;
    step(&state, "orch-2", json!({ "Deny": "no thanks" })).await;
    sse.finish(WAIT).await;

    let ends = sse.data_of("tool_call_end");
    let c1 = ends.iter().find(|e| e["id"] == "c1").expect("c1 ended");
    assert_eq!(c1["is_error"], true);
    assert_eq!(c1["result"]["error"], "denied by user");
    assert!(sse.names().contains(&"done"));
}

#[tokio::test]
async fn test_w35_step_mode_without_request_id_should_be_400() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with(Scripted::new(None), dir.path());

    let resp = router(state)
        .oneshot(post(
            "/orchestrate",
            json!({ "prompt": "do it", "approval_policy": ask_risky() }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_w35_step_mode_with_a_request_id_in_use_should_be_409() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with(Scripted::new(None), dir.path());
    let (_rx, _guard) = state
        .run_registry
        .register("taken".into())
        .expect("register");

    let resp = router(state.clone())
        .oneshot(post("/orchestrate", step_body("taken")))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_w35_step_mode_run_should_be_registered_while_running_and_free_after_it_ends() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with(Scripted::new(None), dir.path());

    let mut sse = start_run(&state, "/orchestrate", step_body("orch-3")).await;
    sse.wait_for("tool_call_paused", WAIT).await;
    assert!(!is_free(&state, "orch-3"), "registered for the run's life");
    step(&state, "orch-3", json!("Proceed")).await;
    sse.finish(WAIT).await;

    // The guard drops when the run task ends; give it a moment.
    let mut free = false;
    for _ in 0..50 {
        if is_free(&state, "orch-3") {
            free = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(free, "the id is free again after the run");
}

// ---------------------------------------------------------------------------
// Step mode off: unchanged (W22)
// ---------------------------------------------------------------------------

async fn assert_unchanged_read_only(body: Value, request_id: Option<&str>, label: &str) {
    let dir = tempfile::tempdir().unwrap();
    let provider = Scripted::new(None);
    let state = state_with(Arc::clone(&provider), dir.path());

    let mut sse = start_run(&state, "/orchestrate", body).await;
    sse.finish(WAIT).await;

    assert!(
        sse.data_of("tool_call_paused").is_empty(),
        "{label}: nothing pauses: {:?}",
        sse.names()
    );
    assert_eq!(
        provider.first_worker_tools(),
        vec!["glob", "grep", "read_file"],
        "{label}: workers stay read-only"
    );
    if let Some(id) = request_id {
        assert!(is_free(&state, id), "{label}: nothing registered");
    }
}

#[tokio::test]
async fn test_w35_without_approval_policy_workers_should_stay_read_only() {
    assert_unchanged_read_only(json!({ "prompt": "do it" }), None, "no policy").await;
}

#[tokio::test]
async fn test_w35_with_auto_policy_workers_should_stay_read_only_even_with_a_request_id() {
    assert_unchanged_read_only(
        json!({ "prompt": "do it", "approval_policy": "Auto", "request_id": "auto-1" }),
        Some("auto-1"),
        "Auto",
    )
    .await;
}

// ---------------------------------------------------------------------------
// dry_run ignores the step-mode fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_w35_dry_run_should_ignore_step_mode_fields_and_register_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with(Scripted::new(None), dir.path());

    let mut sse = start_run(&state, "/orchestrate?dry_run=true", step_body("dry-1")).await;
    sse.finish(WAIT).await;

    assert!(sse.names().contains(&"plan_created"), "{:?}", sse.names());
    assert!(sse.data_of("tool_call_paused").is_empty());
    assert!(is_free(&state, "dry-1"), "dry run registers nothing");
}

#[tokio::test]
async fn test_w35_dry_run_with_a_policy_but_no_request_id_should_not_be_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with(Scripted::new(None), dir.path());

    let resp = router(state)
        .oneshot(post(
            "/orchestrate?dry_run=true",
            json!({ "prompt": "do it", "approval_policy": ask_risky() }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// No per-worker timeout in step mode
// ---------------------------------------------------------------------------

#[tokio::test(start_paused = true)]
async fn test_w35_step_mode_worker_waiting_for_an_answer_past_300_seconds_should_not_time_out() {
    let dir = tempfile::tempdir().unwrap();
    let state = state_with(Scripted::new(None), dir.path());

    let mut sse = start_run(&state, "/orchestrate", step_body("orch-slow")).await;
    let limit = Duration::from_secs(86_400);
    sse.wait_for("tool_call_paused", limit).await;
    // A person takes their time. (Deny afterwards: no process to run under
    // the paused clock.)
    tokio::time::sleep(Duration::from_secs(400)).await;
    step(&state, "orch-slow", json!({ "Deny": "late" })).await;
    sse.finish(limit).await;

    let completed: Vec<&Value> = sse.data_of("worker_completed");
    assert_eq!(completed.len(), 1, "{:?}", sse.names());
    assert_eq!(completed[0]["status"], "completed", "{}", completed[0]);
}
