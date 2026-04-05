use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("Session not found: {id}")]
    SessionNotFound { id: String },
    #[error("Unauthorized")]
    Unauthorized,
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("User {user_id} has already joined session {session_id}")]
    AlreadyJoined { session_id: String, user_id: String },
    #[error("User {user_id} is not a participant of session {session_id}")]
    NotParticipant { session_id: String, user_id: String },
    #[error("Forbidden: only the session owner can perform this action")]
    Forbidden,
}

impl IntoResponse for ServerError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ServerError::SessionNotFound { id } => (
                StatusCode::NOT_FOUND,
                format!("Session not found: {id}"),
            ),
            ServerError::Unauthorized => (StatusCode::UNAUTHORIZED, "Unauthorized".to_owned()),
            ServerError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Internal error: {msg}"),
            ),
            ServerError::AlreadyJoined { session_id, user_id } => (
                StatusCode::CONFLICT,
                format!("User {user_id} has already joined session {session_id}"),
            ),
            ServerError::NotParticipant { session_id, user_id } => (
                StatusCode::NOT_FOUND,
                format!("User {user_id} is not a participant of session {session_id}"),
            ),
            ServerError::Forbidden => (
                StatusCode::FORBIDDEN,
                "Forbidden: only the session owner can perform this action".to_owned(),
            ),
        };

        let body = axum::Json(json!({ "error": message }));
        (status, body).into_response()
    }
}
