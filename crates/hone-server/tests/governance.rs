use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{router, AppState, TeamMember, TeamPolicy};
use tower::ServiceExt;

fn make_state() -> AppState {
    AppState::new(None)
}

#[tokio::test]
async fn test_get_team_usage_returns_stats() {
    let app = router(make_state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/team/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["total_tokens"].is_number(),
        "expected total_tokens field"
    );
}

#[tokio::test]
async fn test_get_team_members_returns_list() {
    let state = make_state();

    // Pre-populate with 2 members.
    state
        .governance
        .add_member(TeamMember {
            id: "user-1".to_owned(),
            name: "Alice".to_owned(),
            role: "admin".to_owned(),
            joined_at: 1_700_000_000,
        })
        .await;
    state
        .governance
        .add_member(TeamMember {
            id: "user-2".to_owned(),
            name: "Bob".to_owned(),
            role: "member".to_owned(),
            joined_at: 1_700_000_001,
        })
        .await;

    let app = router(state);
    let response = app
        .oneshot(
            Request::builder()
                .uri("/team/members")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.is_array(), "expected JSON array");
    assert_eq!(json.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_get_team_policy_returns_current() {
    let app = router(make_state());
    let response = app
        .oneshot(
            Request::builder()
                .uri("/team/policy")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["allowed_models"].is_array(),
        "expected allowed_models field"
    );
}

#[tokio::test]
async fn test_post_team_policy_updates() {
    let state = make_state();

    let new_policy = TeamPolicy {
        allowed_models: vec!["claude-opus-4-5".to_owned()],
        spend_cap_usd: Some(100.0),
        approved_extensions: vec!["ext-a".to_owned()],
    };
    let body = serde_json::to_vec(&new_policy).unwrap();

    // POST the new policy.
    let post_response = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/team/policy")
                .header("Content-Type", "application/json")
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(post_response.status(), StatusCode::OK);

    // GET should now return the updated values.
    let get_response = router(state)
        .oneshot(
            Request::builder()
                .uri("/team/policy")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(get_response.status(), StatusCode::OK);

    let get_body = axum::body::to_bytes(get_response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&get_body).unwrap();
    let models = json["allowed_models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0], "claude-opus-4-5");
    assert_eq!(json["spend_cap_usd"], 100.0);
}

#[tokio::test]
async fn test_governance_endpoints_require_auth() {
    let state = AppState::new(Some("secret".to_owned()));
    let app = router(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/team/usage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
