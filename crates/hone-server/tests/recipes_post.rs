//! Integration tests for `POST /recipes`.
//!
//! These are RED-phase (TDD) tests — they define the contract for the handler
//! that does not exist yet.  They are expected to FAIL TO COMPILE until the
//! rust-expert implements the handler and the `AppState::with_recipes_user_dir`
//! builder method.
//!
//! # Option B injection
//!
//! All tests use `AppState::new(None).with_recipes_user_dir(temp_path)` to
//! redirect writes away from the real `~/.config/hone/recipes/`.  The
//! implementer must add:
//!
//! - `pub recipes_user_dir: Option<PathBuf>` field on `AppState`.
//! - `pub fn with_recipes_user_dir(mut self, dir: PathBuf) -> Self` builder.
//! - The `POST /recipes` handler reads `state.recipes_user_dir` when `Some`,
//!   falling back to `dirs::config_dir().map(|d| d.join("hone").join("recipes"))`
//!   when `None`.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use serde_json::{Value, json};
use tempfile::TempDir;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Drain the response body and parse as a JSON `Value`.
async fn body_json(resp: axum::response::Response) -> Value {
    let bytes = to_bytes(resp.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).expect("body must be valid JSON")
}

/// Build a minimal valid POST body with one step.
fn minimal_body() -> Value {
    json!({
        "name": "review-pr",
        "steps": [
            { "name": "summarize", "prompt": "Summarise PR {{pr_id}}" }
        ]
    })
}

/// POST `body` to `/recipes` using the given `AppState` and return the response.
async fn post_recipe(state: AppState, body: Value) -> axum::response::Response {
    let app = router(state);
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/recipes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

/// Build an `AppState` that writes recipes into `dir`.
fn state_with_user_dir(dir: PathBuf) -> AppState {
    AppState::new(None).with_recipes_user_dir(dir)
}

// ---------------------------------------------------------------------------
// Test 1 — happy path: 201 + RecipeDetail body + file on disk
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_creates_file_and_returns_detail() {
    // Given
    let tmp = TempDir::new().unwrap();
    let user_dir = tmp.path().to_path_buf();
    let state = state_with_user_dir(user_dir.clone());

    let body = minimal_body(); // name = "review-pr", 1 step

    // When
    let resp = post_recipe(state, body).await;

    // Then — status
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Then — content-type is JSON
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        ct.starts_with("application/json"),
        "expected application/json content-type, got: {ct}",
    );

    // Then — body is a RecipeDetail
    let detail = body_json(resp).await;
    assert_eq!(
        detail.get("name").and_then(Value::as_str),
        Some("review-pr"),
        "name field must match",
    );
    assert_eq!(
        detail.get("source").and_then(Value::as_str),
        Some("user"),
        "source must be 'user'",
    );
    let steps = detail.get("steps").and_then(Value::as_array).unwrap();
    assert_eq!(steps.len(), 1, "one step must be returned");

    // Then — path field is present and non-empty
    let path_str = detail
        .get("path")
        .and_then(Value::as_str)
        .expect("path field must be present");
    assert!(!path_str.is_empty(), "path must be non-empty");

    // Then — file exists on disk under the user dir
    let expected_file = user_dir.join("review-pr.yaml");
    assert!(
        expected_file.exists(),
        "recipe file must exist at {expected_file:?}",
    );
}

// ---------------------------------------------------------------------------
// Test 2 — 409 when the recipe file already exists
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_returns_409_when_name_already_exists() {
    // Given — pre-create the file
    let tmp = TempDir::new().unwrap();
    let user_dir = tmp.path().to_path_buf();
    std::fs::create_dir_all(&user_dir).unwrap();
    std::fs::write(
        user_dir.join("review-pr.yaml"),
        b"name: review-pr\nsteps:\n  - name: s\n    prompt: p\n",
    )
    .unwrap();

    let state = state_with_user_dir(user_dir);

    // When — POST the same name
    let resp = post_recipe(state, minimal_body()).await;

    // Then
    assert_eq!(resp.status(), StatusCode::CONFLICT);
}

// ---------------------------------------------------------------------------
// Test 3 — 4xx for empty name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_rejects_empty_name() {
    let tmp = TempDir::new().unwrap();
    let state = state_with_user_dir(tmp.path().to_path_buf());

    let body = json!({
        "name": "",
        "steps": [{ "name": "s", "prompt": "p" }]
    });

    let resp = post_recipe(state, body).await;

    assert!(
        resp.status().is_client_error(),
        "empty name must yield 4xx, got {}",
        resp.status(),
    );
}

// ---------------------------------------------------------------------------
// Test 4 — 4xx for name containing a slash (path traversal)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_rejects_name_with_slash() {
    let tmp = TempDir::new().unwrap();
    let state = state_with_user_dir(tmp.path().to_path_buf());

    let body = json!({
        "name": "evil/path",
        "steps": [{ "name": "s", "prompt": "p" }]
    });

    let resp = post_recipe(state, body).await;

    assert!(
        resp.status().is_client_error(),
        "name with '/' must yield 4xx, got {}",
        resp.status(),
    );
}

// ---------------------------------------------------------------------------
// Test 5 — 4xx for name containing ".." (path traversal)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_rejects_name_with_dotdot() {
    let tmp = TempDir::new().unwrap();
    let state = state_with_user_dir(tmp.path().to_path_buf());

    let body = json!({
        "name": "../evil",
        "steps": [{ "name": "s", "prompt": "p" }]
    });

    let resp = post_recipe(state, body).await;

    assert!(
        resp.status().is_client_error(),
        "name with '..' must yield 4xx, got {}",
        resp.status(),
    );
}

// ---------------------------------------------------------------------------
// Test 6 — 4xx for name with characters outside [a-zA-Z0-9_-]
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_rejects_name_with_special_chars() {
    let tmp = TempDir::new().unwrap();
    let state = state_with_user_dir(tmp.path().to_path_buf());

    // Whitespace is not in the safe whitelist.
    let body = json!({
        "name": "foo bar",
        "steps": [{ "name": "s", "prompt": "p" }]
    });

    let resp = post_recipe(state, body).await;

    assert!(
        resp.status().is_client_error(),
        "name with whitespace must yield 4xx, got {}",
        resp.status(),
    );
}

// ---------------------------------------------------------------------------
// Test 7 — 4xx for empty steps array
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_rejects_empty_steps() {
    let tmp = TempDir::new().unwrap();
    let state = state_with_user_dir(tmp.path().to_path_buf());

    let body = json!({
        "name": "review-pr",
        "steps": []
    });

    let resp = post_recipe(state, body).await;

    assert!(
        resp.status().is_client_error(),
        "empty steps array must yield 4xx, got {}",
        resp.status(),
    );
}

// ---------------------------------------------------------------------------
// Test 8 — 4xx for a step with empty name
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_rejects_step_with_empty_name() {
    let tmp = TempDir::new().unwrap();
    let state = state_with_user_dir(tmp.path().to_path_buf());

    let body = json!({
        "name": "review-pr",
        "steps": [{ "name": "", "prompt": "do something" }]
    });

    let resp = post_recipe(state, body).await;

    assert!(
        resp.status().is_client_error(),
        "step with empty name must yield 4xx, got {}",
        resp.status(),
    );
}

// ---------------------------------------------------------------------------
// Test 9 — 4xx for a step with empty prompt
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_rejects_step_with_empty_prompt() {
    let tmp = TempDir::new().unwrap();
    let state = state_with_user_dir(tmp.path().to_path_buf());

    let body = json!({
        "name": "review-pr",
        "steps": [{ "name": "summarize", "prompt": "" }]
    });

    let resp = post_recipe(state, body).await;

    assert!(
        resp.status().is_client_error(),
        "step with empty prompt must yield 4xx, got {}",
        resp.status(),
    );
}

// ---------------------------------------------------------------------------
// Test 10 — round-trip: POST then GET /recipes and GET /recipes/{name}
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_round_trips_through_get() {
    // Given
    let tmp = TempDir::new().unwrap();
    let user_dir = tmp.path().to_path_buf();
    let state = state_with_user_dir(user_dir.clone());

    let post_body = json!({
        "name": "round-trip",
        "description": "A round-trip recipe",
        "steps": [
            { "name": "step-one", "prompt": "Do the thing" }
        ]
    });

    // When — POST
    let post_resp = post_recipe(state.clone(), post_body).await;
    assert_eq!(
        post_resp.status(),
        StatusCode::CREATED,
        "POST must succeed before we test round-trip GET",
    );

    // When — GET /recipes (list)
    let app_list = router(state.clone());
    let list_resp = app_list
        .oneshot(
            Request::builder()
                .uri("/recipes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(list_resp.status(), StatusCode::OK);
    let list_json = body_json(list_resp).await;
    let list = list_json.as_array().unwrap();

    // Then — the recipe appears in the list with source == "user"
    let found = list
        .iter()
        .find(|e| e.get("name").and_then(Value::as_str) == Some("round-trip"));
    assert!(found.is_some(), "POSTed recipe must appear in GET /recipes list");
    assert_eq!(
        found.unwrap().get("source").and_then(Value::as_str),
        Some("user"),
        "list entry must have source='user'",
    );

    // When — GET /recipes/{name}
    let app_detail = router(state);
    let detail_resp = app_detail
        .oneshot(
            Request::builder()
                .uri("/recipes/round-trip")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(detail_resp.status(), StatusCode::OK);
    let detail = body_json(detail_resp).await;

    // Then — all fields match what was POSTed
    assert_eq!(
        detail.get("name").and_then(Value::as_str),
        Some("round-trip"),
    );
    assert_eq!(
        detail.get("description").and_then(Value::as_str),
        Some("A round-trip recipe"),
    );
    assert_eq!(
        detail.get("source").and_then(Value::as_str),
        Some("user"),
    );
    let steps = detail.get("steps").and_then(Value::as_array).unwrap();
    assert_eq!(steps.len(), 1);
    assert_eq!(
        steps[0].get("name").and_then(Value::as_str),
        Some("step-one"),
    );
    assert_eq!(
        steps[0].get("prompt").and_then(Value::as_str),
        Some("Do the thing"),
    );
}

// ---------------------------------------------------------------------------
// Test 11 — optional test_cmd is persisted and returned by GET
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_recipes_persists_optional_test_cmd() {
    // Given
    let tmp = TempDir::new().unwrap();
    let user_dir = tmp.path().to_path_buf();
    let state = state_with_user_dir(user_dir.clone());

    let body = json!({
        "name": "with-test-cmd",
        "steps": [{ "name": "check", "prompt": "Run checks" }],
        "test_cmd": "cargo test"
    });

    // When — POST
    let post_resp = post_recipe(state.clone(), body).await;
    assert_eq!(post_resp.status(), StatusCode::CREATED);

    // POST response itself must contain test_cmd
    let post_detail = body_json(post_resp).await;
    assert_eq!(
        post_detail.get("test_cmd").and_then(Value::as_str),
        Some("cargo test"),
        "POST response must include test_cmd",
    );

    // When — GET /recipes/{name}
    let app = router(state);
    let get_resp = app
        .oneshot(
            Request::builder()
                .uri("/recipes/with-test-cmd")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(get_resp.status(), StatusCode::OK);
    let get_detail = body_json(get_resp).await;

    // Then — test_cmd is preserved
    assert_eq!(
        get_detail.get("test_cmd").and_then(Value::as_str),
        Some("cargo test"),
        "GET must return persisted test_cmd",
    );
}
