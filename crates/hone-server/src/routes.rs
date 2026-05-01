use std::collections::HashMap;

use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use subtle::ConstantTimeEq;

use crate::collaborative_routes::collaborative_router;
use crate::error::ServerError;
use crate::execution::run_handler;
use crate::governance_routes::governance_router;
use crate::rate_limit::rate_limit_middleware;
use crate::session::SessionInfo;
use crate::state::AppState;
use crate::tunnel_routes::tunnel_router;
use crate::web::serve_ui;

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
        .route("/openapi.json", get(openapi_json));

    // Protected sub-router: everything that mutates server state or reveals
    // session/governance data.  Behind `auth_middleware` when an
    // `auth_token` is configured on `AppState`.
    let protected = Router::new()
        .route("/", get(serve_ui))
        .route("/run", post(run_handler))
        .route("/sessions", post(create_session))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/gc", delete(gc_sessions))
        .merge(tunnel_router())
        .merge(governance_router())
        .merge(collaborative_router())
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
        .with_state(state)
}

async fn auth_middleware(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> axum::response::Response {
    if let Some(expected) = &state.auth_token {
        // Extract bearer token manually from request parts.
        let token = request
            .headers()
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::to_owned);

        match token {
            Some(t) if tokens_match(&t, expected) => next.run(request).await,
            _ => ServerError::Unauthorized.into_response(),
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
