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
        };

        let body = axum::Json(json!({ "error": message }));
        (status, body).into_response()
    }
}
