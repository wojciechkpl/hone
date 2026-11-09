//! R9b (docs/plans/tui-approval-fix-spec.md section 7): `POST /run` records the spend of a run
//! whatever its outcome. A scripted provider is injected with `AppState::with_mock_provider`,
//! so no seam beyond the existing one is needed.
//!
//! Spend per model call is 100 input + 50 output tokens = 150 tokens, priced with the same
//! pricing the handler resolves for `claude-sonnet-4-6`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::{Stream, StreamExt, stream};
use hone_proto::{
    Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth, TokenUsage,
    ToolCallRequest,
};
use hone_providers::{CompletionRequest, ModelProvider, PricingTable, ProviderError};
use hone_server::{AppState, router};
use serde_json::json;
use tower::ServiceExt;

const MODEL: &str = "claude-sonnet-4-6";

struct Metered {
    /// First call asks for a write (which pauses under AskRisky); otherwise plain text.
    first_writes: bool,
    n: AtomicUsize,
}

#[async_trait]
impl ModelProvider for Metered {
    fn id(&self) -> &str {
        "metered"
    }
    fn display_name(&self) -> &str {
        "Metered"
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
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let i = self.n.fetch_add(1, Ordering::SeqCst);
        let usage = ProviderEvent::Usage(TokenUsage {
            input: 100,
            output: 50,
            cache_read: 0,
            cache_creation: 0,
        });
        let events = if i == 0 && self.first_writes {
            vec![
                usage,
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "w1".to_owned(),
                    name: "write_file".to_owned(),
                    arguments: json!({ "path": "never.txt", "content": "x" }),
                }),
                ProviderEvent::Done,
            ]
        } else {
            vec![
                ProviderEvent::Token {
                    text: "done".to_owned(),
                },
                usage,
                ProviderEvent::Done,
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

/// Cost of one 100-in / 50-out call, as the handler prices it.
fn expected_cost_per_call() -> f64 {
    let table = PricingTable::default_table();
    let prefixed = ModelId::new(format!("anthropic:{MODEL}"));
    let entry = table.get(&prefixed).cloned().unwrap_or(PricingEntry {
        model: ModelId::new(MODEL),
        input_per_million: 3.0,
        output_per_million: 15.0,
        cache_read_per_million: None,
        cache_creation_per_million: None,
    });
    entry.calculate_cost(100, 50, 0, 0)
}

fn run_request(extra: serde_json::Value) -> Request<Body> {
    let mut body = json!({ "prompt": "go", "model": MODEL });
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    Request::builder()
        .method("POST")
        .uri("/run")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn state_for(root: &std::path::Path, provider: Arc<Metered>) -> AppState {
    AppState::new(None)
        .with_provider_key(None)
        .with_workspace_root(root.to_path_buf())
        .with_mock_provider(provider)
}

#[tokio::test]
async fn test_r9b_run_when_client_disconnects_at_approval_prompt_should_still_record_spend() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let state = state_for(
        &root,
        Arc::new(Metered {
            first_writes: true,
            n: AtomicUsize::new(0),
        }),
    );
    let governance = Arc::clone(&state.governance);

    let resp = router(state)
        .oneshot(run_request(json!({
            "request_id": "r9b-disconnect",
            "approval_policy": { "AskRisky": { "always_allowed": [] } },
        })))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Read the SSE stream until the run is paused for approval, then hang up.
    let mut body = resp.into_body().into_data_stream();
    let mut seen = String::new();
    tokio::time::timeout(Duration::from_secs(15), async {
        while let Some(chunk) = body.next().await {
            seen.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
            if seen.contains("tool_call_paused") {
                return;
            }
        }
        panic!("stream ended before the approval pause: {seen}");
    })
    .await
    .expect("run never paused for approval");
    drop(body);

    // The spawned run is cancelled; its partial spend must still be counted.
    let usage = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let u = governance.get_usage().await;
            if u.total_tokens > 0 {
                return u;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("cancelled run's spend was never recorded in governance");

    assert_eq!(usage.total_tokens, 150);
    assert!((usage.total_cost_usd - expected_cost_per_call()).abs() < 1e-9);
    let m = usage.by_model.get(MODEL).expect("per-model entry");
    assert_eq!(m.tokens, 150);
}

#[tokio::test]
async fn test_r9b_run_when_completed_ok_should_record_spend_exactly_once() {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let state = state_for(
        &root,
        Arc::new(Metered {
            first_writes: false,
            n: AtomicUsize::new(0),
        }),
    );
    let governance = Arc::clone(&state.governance);

    let resp = router(state).oneshot(run_request(json!({}))).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    tokio::time::timeout(
        Duration::from_secs(15),
        axum::body::to_bytes(resp.into_body(), usize::MAX),
    )
    .await
    .expect("run did not finish")
    .unwrap();

    tokio::time::timeout(Duration::from_secs(10), async {
        while governance.get_usage().await.total_tokens == 0 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("completed run's spend was never recorded");
    // Give a duplicate recording time to show up.
    tokio::time::sleep(Duration::from_millis(300)).await;

    let usage = governance.get_usage().await;
    assert_eq!(usage.total_tokens, 150, "recorded exactly once");
    assert!((usage.total_cost_usd - expected_cost_per_call()).abs() < 1e-9);
}
