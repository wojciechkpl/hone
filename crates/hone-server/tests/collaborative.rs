use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{AppState, Participant, ParticipantRole, router};
use tower::ServiceExt;

fn make_state() -> AppState {
    AppState::new(None)
}

fn join_body(user_id: &str, name: &str) -> Body {
    Body::from(serde_json::json!({ "user_id": user_id, "name": name }).to_string())
}

fn leave_body(user_id: &str) -> Body {
    Body::from(serde_json::json!({ "user_id": user_id }).to_string())
}

fn kick_body(owner_user_id: &str, target_user_id: &str) -> Body {
    Body::from(
        serde_json::json!({
            "owner_user_id": owner_user_id,
            "target_user_id": target_user_id
        })
        .to_string(),
    )
}

async fn read_json(body: axum::body::Body) -> serde_json::Value {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_join_session_adds_participant() {
    let state = make_state();
    let session_id = "sess-1";

    // Join
    let join_resp = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/join"))
                .header("content-type", "application/json")
                .body(join_body("user-1", "Alice"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(join_resp.status(), StatusCode::OK);

    // List
    let list_resp = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/{session_id}/participants"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(list_resp.status(), StatusCode::OK);
    let json = read_json(list_resp.into_body()).await;
    let participants: Vec<Participant> = serde_json::from_value(json).unwrap();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0].user_id, "user-1");
}

#[tokio::test]
async fn test_first_joiner_is_owner() {
    let state = make_state();
    let session_id = "sess-2";

    let join_resp = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/join"))
                .header("content-type", "application/json")
                .body(join_body("user-1", "Alice"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(join_resp.status(), StatusCode::OK);
    let json = read_json(join_resp.into_body()).await;
    let participant: Participant = serde_json::from_value(json).unwrap();
    assert_eq!(participant.role, ParticipantRole::Owner);
}

#[tokio::test]
async fn test_second_joiner_is_editor() {
    let state = make_state();
    let session_id = "sess-3";

    // First joiner — Owner
    router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/join"))
                .header("content-type", "application/json")
                .body(join_body("user-1", "Alice"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Second joiner — should be Editor
    let second_resp = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/join"))
                .header("content-type", "application/json")
                .body(join_body("user-2", "Bob"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(second_resp.status(), StatusCode::OK);
    let json = read_json(second_resp.into_body()).await;
    let participant: Participant = serde_json::from_value(json).unwrap();
    assert_eq!(participant.role, ParticipantRole::Editor);

    // List and verify both roles
    let list_resp = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/{session_id}/participants"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let json = read_json(list_resp.into_body()).await;
    let participants: Vec<Participant> = serde_json::from_value(json).unwrap();
    assert_eq!(participants.len(), 2);

    let owner = participants.iter().find(|p| p.user_id == "user-1").unwrap();
    let editor = participants.iter().find(|p| p.user_id == "user-2").unwrap();
    assert_eq!(owner.role, ParticipantRole::Owner);
    assert_eq!(editor.role, ParticipantRole::Editor);
}

#[tokio::test]
async fn test_leave_session_removes_participant() {
    let state = make_state();
    let session_id = "sess-4";

    // Join
    router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/join"))
                .header("content-type", "application/json")
                .body(join_body("user-1", "Alice"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Leave
    let leave_resp = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/leave"))
                .header("content-type", "application/json")
                .body(leave_body("user-1"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(leave_resp.status(), StatusCode::NO_CONTENT);

    // List — should be empty
    let list_resp = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/{session_id}/participants"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let json = read_json(list_resp.into_body()).await;
    let participants: Vec<Participant> = serde_json::from_value(json).unwrap();
    assert!(participants.is_empty());
}

#[tokio::test]
async fn test_kick_by_owner_succeeds() {
    let state = make_state();
    let session_id = "sess-5";

    // user-1 joins (Owner)
    router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/join"))
                .header("content-type", "application/json")
                .body(join_body("user-1", "Alice"))
                .unwrap(),
        )
        .await
        .unwrap();

    // user-2 joins (Editor)
    router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/join"))
                .header("content-type", "application/json")
                .body(join_body("user-2", "Bob"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Owner kicks user-2
    let kick_resp = router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/kick"))
                .header("content-type", "application/json")
                .body(kick_body("user-1", "user-2"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(kick_resp.status(), StatusCode::NO_CONTENT);

    // Only user-1 remains
    let list_resp = router(state)
        .oneshot(
            Request::builder()
                .uri(format!("/sessions/{session_id}/participants"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let json = read_json(list_resp.into_body()).await;
    let participants: Vec<Participant> = serde_json::from_value(json).unwrap();
    assert_eq!(participants.len(), 1);
    assert_eq!(participants[0].user_id, "user-1");
}

#[tokio::test]
async fn test_kick_by_non_owner_returns_403() {
    let state = make_state();
    let session_id = "sess-6";

    // user-1 joins (Owner)
    router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/join"))
                .header("content-type", "application/json")
                .body(join_body("user-1", "Alice"))
                .unwrap(),
        )
        .await
        .unwrap();

    // user-2 joins (Editor)
    router(state.clone())
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/join"))
                .header("content-type", "application/json")
                .body(join_body("user-2", "Bob"))
                .unwrap(),
        )
        .await
        .unwrap();

    // user-2 (Editor) tries to kick user-1 — should be 403
    let kick_resp = router(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/kick"))
                .header("content-type", "application/json")
                .body(kick_body("user-2", "user-1"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(kick_resp.status(), StatusCode::FORBIDDEN);
}
