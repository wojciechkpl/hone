use std::collections::HashMap;

use axum::body::HttpBody as _;
use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use subtle::{Choice, ConstantTimeEq};
use tower_http::limit::RequestBodyLimitLayer;

use crate::collaborative_routes::collaborative_router;
use crate::distributed_routes::distributed_router;
use crate::error::ServerError;
use crate::discovery::{
    agent_detail_handler, list_agents_handler, list_recipes_handler, recipe_detail_handler,
};
use crate::eval_routes::{eval_detail_handler, list_evals_handler};
use crate::mcp_routes::{installed_handler, marketplace_handler};
use crate::memory_routes::{list_memory_handler, memory_stats_handler};
use crate::provenance_routes::{
    list_logs_handler, log_entries_handler, log_verify_handler,
};
use crate::sandbox_routes::sandbox_policies_handler;
use crate::execution::run_handler;
use crate::orchestration::orchestrate_handler;
use crate::run_registry::StepResult;
use crate::governance_routes::governance_router;
use crate::rate_limit::rate_limit_middleware;
use crate::session::SessionInfo;
use crate::state::AppState;
use crate::tunnel_routes::tunnel_router;
use crate::web::serve_ui;
use crate::webui::{serve_asset as serve_webui_asset, serve_index as serve_webui_index};

pub fn router(state: AppState) -> Router {
    // Public sub-router: endpoints that MUST be reachable without an
    // Authorization header so that liveness probes from load balancers /
    // Kubernetes / ECS can poll them.  These are also the diagnostics that
    // belong in deployment dashboards (open API spec) — see AUDIT-MAJOR-7.
    //
    // Built without the `auth_middleware` layer.  Rate limiting still
    // applies (it is layered on the merged router below), so a malicious
    // caller cannot brute-force these endpoints into a DoS vector.
    let public = Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi_json))
        // Web UI static assets — public so the SPA can load before
        // the user authenticates. The actual API endpoints the SPA
        // calls remain auth-gated, so this is safe.
        .route("/ui", get(serve_webui_index))
        .route("/ui/", get(serve_webui_index))
        .route("/ui/{*path}", get(serve_webui_asset));

    // Protected sub-router: everything that mutates server state or reveals
    // session/governance data.  Behind `auth_middleware` when an
    // `auth_token` is configured on `AppState`.
    let protected = Router::new()
        .route("/", get(serve_ui))
        .route("/run", post(run_handler))
        .route("/run/{request_id}/step", post(step_handler))
        .route("/orchestrate", post(orchestrate_handler))
        .route("/agents", get(list_agents_handler))
        .route("/agents/{name}", get(agent_detail_handler))
        .route("/recipes", get(list_recipes_handler))
        .route("/recipes/{name}", get(recipe_detail_handler))
        .route("/mcp/marketplace", get(marketplace_handler))
        .route("/mcp/installed", get(installed_handler))
        .route("/sandbox/policies", get(sandbox_policies_handler))
        .route("/memory", get(list_memory_handler))
        .route("/memory/stats", get(memory_stats_handler))
        .route("/provenance", get(list_logs_handler))
        .route("/provenance/{id}", get(log_entries_handler))
        .route("/provenance/{id}/verify", get(log_verify_handler))
        .route("/evals", get(list_evals_handler))
        .route("/evals/{id}", get(eval_detail_handler))
        .route("/sessions", post(create_session))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/gc", delete(gc_sessions))
        .merge(tunnel_router())
        .merge(governance_router())
        .merge(collaborative_router())
        .merge(distributed_router())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // Merge the public router onto the auth-layered one.  Axum's `.merge()`
    // composes routes from both routers; layers attached to a router only
    // apply to its OWN routes, so `/health` and `/openapi.json` remain
    // un-authed even though they share the final stack with the protected
    // routes.  Rate limiting is applied to the merged result so that BOTH
    // public and protected endpoints share the same throttle bucket.
    protected
        .merge(public)
        // Rate limiting runs before auth so that brute-force attempts are also throttled.
        // The generous default (100 req/min) means existing tests are never affected.
        .layer(middleware::from_fn_with_state(
            state.rate_limiter.clone(),
            rate_limit_middleware,
        ))
        // Two-layer body-size cap, applied outermost so that a giant
        // unauthenticated body is rejected before any handler logic runs.
        // Layers compose right-to-left; the last `.layer()` call runs first
        // from the request's perspective.  AUDIT-P1 #3.
        //
        // Layer A — `body_size_hint_check`: cheap, synchronous.  Covers two
        // fast-path cases without reading a single byte:
        //   1. `Content-Length` header present and over limit.
        //   2. Body size-hint upper bound known and over limit (in-memory
        //      bodies such as those used in integration tests that carry no
        //      `Content-Length` header, e.g. `Body::from(Vec<u8>)`).
        //
        // Layer B — `RequestBodyLimitLayer`: wraps the body stream.  Covers
        // chunked / streaming bodies where neither check above fires.  The
        // stream enforces the limit as bytes are polled by extractors or
        // handlers that read the body.
        //
        // Together the two layers close the hole the original implementation
        // had: chunked streams with no `Content-Length` and no size hint are
        // now capped by Layer B.  See `HONE_MAX_BODY_BYTES` in `lib.rs`.
        .layer(RequestBodyLimitLayer::new(crate::HONE_MAX_BODY_BYTES))
        .layer(middleware::from_fn(body_size_hint_check))
        .with_state(state)
}

/// Cheap synchronous body-size guard.  Rejects with `413` when either
/// the `Content-Length` request header or the body's `size_hint().upper()`
/// bound exceeds [`crate::HONE_MAX_BODY_BYTES`] — without reading a byte.
///
/// `Content-Length` covers well-behaved HTTP clients.  `size_hint().upper()`
/// covers in-memory bodies (e.g. `Body::from(Vec<u8>)`) used in integration
/// tests that send no header.
///
/// Chunked / streaming bodies that provide neither (the reviewer's original
/// concern) are handled by the surrounding `RequestBodyLimitLayer` which wraps
/// the stream and enforces the cap as bytes are consumed.
async fn body_size_hint_check(request: Request, next: Next) -> axum::response::Response {
    let limit = crate::HONE_MAX_BODY_BYTES as u64;

    if let Some(content_length) = request
        .headers()
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
    {
        if content_length > limit {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
    }

    if let Some(upper) = request.body().size_hint().upper() {
        if upper > limit {
            return StatusCode::PAYLOAD_TOO_LARGE.into_response();
        }
    }

    next.run(request).await
}

async fn auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> axum::response::Response {
    if state.auth_token.is_some() || !state.admin_tokens.is_empty() {
        // Extract bearer token manually from request parts.
        let token = request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::to_owned);

        let is_valid = match &token {
            Some(t) => {
                // Accept the token if it matches the shared auth_token OR if it
                // is a member of the admin_tokens set (admins are also authenticated).
                let matches_auth = state
                    .auth_token
                    .as_deref()
                    .map(|expected| tokens_match(t, expected))
                    .unwrap_or(false);
                // Constant-time membership check: always iterates every element
                // so that the set size (not a match position) determines wall
                // time, closing the short-circuit timing oracle.
                let t_bytes = t.as_bytes();
                let admin_result: Choice = state
                    .admin_tokens
                    .iter()
                    .map(|admin| admin.as_bytes().ct_eq(t_bytes))
                    .fold(Choice::from(0u8), |acc, c| acc | c);
                let is_admin = bool::from(admin_result);
                matches_auth || is_admin
            }
            None => false,
        };

        if is_valid {
            next.run(request).await
        } else {
            ServerError::Unauthorized.into_response()
        }
    } else {
        next.run(request).await
    }
}

/// Constant-time bearer-token comparison.
///
/// Replaces the naive `&t == expected`, which short-circuits at the first
/// mismatching byte and leaks per-byte progress through response timing.
/// `subtle::ConstantTimeEq::ct_eq` over `[u8]` returns `Choice::from(0)`
/// immediately for length mismatches (this leaks only the length of the
/// expected token — a server-side configuration value, not the secret
/// material itself) and otherwise compares all bytes in constant time.
fn tokens_match(presented: &str, expected: &str) -> bool {
    presented.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8() == 1
}

#[utoipa::path(
    get, path = "/health",
    tag = "system",
    responses((status = 200, description = "Service is healthy", body = serde_json::Value))
)]
pub(crate) async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// `POST /run/{request_id}/step`
///
/// Routes a `StepAction` from the client back to the in-flight agent
/// identified by `request_id`. The agent must have been started in
/// step-mode (`approval_policy != Auto` AND `request_id` supplied).
///
/// Status codes:
/// - 202 Accepted — the action was forwarded to the agent.
/// - 404 Not Found — no run with this id is registered (either it never
///   started in step-mode, has already finished, or the id is wrong).
/// - 410 Gone — the run was registered but the agent's receiver has
///   already been dropped (rare race with shutdown).
/// - 400 Bad Request — body did not deserialise into `StepAction`.
pub(crate) async fn step_handler(
    State(state): State<AppState>,
    Path(request_id): Path<String>,
    Json(action): Json<hone_proto::StepAction>,
) -> impl IntoResponse {
    match state.run_registry.step(&request_id, action) {
        StepResult::Accepted => (
            StatusCode::ACCEPTED,
            Json(json!({ "status": "accepted", "request_id": request_id })),
        )
            .into_response(),
        StepResult::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "status": "not_found", "request_id": request_id })),
        )
            .into_response(),
        StepResult::Closed => (
            StatusCode::GONE,
            Json(json!({ "status": "gone", "request_id": request_id })),
        )
            .into_response(),
    }
}

#[utoipa::path(
    post, path = "/sessions",
    tag = "sessions",
    responses(
        (status = 201, description = "Session created", body = serde_json::Value),
        (status = 401, description = "Unauthorized"),
    )
)]
pub(crate) async fn create_session(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ServerError> {
    let info = state.sessions.create_new().await?;
    let body = Json(json!({ "session_id": info.id.to_string() }));
    Ok((StatusCode::CREATED, body))
}

#[utoipa::path(
    get, path = "/sessions/{id}",
    tag = "sessions",
    params(("id" = String, Path, description = "Session ID (UUID)")),
    responses(
        (status = 200, description = "Session info", body = SessionInfo),
        (status = 404, description = "Session not found"),
    )
)]
pub(crate) async fn get_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    let info = state
        .sessions
        .get(&id)
        .await?
        .ok_or_else(|| ServerError::SessionNotFound { id: id.clone() })?;

    Ok(Json(info))
}

#[utoipa::path(
    delete, path = "/sessions/{id}",
    tag = "sessions",
    params(("id" = String, Path, description = "Session ID (UUID)")),
    responses(
        (status = 204, description = "Session deleted"),
        (status = 404, description = "Session not found"),
    )
)]
pub(crate) async fn delete_session(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    let deleted = state.sessions.delete(&id).await?;
    if deleted {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ServerError::SessionNotFound { id })
    }
}

#[derive(serde::Serialize, utoipa::ToSchema)]
pub struct GcResult {
    pub removed: u32,
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[utoipa::path(
    delete, path = "/sessions/gc",
    tag = "sessions",
    params(("older_than_days" = Option<u64>, Query, description = "Delete sessions older than N days (default: 30)")),
    responses(
        (status = 200, description = "GC completed", body = GcResult),
    )
)]
pub(crate) async fn gc_sessions(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<GcResult>, ServerError> {
    let days: u64 = params
        .get("older_than_days")
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);

    // `days.saturating_mul(86_400)` so that `days = u64::MAX` (or any other
    // adversarial value supplied via the query string) cannot panic in
    // debug builds nor silently wrap in release.  The follow-on
    // `saturating_sub` then clamps the cutoff to 0 — which the SQLite store
    // interprets as "delete nothing" (no row's `created_at` is older than
    // the epoch).  See AUDIT-MEDIUM-12 / `tests/sessions_gc_overflow.rs`.
    let cutoff = now_unix().saturating_sub(days.saturating_mul(86_400));
    let removed = state.sessions.gc(cutoff).await?;
    Ok(Json(GcResult { removed }))
}

async fn openapi_json() -> Json<serde_json::Value> {
    use utoipa::OpenApi as _;
    let spec = crate::openapi::ApiDoc::openapi();
    Json(serde_json::to_value(spec).unwrap_or_default())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tokens_match_tests {
    use super::tokens_match;

    #[test]
    fn exact_match_returns_true() {
        assert!(tokens_match("secret-token-abc", "secret-token-abc"));
    }

    #[test]
    fn mismatched_returns_false() {
        assert!(!tokens_match("secret-token-abc", "secret-token-xyz"));
    }

    #[test]
    fn presented_shorter_than_expected_returns_false() {
        // Length mismatch must NOT be treated as a prefix match (the naive
        // `==` would also reject this, but verifying the contract here guards
        // against future changes that pad/truncate before comparison).
        assert!(!tokens_match("secret", "secret-token"));
    }

    #[test]
    fn presented_longer_than_expected_returns_false() {
        assert!(!tokens_match("secret-token-extra", "secret-token"));
    }

    #[test]
    fn empty_presented_with_nonempty_expected_returns_false() {
        // The `evaluate_auth_gate` in `honed/main.rs` already filters empty
        // tokens out at startup so `expected` is never empty here, but a
        // defensive test pins the contract anyway.
        assert!(!tokens_match("", "real-secret"));
    }

    #[test]
    fn equal_length_with_first_byte_diff_returns_false() {
        // The whole point of switching to ct_eq: byte-position of the first
        // diff must not change observable behaviour.  We can't measure timing
        // in a unit test reliably, but we CAN pin that the two cases below
        // both return false (i.e. early-byte and late-byte diffs are not
        // distinguished by the boolean result).
        assert!(!tokens_match("Xsecret-token-abc", "Asecret-token-abc"));
        assert!(!tokens_match("secret-token-abX", "secret-token-abA"));
    }

    #[test]
    fn unicode_tokens_compared_byte_wise() {
        // ct_eq operates on bytes, so multi-byte UTF-8 sequences are compared
        // verbatim.  Two tokens that differ in a single multi-byte char must
        // still return false.
        assert!(tokens_match("café-token", "café-token"));
        assert!(!tokens_match("café-token", "cafe-token"));
    }
}
