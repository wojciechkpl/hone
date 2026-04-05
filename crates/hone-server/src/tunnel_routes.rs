use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use crate::error::ServerError;
use crate::state::AppState;
use crate::tunnel::TunnelConfig;

pub fn tunnel_router() -> Router<AppState> {
    Router::new().route(
        "/tunnel",
        post(start_tunnel).delete(stop_tunnel).get(tunnel_status),
    )
}

#[derive(Debug, Deserialize)]
struct StartTunnelRequest {
    local_port: u16,
}

async fn start_tunnel(
    State(state): State<AppState>,
    Json(body): Json<StartTunnelRequest>,
) -> Result<impl IntoResponse, ServerError> {
    let config = TunnelConfig {
        local_port: body.local_port,
        auth_token: state.auth_token.clone().unwrap_or_default(),
    };
    let info = state.tunnel_manager.start(&config).await?;
    Ok((StatusCode::CREATED, Json(info)))
}

async fn stop_tunnel(State(state): State<AppState>) -> Result<impl IntoResponse, ServerError> {
    state.tunnel_manager.stop().await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn tunnel_status(State(state): State<AppState>) -> Result<impl IntoResponse, ServerError> {
    match state.tunnel_manager.status().await? {
        Some(info) => Ok(Json(json!(info))),
        None => Ok(Json(json!({ "is_active": false }))),
    }
}
