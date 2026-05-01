//! RED-PHASE regression tests for **CRITICAL-1**: `POST /run` accepts arbitrary
//! client-controlled `working_dir`.
//!
//! Audit finding (a):
//!     `crates/hone-server/src/execution.rs:97-101, 115`
//!     The handler currently does `req.working_dir.map(PathBuf::from)` with
//!     no validation, and mounts a `NullSandbox` so the only per-call guard
//!     is the inner `resolve_within_working_dir` — which is trivially
//!     circumvented by choosing `"/"` as the root.
//!
//! Contract these tests pin:
//!     The server MUST reject any `working_dir` that escapes a server-side
//!     allowed root.  Implementation is free to choose the field name
//!     (`workspace_root`, `HONE_WORKSPACE_ROOT`, etc.) and the rejection
//!     mechanism (early 400, SSE error event, etc.) — but the response MUST
//!     NOT be a `200 OK` SSE stream that begins streaming agent events with
//!     the attacker-supplied root.
//!
//! These tests will FAIL against current `main` because:
//!     1. The handler accepts any `working_dir` and either streams an
//!        AgentEvent::Error (api_key missing) or starts a real agent run
//!        with the attacker-controlled root — both yielding `200 OK`.
//!
//! Implementer's freedom (green phase):
//!     - Add a `workspace_root: Option<PathBuf>` field to `AppState` (or
//!       similar) that defaults to `std::env::current_dir()`.
//!     - Validate `req.working_dir` against `workspace_root` BEFORE invoking
//!       the agent, and return `400 BAD REQUEST` on escape.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tower::ServiceExt;

/// Build an app whose `provider_key` is explicitly `None` so the test does
/// not depend on whatever `ANTHROPIC_API_KEY` happens to be set in the
/// developer's shell.  The validation we are testing must happen
/// independently of provider configuration.
fn app() -> axum::Router {
    let state = AppState::new(None).with_provider_key(None);
    router(state)
}

fn run_request(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/run")
        .header("content-type", "application/json")
        .body(Body::from(body.to_owned()))
        .unwrap()
}

#[tokio::test]
async fn run_rejects_absolute_working_dir_outside_workspace_root() {
    // `/etc` is the textbook example: outside any sane workspace root and
    // contains highly sensitive files (`/etc/shadow`, `/etc/passwd`).
    let body = r#"{"prompt":"read /etc/passwd","working_dir":"/etc"}"#;

    let response = app().oneshot(run_request(body)).await.unwrap();

    assert_ne!(
        response.status(),
        StatusCode::OK,
        "AUDIT-CRITICAL-1: /run must NOT return 200 for a working_dir that \
         escapes the configured workspace root.  Today the handler streams an \
         SSE response with the attacker-supplied root mounted under NullSandbox."
    );
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "AUDIT-CRITICAL-1: /run must reject out-of-root working_dir with 400 \
         BAD REQUEST.  Got status: {}",
        response.status()
    );
}

#[tokio::test]
async fn run_rejects_root_filesystem_as_working_dir() {
    // The most damaging form of the bug: `/` exposes everything readable by
    // the server process.
    let body = r#"{"prompt":"ls","working_dir":"/"}"#;

    let response = app().oneshot(run_request(body)).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "AUDIT-CRITICAL-1: /run must reject working_dir=\"/\".  Got: {}",
        response.status()
    );
}

#[tokio::test]
async fn run_rejects_dotdot_traversal_in_working_dir() {
    // A more subtle escape: `..` segments.  Any reasonable validation must
    // canonicalize before comparison.
    let body = r#"{"prompt":"x","working_dir":"./../../../../../etc"}"#;

    let response = app().oneshot(run_request(body)).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "AUDIT-CRITICAL-1: /run must canonicalize and reject .. traversal in \
         working_dir.  Got: {}",
        response.status()
    );
}

#[tokio::test]
async fn run_accepts_working_dir_inside_workspace_root() {
    // Positive contract: a working_dir equal to the server's CWD (the natural
    // default for workspace_root) MUST be accepted.  This guards against a
    // zealous fix that rejects every non-None working_dir.
    //
    // The handler will still emit an error event because no api_key is set,
    // but the HTTP status must be 200 (the SSE stream is well-formed).
    let cwd = std::env::current_dir().unwrap();
    let body = format!(
        r#"{{"prompt":"x","working_dir":"{}"}}"#,
        cwd.display().to_string().replace('\\', "\\\\")
    );

    let response = app().oneshot(run_request(&body)).await.unwrap();

    // Accept either 200 (SSE) or 500 (api_key missing surfaced as HTTP) —
    // what we MUST NOT see is a 400 rejection of a legitimate path.
    assert_ne!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "AUDIT-CRITICAL-1: a working_dir equal to the server's cwd must be \
         accepted; over-zealous validation rejected the legitimate path"
    );
}
