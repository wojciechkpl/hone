//! A model id without a provider prefix is an Anthropic id (docs: honed model-prefix spec, R1-R3).
//!
//! `honed` installs a process-wide default price lookup keyed `anthropic:<id>`. This file
//! installs it the same way, so it must stay its own test binary (the lookup is global).
//!
//! Per model call the scripted provider reports 100 input + 50 output tokens:
//! Opus 4.7 = 100 x $5 + 50 x $25 per million = $0.00175; Sonnet 4.6 = $0.00105.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::pin::Pin;
use std::sync::{Arc, Once};
use std::time::Duration;

use async_trait::async_trait;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use futures::{Stream, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, TokenUsage};
use hone_providers::{CompletionRequest, MockProvider, ModelProvider, ProviderError};
use hone_server::{AppState, router};
use serde_json::{Value, json};
use tower::ServiceExt;

const OPUS_COST: f64 = 0.00175;
const SONNET_COST: f64 = 0.00105;
const TOLERANCE: f64 = 1e-9;

static INSTALL: Once = Once::new();

/// Install the price lookup exactly as `bins/honed/src/main.rs` does (no user config).
fn install_honed_price_lookup() {
    INSTALL.call_once(|| {
        let lookup = hone_server::build_price_lookup(None, None).expect("price lookup");
        hone_core::set_default_price_lookup(lookup);
    });
}

fn usage() -> TokenUsage {
    TokenUsage {
        input: 100,
        output: 50,
        cache_read: 0,
        cache_creation: 0,
    }
}

struct Scripted;

#[async_trait]
impl ModelProvider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }
    fn display_name(&self) -> &str {
        "Scripted"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }
    async fn complete(
        &self,
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::Token {
                text: "done".to_owned(),
            },
            ProviderEvent::Usage(usage()),
            ProviderEvent::Done,
        ])))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        5
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

fn post(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn sse_body(resp: axum::response::Response) -> String {
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = tokio::time::timeout(
        Duration::from_secs(15),
        to_bytes(resp.into_body(), 16 * 1024 * 1024),
    )
    .await
    .expect("run did not finish")
    .unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Data of the first SSE event named `name`.
fn event_data(body: &str, name: &str) -> Option<Value> {
    let mut lines = body.lines();
    while let Some(line) = lines.next() {
        if line == format!("event: {name}") {
            for next in lines.by_ref() {
                if let Some(d) = next.strip_prefix("data: ") {
                    return serde_json::from_str(d).ok();
                }
            }
        }
    }
    None
}

/// Run `POST /run` and return (recorded cost, recorded per-model keys, SSE body).
async fn run_and_read_spend(model: Option<&str>) -> (f64, Vec<String>, String) {
    install_honed_price_lookup();
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let state = AppState::new(None)
        .with_provider_key(None)
        .with_workspace_root(root)
        .with_mock_provider(Arc::new(Scripted));
    let governance = Arc::clone(&state.governance);

    let mut req = json!({ "prompt": "go" });
    if let Some(m) = model {
        req["model"] = json!(m);
    }
    let body = sse_body(router(state).oneshot(post("/run", req)).await.unwrap()).await;

    tokio::time::timeout(Duration::from_secs(10), async {
        while governance.get_usage().await.total_tokens == 0 {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("spend was never recorded");
    let u = governance.get_usage().await;
    assert_eq!(u.total_tokens, 150);
    let keys = u.by_model.keys().cloned().collect();
    (u.total_cost_usd, keys, body)
}

fn assert_priced(cost: f64, expected: f64, body: &str) {
    assert!(
        (cost - expected).abs() < TOLERANCE,
        "recorded spend {cost}, expected {expected}"
    );
    assert!(
        event_data(body, "unpriced_model").is_none(),
        "run reported an unpriced model:\n{body}"
    );
}

#[tokio::test]
async fn test_r2_run_when_model_unprefixed_opus_should_record_priced_spend() {
    let (cost, keys, body) = run_and_read_spend(Some("claude-opus-4-7")).await;
    assert_priced(cost, OPUS_COST, &body);
    // R1: recorded under the prefixed id.
    assert_eq!(keys, vec!["anthropic:claude-opus-4-7".to_owned()]);
}

#[tokio::test]
async fn test_r2_run_when_model_omitted_should_price_default_sonnet() {
    let (cost, keys, body) = run_and_read_spend(None).await;
    assert_priced(cost, SONNET_COST, &body);
    assert_eq!(keys, vec!["anthropic:claude-sonnet-4-6".to_owned()]);
}

#[tokio::test]
async fn test_r2_run_when_model_prefixed_opus_should_record_same_priced_spend() {
    let (cost, keys, body) = run_and_read_spend(Some("anthropic:claude-opus-4-7")).await;
    assert_priced(cost, OPUS_COST, &body);
    // R1: kept as given, no double prefix.
    assert_eq!(keys, vec!["anthropic:claude-opus-4-7".to_owned()]);
}

/// R3: `POST /orchestrate?dry_run=true` prices the lead's decompose call. The mock reports
/// 100 + 50 tokens, so `budget_progress.spent_usd` is the lead cost of one call.
async fn orchestrate_lead_cost(model: Option<&str>) -> (f64, String) {
    install_honed_price_lookup();
    let plan = r#"{"goal":"g","subtasks":[{"id":"1","description":"d","agent":"general-purpose","context":"","depends_on":[],"acceptance_tests":[],"budget_usd":null,"scope":[]}]}"#;
    let provider: Arc<dyn ModelProvider> =
        Arc::new(MockProvider::new(vec![plan.to_owned()]).with_usage(usage()));
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let state = AppState::new(None)
        .with_provider_key(None)
        .with_workspace_root(root)
        .with_mock_provider(provider);
    let mut req = json!({ "prompt": "go" });
    if let Some(m) = model {
        req["model"] = json!(m);
    }
    let body = sse_body(
        router(state)
            .oneshot(post("/orchestrate?dry_run=true", req))
            .await
            .unwrap(),
    )
    .await;
    let spent = event_data(&body, "budget_progress")
        .unwrap_or_else(|| panic!("no budget_progress in:\n{body}"))["spent_usd"]
        .as_f64()
        .expect("spent_usd");
    (spent, body)
}

#[tokio::test]
async fn test_r3_orchestrate_when_model_unprefixed_opus_should_price_lead_call() {
    let (spent, body) = orchestrate_lead_cost(Some("claude-opus-4-7")).await;
    assert_priced(spent, OPUS_COST, &body);
}

#[tokio::test]
async fn test_r3_orchestrate_when_model_omitted_should_price_default_sonnet() {
    let (spent, body) = orchestrate_lead_cost(None).await;
    assert_priced(spent, SONNET_COST, &body);
}

#[tokio::test]
async fn test_r3_orchestrate_when_model_prefixed_opus_should_price_lead_call() {
    let (spent, body) = orchestrate_lead_cost(Some("anthropic:claude-opus-4-7")).await;
    assert_priced(spent, OPUS_COST, &body);
}

#[test]
fn test_r4_model_id_when_prefixed_should_send_bare_name_on_the_wire() {
    assert_eq!(
        ModelId::new("anthropic:claude-opus-4-7").model_name(),
        "claude-opus-4-7"
    );
}
