use std::collections::HashMap;

use axum::extract::{Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::collaborative_routes::collaborative_router;
use crate::error::ServerError;
use crate::governance_routes::governance_router;
use crate::session::SessionInfo;
use crate::rate_limit::rate_limit_middleware;
use crate::state::AppState;
use crate::tunnel_routes::tunnel_router;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/sessions/gc", delete(gc_sessions))
        .route("/health", get(health))
        .route("/openapi.json", get(openapi_json))
        .merge(tunnel_router())
        .merge(governance_router())
        .merge(collaborative_router())
        // Rate limiting runs before auth so that brute-force attempts are also throttled.
        // The generous default (100 req/min) means existing tests are never affected.
        .layer(middleware::from_fn_with_state(
            state.rate_limiter.clone(),
            rate_limit_middleware,
        ))
        .layer(middleware::from_fn_with_state(state.clone(), auth_middleware))
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
            Some(t) if &t == expected => next.run(request).await,
            _ => ServerError::Unauthorized.into_response(),
        }
    } else {
        next.run(request).await
    }
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

    let cutoff = now_unix().saturating_sub(days * 86_400);
    let removed = state.sessions.gc(cutoff).await?;
    Ok(Json(GcResult { removed }))
}

async fn openapi_json() -> Json<serde_json::Value> {
    use utoipa::OpenApi as _;
    let spec = crate::openapi::ApiDoc::openapi();
    Json(serde_json::to_value(spec).unwrap_or_default())
}
