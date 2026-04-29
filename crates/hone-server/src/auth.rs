use axum::extract::FromRequestParts;
use axum::http::StatusCode;
use axum::http::request::Parts;
use axum::response::{IntoResponse, Response};

/// Extractor for `Authorization: Bearer <token>` headers.
pub struct BearerAuth(pub String);

#[derive(Debug)]
pub struct BearerAuthError;

impl IntoResponse for BearerAuthError {
    fn into_response(self) -> Response {
        (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({ "error": "Unauthorized" })),
        )
            .into_response()
    }
}

impl<S> FromRequestParts<S> for BearerAuth
where
    S: Send + Sync,
{
    type Rejection = BearerAuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(BearerAuthError)?;

        let token = header
            .strip_prefix("Bearer ")
            .ok_or(BearerAuthError)?
            .to_owned();

        Ok(BearerAuth(token))
    }
}
