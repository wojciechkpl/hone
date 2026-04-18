//! End-to-end session lifecycle test.
//!
//! Boots a real `axum::Router` on a random ephemeral port via
//! `tokio::net::TcpListener::bind("127.0.0.1:0")`, then drives the complete
//! session lifecycle with `reqwest`:
//!
//!   POST /sessions
//!   → GET  /sessions/{id}
//!   → DELETE /sessions/{id}
//!   → GET  /sessions/{id}  (must now return 404)
//!
//! This gives us true HTTP stack coverage (routing, middleware, serialization)
//! without requiring an external process.

use hone_server::{AppState, router};
use reqwest::StatusCode;
use tokio::net::TcpListener;

/// Bind a real TCP socket on an OS-assigned port and return
/// `(listener, "http://127.0.0.1:<port>")`.
async fn bind_random_port() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind random port");
    let addr = listener.local_addr().expect("local_addr");
    let base_url = format!("http://127.0.0.1:{}", addr.port());
    (listener, base_url)
}

/// Spawn the server in the background and return the base URL.
async fn start_server() -> String {
    let (listener, base_url) = bind_random_port().await;
    let state = AppState::new(None);
    let app = router(state);

    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("server error");
    });

    base_url
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Full session lifecycle: POST → GET → DELETE → GET (404).
#[tokio::test]
async fn test_e2e_session_create_get_delete() {
    let base = start_server().await;
    let client = reqwest::Client::new();

    // 1. Create a session.
    let create_resp = client
        .post(format!("{base}/sessions"))
        .send()
        .await
        .expect("POST /sessions");

    assert_eq!(
        create_resp.status(),
        StatusCode::CREATED,
        "expected 201 CREATED"
    );

    let body: serde_json::Value = create_resp.json().await.expect("parse body");
    let session_id = body["session_id"]
        .as_str()
        .expect("session_id must be a string")
        .to_owned();

    assert!(!session_id.is_empty(), "session_id must not be empty");

    // 2. GET the session — must exist.
    let get_resp = client
        .get(format!("{base}/sessions/{session_id}"))
        .send()
        .await
        .expect("GET /sessions/{id}");

    assert_eq!(
        get_resp.status(),
        StatusCode::OK,
        "session should exist after creation"
    );

    let session_body: serde_json::Value = get_resp.json().await.expect("parse session body");
    assert_eq!(
        session_body["id"].as_str().unwrap_or(""),
        session_id,
        "returned id should match the created session"
    );

    // 3. DELETE the session.
    let delete_resp = client
        .delete(format!("{base}/sessions/{session_id}"))
        .send()
        .await
        .expect("DELETE /sessions/{id}");

    assert_eq!(
        delete_resp.status(),
        StatusCode::NO_CONTENT,
        "expected 204 NO_CONTENT on delete"
    );

    // 4. GET after DELETE — must return 404.
    let get_after_delete = client
        .get(format!("{base}/sessions/{session_id}"))
        .send()
        .await
        .expect("GET /sessions/{id} after delete");

    assert_eq!(
        get_after_delete.status(),
        StatusCode::NOT_FOUND,
        "session should return 404 after deletion"
    );
}

/// Deleting a non-existent session returns 404.
#[tokio::test]
async fn test_e2e_delete_nonexistent_session() {
    let base = start_server().await;
    let client = reqwest::Client::new();

    let fake_id = "00000000-0000-7000-8000-000000000000";
    let resp = client
        .delete(format!("{base}/sessions/{fake_id}"))
        .send()
        .await
        .expect("DELETE non-existent session");

    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "deleting a non-existent session must return 404"
    );
}

/// Multiple sessions can be created and are independent.
#[tokio::test]
async fn test_e2e_multiple_sessions_are_independent() {
    let base = start_server().await;
    let client = reqwest::Client::new();

    // Create two sessions.
    let id_a = {
        let resp = client
            .post(format!("{base}/sessions"))
            .send()
            .await
            .expect("POST session A");
        let body: serde_json::Value = resp.json().await.expect("parse A");
        body["session_id"].as_str().unwrap().to_owned()
    };

    let id_b = {
        let resp = client
            .post(format!("{base}/sessions"))
            .send()
            .await
            .expect("POST session B");
        let body: serde_json::Value = resp.json().await.expect("parse B");
        body["session_id"].as_str().unwrap().to_owned()
    };

    assert_ne!(id_a, id_b, "two sessions must have distinct IDs");

    // Delete A; B should still exist.
    client
        .delete(format!("{base}/sessions/{id_a}"))
        .send()
        .await
        .expect("delete A");

    let b_status = client
        .get(format!("{base}/sessions/{id_b}"))
        .send()
        .await
        .expect("GET B after deleting A")
        .status();

    assert_eq!(
        b_status,
        StatusCode::OK,
        "session B should still exist after deleting session A"
    );
}
