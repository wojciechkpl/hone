use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use crate::governance::TeamPolicy;
use crate::state::AppState;

pub fn governance_router() -> Router<AppState> {
    Router::new()
        .route("/team/usage", get(get_usage))
        .route("/team/members", get(get_members))
        .route("/team/policy", get(get_policy).post(set_policy))
}

async fn get_usage(State(state): State<AppState>) -> Json<crate::governance::TeamUsage> {
    Json(state.governance.get_usage().await)
}

async fn get_members(State(state): State<AppState>) -> Json<Vec<crate::governance::TeamMember>> {
    Json(state.governance.get_members().await)
}

async fn get_policy(State(state): State<AppState>) -> Json<TeamPolicy> {
    Json(state.governance.get_policy().await)
}

async fn set_policy(
    State(state): State<AppState>,
    Json(policy): Json<TeamPolicy>,
) -> Json<TeamPolicy> {
    state.governance.set_policy(policy).await;
    Json(state.governance.get_policy().await)
}
