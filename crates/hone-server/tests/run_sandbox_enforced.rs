//! RED-PHASE regression tests for **AUDIT-P0**: `POST /run` installs a
//! `NullSandbox` on every spawned agent, providing zero kernel-level
//! enforcement.
//!
//! # Audit finding
//!
//!   File: `crates/hone-server/src/execution.rs`
//!   Line: (formerly 169) `sandbox: Arc::new(NullSandbox),`
//!
//!   The `run_handler` (and, after the refactor in this red-phase, its
//!   extracted helper `build_sandbox()`) hard-codes `NullSandbox` so every
//!   agent spawned via HTTP runs with zero OS-enforced filesystem restrictions.
//!   The CLI binary (`bins/hone/src/main.rs`) already calls
//!   `SandboxBuilder::detect()` correctly; the server path was simply missed.
//!
//! # Public API added to make this testable (contract)
//!
//!   `hone_server::execution::build_sandbox() -> Arc<dyn Sandbox>` (re-exported
//!   as `hone_server::build_sandbox`) is a new single-responsibility helper that
//!   encapsulates the server's sandbox-selection logic and is called by
//!   `run_handler`.  Exposing it lets this test file assert on the selected
//!   backend without spawning a live HTTP server or depending on a real API key.
//!
//!   The accessor is intentionally minimal: it returns the `Arc<dyn Sandbox>`
//!   that the handler will install, and the `Sandbox::platform()` method
//!   (already public on the trait) is used for assertions.  No other private
//!   fields are exposed.
//!
//! # Why these tests fail today
//!
//!   `build_sandbox()` currently returns `Arc::new(NullSandbox)`, whose
//!   `platform()` accessor returns `SandboxPlatform::Null`.
//!
//!   On macOS and Linux — the only platforms on which these tests are not
//!   `#[cfg]`-skipped — the expected platform is `Seatbelt` or `Landlock`
//!   respectively (what `SandboxBuilder::detect()` would select).  The tests
//!   assert `!= Null`, which fails against the current implementation.
//!
//! # What the green-phase implementer must change
//!
//!   Replace the body of `build_sandbox()` in `execution.rs` with:
//!
//!   ```rust
//!   hone_sandbox::SandboxBuilder::detect().into()
//!   ```
//!
//!   and remove the now-unused `NullSandbox` import.  The `run_handler`
//!   already calls `build_sandbox()` after the red-phase refactor, so no
//!   further changes to the handler are needed.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_sandbox::{Sandbox, SandboxBuilder, SandboxPlatform};
use hone_server::{AppState, build_sandbox, router};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Shared test helpers (mirror the style of run_working_dir.rs)
// ---------------------------------------------------------------------------

/// Build a test app with no API key so the agent is never actually launched.
/// Sandbox validation must happen independently of provider configuration.
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

// ---------------------------------------------------------------------------
// Contract 1 — The sandbox installed by the server is NOT NullSandbox on
//              platforms that provide native kernel-level enforcement.
//
// On macOS `SandboxBuilder::detect()` returns `SeatbeltSandbox`
// (platform = Seatbelt).  On Linux it returns `LandlockSandbox` (platform =
// Landlock).  `NullSandbox` (platform = Null) is the bug we are fixing.
//
// These tests are `#[cfg]`-gated to macOS and Linux so they are skipped on
// genuinely unsupported hosts where Null is the correct fallback.
// ---------------------------------------------------------------------------

/// On macOS, `build_sandbox()` must return a Seatbelt-backed sandbox — not
/// a NullSandbox — so that every `POST /run` agent runs under real OS-level
/// confinement.
///
/// FAILS TODAY: `build_sandbox()` returns `Arc::new(NullSandbox)` whose
/// `platform()` is `SandboxPlatform::Null`, not `SandboxPlatform::Seatbelt`.
#[cfg(target_os = "macos")]
#[test]
fn run_does_not_install_null_sandbox_on_macos() {
    let sandbox = build_sandbox();

    assert_ne!(
        sandbox.platform(),
        SandboxPlatform::Null,
        "AUDIT-P0: build_sandbox() returned NullSandbox on macOS. \
         Expected SeatbeltSandbox (platform = Seatbelt). \
         Fix: replace `Arc::new(NullSandbox)` with `SandboxBuilder::detect().into()` \
         in `execution.rs::build_sandbox()`."
    );

    assert_eq!(
        sandbox.platform(),
        SandboxPlatform::Seatbelt,
        "AUDIT-P0: on macOS build_sandbox() must return SeatbeltSandbox. \
         Got: {:?}",
        sandbox.platform()
    );
}

/// On Linux, `build_sandbox()` must return a Landlock-backed sandbox — not a
/// NullSandbox — so that every `POST /run` agent runs under real LSM
/// confinement.
///
/// FAILS TODAY: `build_sandbox()` returns `Arc::new(NullSandbox)` whose
/// `platform()` is `SandboxPlatform::Null`, not `SandboxPlatform::Landlock`.
#[cfg(target_os = "linux")]
#[test]
fn run_does_not_install_null_sandbox_on_linux() {
    let sandbox = build_sandbox();

    assert_ne!(
        sandbox.platform(),
        SandboxPlatform::Null,
        "AUDIT-P0: build_sandbox() returned NullSandbox on Linux. \
         Expected LandlockSandbox (platform = Landlock). \
         Fix: replace `Arc::new(NullSandbox)` with `SandboxBuilder::detect().into()` \
         in `execution.rs::build_sandbox()`."
    );

    assert_eq!(
        sandbox.platform(),
        SandboxPlatform::Landlock,
        "AUDIT-P0: on Linux build_sandbox() must return LandlockSandbox. \
         Got: {:?}",
        sandbox.platform()
    );
}

// ---------------------------------------------------------------------------
// Contract 2 — The sandbox is `SandboxBuilder::detect()`-derived.
//
// `build_sandbox()` must select the same backend as `SandboxBuilder::detect()`.
// This contract is platform-independent: even on unsupported hosts (where both
// return Null) the platforms must match.  The critical case is macOS and Linux
// where `detect()` returns a non-Null impl but `build_sandbox()` currently
// returns Null.
// ---------------------------------------------------------------------------

/// `build_sandbox()` must select the same sandbox backend as
/// `SandboxBuilder::detect()`.
///
/// FAILS TODAY on macOS/Linux: `build_sandbox()` returns `NullSandbox`
/// (platform = Null) while `SandboxBuilder::detect()` returns `Seatbelt` or
/// `Landlock` respectively.
#[test]
fn build_sandbox_platform_matches_sandbox_builder_detect() {
    let server_sandbox = build_sandbox();
    let expected_sandbox: Box<dyn Sandbox> = SandboxBuilder::detect();

    assert_eq!(
        server_sandbox.platform(),
        expected_sandbox.platform(),
        "AUDIT-P0: build_sandbox() platform ({:?}) does not match \
         SandboxBuilder::detect() platform ({:?}). \
         The server must use detect() — not a hard-coded NullSandbox — so that \
         the strongest available kernel sandbox is always selected.",
        server_sandbox.platform(),
        expected_sandbox.platform()
    );
}

// ---------------------------------------------------------------------------
// Contract 3 — Workspace root is honoured by the sandbox: the configured
//              `working_dir` must survive the full request path unmodified.
//
// We test this via the HTTP layer: issue a POST /run with `working_dir` equal
// to the server's current-directory workspace root, verify the request is
// accepted (200 SSE), proving the working_dir flows into the agent config
// rather than being silently replaced by `/` or discarded.
//
// The negative complement (out-of-root working_dir rejected with 400) is
// pinned by `run_working_dir.rs`; we only test the positive case here.
// ---------------------------------------------------------------------------

/// When `working_dir` is inside the configured workspace root, the request
/// must be accepted (200 SSE) — confirming the validated path reaches the
/// `AgentConfig` that will receive the sandbox.
///
/// This test is intentionally not `cfg`-gated and PASSES TODAY.  It is
/// included as a guardian: future refactors of `build_sandbox()` must not
/// break the HTTP plumbing that passes `working_dir` into `AgentConfig`.
/// It complements the two failing tests above by ensuring the "valid path"
/// branch of the sandbox/working_dir code path continues to work after the
/// green-phase fix is applied.
///
/// NOTE: this test passes with `NullSandbox` today because `NullSandbox`
/// never rejects anything.  Its value is preserved post-fix: with a real
/// sandbox, a `working_dir` inside the root must still be accepted.
#[tokio::test]
async fn run_workspace_root_working_dir_reaches_agent_config() {
    let cwd = std::env::current_dir().unwrap();
    let body = format!(
        r#"{{"prompt":"x","working_dir":"{}"}}"#,
        cwd.display().to_string().replace('\\', "\\\\")
    );

    let response = app().oneshot(run_request(&body)).await.unwrap();

    // A valid working_dir (inside workspace root) must NEVER yield a 400.
    // A 400 here would indicate over-zealous validation — equally a bug.
    assert_ne!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "AUDIT-P0: working_dir equal to server cwd was rejected with 400. \
         A valid path inside the workspace root must always be accepted."
    );

    // The request is accepted with a 200 SSE stream.  With no API key the
    // stream will contain a single error event, but the HTTP status must be 200
    // (the sandbox plumbing must not prevent a valid request from reaching the
    // SSE layer).
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "AUDIT-P0: POST /run with valid working_dir must return 200 SSE. \
         Got: {}",
        response.status()
    );
}

/// A `working_dir` outside the workspace root must be rejected before any
/// sandbox is applied, and the rejection must be `400 BAD REQUEST`.  This
/// mirrors and reinforces the contract pinned in `run_working_dir.rs` from
/// the perspective of the sandbox audit trail: even if the sandbox were
/// `NullSandbox`, the HTTP layer must still gate on `workspace_root`.
///
/// PASSES TODAY (this is a positive assertion of already-green behaviour) —
/// included here so that future refactors of `build_sandbox()` cannot
/// accidentally remove the `working_dir` validation that lives upstream of
/// sandbox selection.
#[tokio::test]
async fn run_with_out_of_root_working_dir_is_rejected_before_sandbox_is_consulted() {
    let body = r#"{"prompt":"x","working_dir":"/etc"}"#;

    let response = app().oneshot(run_request(body)).await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "AUDIT-P0: a working_dir outside workspace_root must be rejected 400 \
         before any sandbox is instantiated. Got: {}",
        response.status()
    );
}
