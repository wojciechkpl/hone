//! RED-PHASE regression tests for **AUDIT-P1 #5**: two related governance bugs.
//!
//! ## Bug A — Missing RBAC check on `POST /team/policy`
//!
//! `crates/hone-server/src/governance_routes.rs:27`: `set_policy` calls
//! `state.governance.set_policy(policy)` without any role check.  Any caller
//! with a valid bearer token (or no token when auth is disabled) can overwrite
//! the team spend cap and allowed-model list.
//!
//! ## Bug B — Dead `add_usage` wiring
//!
//! `GovernanceStore::add_usage` (defined in `governance.rs:84`) is never called
//! from `run_handler` or anywhere in production code.  `GET /team/usage` always
//! returns `{ "total_tokens": 0, "total_cost_usd": 0.0, "by_model": {} }` even
//! after a successful `/run` invocation.
//!
//! ---
//!
//! ## Public API the implementer MUST add (green phase)
//!
//! ### 1. `AppState::admin_tokens: HashSet<String>` (or equivalent)
//!
//! File: `crates/hone-server/src/state.rs`
//!
//! The current auth flow is binary: a request either passes or fails a single
//! shared `auth_token`.  There is no concept of role.  To enforce "only admins
//! may call `POST /team/policy`", `AppState` needs a way to distinguish admin
//! callers from regular callers at the token level.
//!
//! Minimum viable surface (all other implementations are acceptable too):
//!
//! ```rust
//! use std::collections::HashSet;
//!
//! pub struct AppState {
//!     // … existing fields …
//!     /// Tokens whose bearers have the "admin" role.
//!     /// Any token NOT in this set is treated as non-admin regardless of
//!     /// whether it passes the `auth_token` gate.
//!     pub admin_tokens: HashSet<String>,
//! }
//!
//! impl AppState {
//!     /// Builder helper used by tests and production config loaders.
//!     pub fn with_admin_tokens(mut self, tokens: HashSet<String>) -> Self {
//!         self.admin_tokens = tokens;
//!         self
//!     }
//! }
//! ```
//!
//! The `set_policy` handler must then extract the bearer token from the request
//! and reject with `403 FORBIDDEN` if the token is not in `admin_tokens`.
//! When `admin_tokens` is empty (the default), **no caller is admin** — this
//! matches the principle of least privilege.  Operators must explicitly opt in
//! by populating the set.
//!
//! ### 2. `run_handler` must call `governance.add_usage(model, tokens, cost)`
//!
//! File: `crates/hone-server/src/execution.rs`
//!
//! After receiving `AgentEvent::Done { usage, cost_usd }` from the SSE stream,
//! the handler (or the spawned task) must call:
//!
//! ```rust
//! state.governance.add_usage(
//!     model_id.as_str(),
//!     usage.total() as u64,
//!     cost_usd,
//! ).await;
//! ```
//!
//! Because `run_handler` currently spawns a `tokio::spawn` task that owns the
//! event channel receiver, the simplest fix is to accumulate usage inside that
//! task before it exits, then call `add_usage` once with the totals.
//!
//! ### 3. Mock provider seam on `AppState`
//!
//! File: `crates/hone-server/src/state.rs` + `execution.rs`
//!
//! To test the usage-tracking path without a real Anthropic API key the
//! implementer must allow a mock provider to be injected.  Minimum surface:
//!
//! ```rust
//! use std::sync::Arc;
//! use hone_core::ModelProvider; // or whichever trait execution.rs uses
//!
//! pub struct AppState {
//!     // … existing fields …
//!     /// Overrides the `AnthropicProvider` used by `run_handler` (test seam).
//!     /// `None` means "use `AnthropicProvider::new(provider_key)`" as today.
//!     pub mock_provider: Option<Arc<dyn ModelProvider>>,
//! }
//!
//! impl AppState {
//!     pub fn with_mock_provider(mut self, p: Arc<dyn ModelProvider>) -> Self {
//!         self.mock_provider = Some(p);
//!         self
//!     }
//! }
//! ```
//!
//! `run_handler` selects the provider with:
//!
//! ```rust
//! let provider: Arc<dyn ModelProvider> = state.mock_provider.clone()
//!     .unwrap_or_else(|| Arc::new(AnthropicProvider::new(key)));
//! ```
//!
//! For this test file the mock provider is a `MockProvider` defined below that
//! immediately emits `AgentEvent::Done` with non-zero usage.  If the provider
//! trait is not object-safe or is not the right abstraction, the implementer
//! may choose any equivalent seam (e.g. a handler-level callback, a channel
//! that injects events) — the requirement is that tests can drive a `/run`
//! request to completion without a live API key.
//!
//! ---
//!
//! ## Why these four tests fail today
//!
//! * `set_policy_requires_admin_role` — `set_policy` has no role check;
//!   returns `200 OK` instead of `403 FORBIDDEN`.
//! * `set_policy_succeeds_with_admin_role` — `admin_tokens` does not exist;
//!   test will not compile until the seam is added.
//! * `set_policy_returns_401_unauthenticated` — this one PASSES today (the
//!   existing `auth_middleware` already rejects unauthenticated requests).
//!   It is included as a guardian test to prevent regressions.
//! * `team_usage_reflects_run_invocation` — `add_usage` is never called;
//!   `GET /team/usage` always returns `total_tokens: 0`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_proto::TokenUsage;
use hone_providers::{MockProvider, ModelProvider};
use hone_server::{AppState, TeamPolicy, router};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build an app where:
///   - `auth_token` = `Some("any-token")` so the auth gate is active.
///   - `admin_tokens` is empty (default) — no bearer is an admin.
///
/// Used by the non-admin tests.
fn app_with_auth_no_admin() -> axum::Router {
    let state = AppState::new(Some("any-token".to_owned()));
    router(state)
}

/// Build an app where:
///   - `auth_token` = `Some("any-token")` (gate active).
///   - `admin_tokens` = `{"admin-token"}` so only `admin-token` is admin.
///
/// Used by the admin-success test.
///
/// COMPILE GATE: `AppState::with_admin_tokens` does not exist yet.
/// The green-phase implementer must add it (see file header).
fn app_with_auth_and_admin() -> axum::Router {
    use std::collections::HashSet;
    let admin_set: HashSet<String> = ["admin-token".to_owned()].into_iter().collect();
    let state = AppState::new(Some("any-token".to_owned()))
        // NOTE: `with_admin_tokens` must be added by the implementer.
        .with_admin_tokens(admin_set);
    router(state)
}

/// Minimal valid `TeamPolicy` body.
fn policy_body() -> Vec<u8> {
    serde_json::to_vec(&TeamPolicy {
        allowed_models: vec!["claude-opus-4-5".to_owned()],
        spend_cap_usd: Some(50.0),
        approved_extensions: vec![],
    })
    .unwrap()
}

/// Build a `POST /team/policy` request with an optional bearer token.
fn post_policy_request(bearer: Option<&str>) -> Request<Body> {
    let body = policy_body();
    let mut builder = Request::builder()
        .method("POST")
        .uri("/team/policy")
        .header("Content-Type", "application/json");

    if let Some(token) = bearer {
        builder = builder.header("Authorization", format!("Bearer {token}"));
    }

    builder.body(Body::from(body)).unwrap()
}

// ---------------------------------------------------------------------------
// Contract A — RBAC on `POST /team/policy`
// ---------------------------------------------------------------------------

/// AUDIT-P1 #5-A1: `POST /team/policy` with a non-admin bearer token must
/// return `403 FORBIDDEN`.
///
/// FAILS TODAY: `set_policy` has no role check and returns `200 OK` for any
/// authenticated request.
#[tokio::test]
async fn set_policy_requires_admin_role() {
    let app = app_with_auth_no_admin();

    let response = app
        .oneshot(post_policy_request(Some("any-token")))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "AUDIT-P1 #5: POST /team/policy must return 403 FORBIDDEN for a \
         non-admin bearer token. Currently returns {} — the route lacks any \
         RBAC check so any valid token can overwrite the team policy.",
        response.status()
    );
}

/// AUDIT-P1 #5-A2: `POST /team/policy` with a bearer token that IS in
/// `admin_tokens` must return `200 OK`.
///
/// FAILS TODAY: `AppState::with_admin_tokens` does not exist (compile error).
/// Once the seam is added the test will compile but may still fail if the
/// handler does not correctly look up the token in `admin_tokens`.
#[tokio::test]
async fn set_policy_succeeds_with_admin_role() {
    let app = app_with_auth_and_admin();

    let response = app
        .oneshot(post_policy_request(Some("admin-token")))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "AUDIT-P1 #5: POST /team/policy must return 200 OK for a bearer token \
         that is in admin_tokens. Got: {}",
        response.status()
    );
}

/// AUDIT-P1 #5-A3 (guardian): `POST /team/policy` with no Authorization header
/// must return `401 UNAUTHORIZED`.
///
/// This PASSES today via the existing `auth_middleware`.  It is included here
/// to prevent regressions if the RBAC refactor accidentally bypasses the auth
/// gate.
#[tokio::test]
async fn set_policy_returns_401_unauthenticated() {
    let app = app_with_auth_no_admin();

    let response = app.oneshot(post_policy_request(None)).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "AUDIT-P1 #5: POST /team/policy with no Authorization header must \
         return 401 UNAUTHORIZED. Got: {}",
        response.status()
    );
}

// ---------------------------------------------------------------------------
// Contract B — Usage tracking from `POST /run`
// ---------------------------------------------------------------------------

/// AUDIT-P1 #5-B: after a successful `POST /run`, `GET /team/usage` must
/// reflect non-zero `total_tokens` and `total_cost_usd`.
///
/// FAILS TODAY: `run_handler` never calls `GovernanceStore::add_usage`, so
/// `GET /team/usage` always returns `{ "total_tokens": 0, ... }`.
///
/// ## Compile gate
///
/// This test uses `AppState::with_mock_provider` which does not yet exist.
/// The green-phase implementer must add it (see file header).
///
/// ## What "mock provider" means here
///
/// The mock must respond to a single-turn request with an `AgentEvent::Done`
/// carrying non-zero usage and cost so that the `run_handler` (once fixed) has
/// something to record.  It must NOT require a live network connection.
///
/// The implementer may use any mechanism they wish: a struct that implements
/// `ModelProvider` (or whatever trait `AnthropicProvider` satisfies), a
/// channel-based stub, a callback closure, etc.  The contract is:
///
///   * The `/run` request returns `200 OK` (SSE stream).
///   * The SSE stream contains at least one `done` event with
///     `usage.total() > 0` and `cost_usd > 0`.
///   * After the SSE stream is fully consumed, `GET /team/usage` returns
///     `total_tokens > 0` and `total_cost_usd > 0`.
#[tokio::test]
async fn team_usage_reflects_run_invocation() {
    use std::collections::HashSet;

    // NOTE: `with_mock_provider` must be added by the implementer.
    // We build state with a well-known admin token so we can also POST a run
    // without triggering auth failures.
    let admin_set: HashSet<String> = ["run-token".to_owned()].into_iter().collect();

    // The mock provider is constructed by the green-phase implementer.
    // Here we document the *interface* the test expects: it must implement
    // whatever trait `execution.rs` uses for providers and must emit a Done
    // event with usage = TokenUsage { input: 10, output: 20, .. } and
    // cost_usd = 0.001 (or any non-zero values).
    //
    // Until `with_mock_provider` exists this line will not compile.
    let state = AppState::new(Some("run-token".to_owned()))
        .with_admin_tokens(admin_set)
        // `MockDoneProvider` is a placeholder name; the implementer chooses
        // the concrete type.  What matters is that it satisfies the provider
        // trait and completes a turn immediately with non-zero usage.
        .with_mock_provider(make_mock_done_provider());

    let app = router(state.clone());

    // Issue POST /run.
    let run_body = r#"{"prompt":"hello"}"#;
    let run_req = Request::builder()
        .method("POST")
        .uri("/run")
        .header("Content-Type", "application/json")
        .header("Authorization", "Bearer run-token")
        .body(Body::from(run_body))
        .unwrap();

    let run_response = app.clone().oneshot(run_req).await.unwrap();

    assert_eq!(
        run_response.status(),
        StatusCode::OK,
        "AUDIT-P1 #5: POST /run must return 200 OK (SSE) when mock provider is \
         configured. Got: {}",
        run_response.status()
    );

    // Drain the SSE body so that the spawned task (which calls add_usage) has
    // time to complete before we query /team/usage.
    let _body = axum::body::to_bytes(run_response.into_body(), usize::MAX)
        .await
        .unwrap();

    // Give the spawned task a moment to flush add_usage into the store.
    // We use a tiny sleep here because the task is fire-and-forget; a proper
    // implementation might use a oneshot ack channel, but a 50 ms pause is
    // sufficient for an in-process test.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Query usage on the SAME state (shared Arc<GovernanceStore>).
    let usage_req = Request::builder()
        .method("GET")
        .uri("/team/usage")
        .header("Authorization", "Bearer run-token")
        .body(Body::empty())
        .unwrap();

    let usage_response = router(state).oneshot(usage_req).await.unwrap();

    assert_eq!(
        usage_response.status(),
        StatusCode::OK,
        "GET /team/usage must return 200 OK"
    );

    let usage_body = axum::body::to_bytes(usage_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&usage_body).unwrap();

    let total_tokens = json["total_tokens"].as_u64().unwrap_or(0);
    let total_cost = json["total_cost_usd"].as_f64().unwrap_or(0.0);

    assert!(
        total_tokens > 0,
        "AUDIT-P1 #5: GET /team/usage must return total_tokens > 0 after a \
         successful /run. Got total_tokens = {total_tokens}. Bug: \
         GovernanceStore::add_usage is never called from run_handler."
    );

    assert!(
        total_cost > 0.0,
        "AUDIT-P1 #5: GET /team/usage must return total_cost_usd > 0 after a \
         successful /run. Got total_cost_usd = {total_cost}."
    );
}

// ---------------------------------------------------------------------------
// Placeholder for the mock provider the implementer must supply.
//
// This function will not compile until the implementer adds the mock provider
// type and the `with_mock_provider` seam.  It is intentionally left as a
// compile error to signal the required work clearly.
// ---------------------------------------------------------------------------

/// Returns a `MockProvider` that immediately responds with a single token
/// stream carrying `TokenUsage { input: 10, output: 20 }` and enough cost to
/// satisfy the `total_cost_usd > 0` assertion.
///
/// Uses the existing `MockProvider` from `hone-providers` configured via its
/// `with_usage` builder method.  The pricing entry on `AgentConfig` (default:
/// $3/$15 per million) converts 30 tokens into ~$0.00034 USD cost.
#[allow(dead_code)]
fn make_mock_done_provider() -> Arc<dyn ModelProvider> {
    let usage = TokenUsage {
        input: 10,
        output: 20,
        cache_read: 0,
        cache_creation: 0,
    };
    Arc::new(MockProvider::new(vec!["hello from mock".to_owned()]).with_usage(usage))
}
