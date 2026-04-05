use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;

use crate::error::ServerError;
use crate::state::AppState;

pub fn collaborative_router() -> Router<AppState> {
    Router::new()
        .route("/sessions/{id}/join", post(join_session))
        .route("/sessions/{id}/leave", post(leave_session))
        .route("/sessions/{id}/participants", get(list_participants))
        .route("/sessions/{id}/kick", post(kick_participant))
}

#[derive(Debug, Deserialize)]
struct JoinRequest {
    user_id: String,
    name: String,
}

#[derive(Debug, Deserialize)]
struct LeaveRequest {
    user_id: String,
}

#[derive(Debug, Deserialize)]
struct KickRequest {
    owner_user_id: String,
    target_user_id: String,
}

async fn join_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<JoinRequest>,
) -> Result<impl IntoResponse, ServerError> {
    let participant = state
        .collaborative
        .join(&session_id, body.user_id, body.name)
        .await?;
    Ok((StatusCode::OK, Json(participant)))
}

async fn leave_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<LeaveRequest>,
) -> Result<impl IntoResponse, ServerError> {
    state
        .collaborative
        .leave(&session_id, &body.user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_participants(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> impl IntoResponse {
    let participants = state.collaborative.list_participants(&session_id).await;
    Json(participants)
}

async fn kick_participant(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(body): Json<KickRequest>,
) -> Result<impl IntoResponse, ServerError> {
    state
        .collaborative
        .kick(&session_id, &body.owner_user_id, &body.target_user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
