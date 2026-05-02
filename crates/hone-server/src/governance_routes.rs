use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use subtle::{Choice, ConstantTimeEq};

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

/// RBAC-guarded policy update.
///
/// When `admin_tokens` is non-empty the caller must present a bearer token
/// that appears in that set (checked in constant time).  When `admin_tokens`
/// is empty no RBAC restriction is applied and any authenticated (or
/// unauthenticated, when auth is disabled) caller can update the policy —
/// preserving backwards-compatible behaviour for deployments that have not
/// opted into role-based access control.
async fn set_policy(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(policy): Json<TeamPolicy>,
) -> impl IntoResponse {
    // RBAC is only skipped entirely when the server is running without any
    // authentication gate (`auth_token` is `None`).  In that mode the
    // deployment is inherently trusted (e.g. local dev) and we preserve the
    // pre-RBAC behaviour.
    //
    // When an auth gate IS active (`auth_token` is `Some`), the caller must
    // prove admin membership: their token must appear in `admin_tokens`.
    // An empty `admin_tokens` with an active auth gate means no one is admin,
    // which matches the principle of least privilege.
    if state.auth_token.is_some() || !state.admin_tokens.is_empty() {
        // Extract the bearer token from the Authorization header.
        let presented = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");

        // Constant-time membership check: fold over ALL tokens regardless of
        // match position, preventing early-exit timing from revealing which
        // element (if any) matched.  The `|` of `Choice` values is itself
        // constant-time; `bool::from` collapses the result only at the end.
        let presented_bytes = presented.as_bytes();
        let result: Choice = state
            .admin_tokens
            .iter()
            .map(|t| t.as_bytes().ct_eq(presented_bytes))
            .fold(Choice::from(0u8), |acc, c| acc | c);
        let is_admin = bool::from(result);

        if !is_admin {
            return (
                StatusCode::FORBIDDEN,
                axum::Json(serde_json::json!({ "error": "Forbidden: admin role required" })),
            )
                .into_response();
        }
    }

    state.governance.set_policy(policy).await;
    Json(state.governance.get_policy().await).into_response()
}
