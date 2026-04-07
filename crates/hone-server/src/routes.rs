use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::collaborative_routes::collaborative_router;
use crate::error::ServerError;
use crate::governance_routes::governance_router;
use crate::state::AppState;
use crate::tunnel_routes::tunnel_router;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/sessions", post(create_session))
        .route("/sessions/{id}", get(get_session).delete(delete_session))
        .route("/health", get(health))
        .merge(tunnel_router())
        .merge(governance_router())
        .merge(collaborative_router())
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

async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

async fn create_session(
    State(state): State<AppState>,
) -> Result<impl IntoResponse, ServerError> {
    let info = state.sessions.create_new().await?;
    let body = Json(json!({ "session_id": info.id.to_string() }));
    Ok((StatusCode::CREATED, body))
}

async fn get_session(
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

async fn delete_session(
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
