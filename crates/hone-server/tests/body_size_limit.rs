//! RED-PHASE regression tests for **AUDIT-P1 #3**: the Axum router in
//! `crates/hone-server/src/routes.rs` has no explicit `DefaultBodyLimit` /
//! `RequestBodyLimitLayer` applied at the router level.  Routes whose handlers
//! do NOT use a `Json<T>` extractor (e.g. `POST /sessions`, which reads no body)
//! receive NO body-size cap at all — a caller can send an arbitrarily large body
//! that is buffered in memory before the handler runs.  OOM vector.
//!
//! # The nuance: Axum's implicit per-extractor cap
//!
//!   Axum 0.8 applies a default `DefaultBodyLimit` of 2 MiB *per extractor*
//!   when `Json<T>` is used.  This means `POST /run` (which extracts `Json`)
//!   happens to be capped at 2 MiB today — but only because of an accidental
//!   implicit default, not an intentional, auditable, uniform policy.
//!
//!   Routes that do NOT use `Json<T>` — including `POST /sessions` — have NO
//!   cap whatsoever.  The green-phase fix must add an explicit router-level
//!   layer so that ALL routes, regardless of which extractors they use, share
//!   a consistent, intentional, documented limit.
//!
//! # Which tests fail today (red) vs. pass today (guardian)
//!
//!   RED (fails today, must pass after green-phase fix):
//!     - `post_sessions_rejects_oversized_body_with_413`
//!     - `post_run_rejects_very_large_body_with_413` (> Axum implicit 2 MiB)
//!
//!   GUARDIAN (passes today via Axum's implicit cap; must continue to pass):
//!     - `post_run_accepts_body_at_limit_boundary`
//!
//! # Green-phase hint (do NOT implement here)
//!
//!   Add `RequestBodyLimitLayer::new(HONE_MAX_BODY_BYTES)` (or equivalent
//!   `DefaultBodyLimit::max(HONE_MAX_BODY_BYTES)`) to the Axum router in
//!   `routes.rs` so it wraps ALL routes uniformly.  Expose
//!   `pub const HONE_MAX_BODY_BYTES: usize` in `hone_server` (e.g. `src/lib.rs`)
//!   so these tests can reference it instead of hardcoded sizes.
//!
//!   After the fix, replace `ASSUMED_LIMIT_BYTES` below with
//!   `hone_server::HONE_MAX_BODY_BYTES` and remove the constant.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hone_server::{AppState, router};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Assumed limit for boundary tests.
//
// Axum 0.8's `Json<T>` extractor uses a built-in default `DefaultBodyLimit`
// of 2 MiB (2 * 1024 * 1024 bytes).  We use this as the assumed explicit
// limit that the green-phase implementer must match or undercut.
//
// `VERY_LARGE_BODY_BYTES` is chosen well above any sane limit (16 MiB) to
// prove that routes WITHOUT a `Json` extractor have no cap today.
//
// Replace `ASSUMED_LIMIT_BYTES` with `hone_server::HONE_MAX_BODY_BYTES` once
// the green-phase constant is exposed.
// ---------------------------------------------------------------------------
const ASSUMED_LIMIT_BYTES: usize = 2 * 1024 * 1024; // 2 MiB — Axum 0.8 Json extractor default
const VERY_LARGE_BODY_BYTES: usize = 16 * 1024 * 1024; // 16 MiB — well above any sane limit

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Build a minimal test app with no provider key.  Body-limit rejection must
/// occur before any handler logic runs, independently of credentials.
fn app() -> axum::Router {
    let state = AppState::new(None).with_provider_key(None);
    router(state)
}

/// Build a POST request to `uri` with `content-type: application/json` and
/// the given raw body bytes.
fn post_request(uri: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

/// Produce a byte vector of `total_bytes` length containing a JSON-looking
/// payload.  The extra bytes are stuffed into an ignored `"pad"` field so
/// that the body is at least syntactically plausible JSON.
fn padded_body(total_bytes: usize) -> Vec<u8> {
    const PREFIX: &[u8] = b"{\"prompt\":\"x\",\"pad\":\"";
    const SUFFIX: &[u8] = b"\"}";
    let overhead = PREFIX.len() + SUFFIX.len();
    let pad_len = total_bytes.saturating_sub(overhead);
    let mut v = Vec::with_capacity(total_bytes);
    v.extend_from_slice(PREFIX);
    v.extend(std::iter::repeat(b'x').take(pad_len));
    v.extend_from_slice(SUFFIX);
    v
}

// ---------------------------------------------------------------------------
// RED — Test 1: POST /sessions must reject an oversized body with 413.
//
// `POST /sessions` has no `Json<T>` extractor (it reads no body), so Axum's
// per-extractor 2 MiB default does NOT apply.  Today the route accepts any
// body size and returns 201 CREATED — the OOM vector in its purest form.
//
// FAILS TODAY: no router-level body-limit layer → `create_session` returns
// 201 CREATED for a 16 MiB body.
// ---------------------------------------------------------------------------

/// `POST /sessions` must reject a 16 MiB body with `413 PAYLOAD_TOO_LARGE`.
///
/// AUDIT-P1 #3 — FAILS TODAY: the router has no explicit body-limit layer,
/// and `POST /sessions` uses no `Json<T>` extractor, so Axum's implicit
/// per-extractor cap does not apply.  The handler receives the full 16 MiB
/// body and returns `201 CREATED`.
#[tokio::test]
async fn post_sessions_rejects_oversized_body_with_413() {
    let oversized_body = padded_body(VERY_LARGE_BODY_BYTES);

    let response = app()
        .oneshot(post_request("/sessions", oversized_body))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "AUDIT-P1 #3: POST /sessions with a 16 MiB body must return 413. \
         Got: {}. \
         Fix: add a router-level RequestBodyLimitLayer so ALL routes are \
         protected, not just those using Json<T> extractors.",
        response.status()
    );
}

// ---------------------------------------------------------------------------
// RED — Test 2: POST /run must reject a body well above any sane limit.
//
// Today Axum's implicit Json extractor default (2 MiB) caps /run at 2 MiB.
// A router-level explicit layer at, say, 4 MiB must ALSO reject 16 MiB.
// This test verifies that a very-large body (16 MiB) is rejected — it passes
// today via the implicit cap, but must also pass with an explicit router-level
// layer after the fix (green phase must not accidentally raise the cap to
// something that allows 16 MiB through).
//
// This test PASSES TODAY (Axum's implicit 2 MiB cap rejects it) but is
// included to guard against a green-phase fix that accidentally raises the
// limit beyond 16 MiB.  It is paired with the boundary test below which
// verifies the lower bound of the limit.
// ---------------------------------------------------------------------------

/// `POST /run` must reject a 16 MiB body with `413 PAYLOAD_TOO_LARGE`.
///
/// PASSES TODAY via Axum's implicit `Json<T>` extractor 2 MiB cap, but the
/// cap is not intentional or uniform — this test guards the upper bound.
/// Must continue to pass after the green-phase explicit router-level layer
/// is added (the explicit limit must remain ≤ 16 MiB).
#[tokio::test]
async fn post_run_rejects_very_large_body_with_413() {
    let very_large_body = padded_body(VERY_LARGE_BODY_BYTES);

    let response = app()
        .oneshot(post_request("/run", very_large_body))
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "AUDIT-P1 #3: POST /run with a 16 MiB body must return 413. \
         Got: {}. \
         The explicit body-limit must be ≤ 16 MiB.",
        response.status()
    );
}

// ---------------------------------------------------------------------------
// GUARDIAN — Test 3: POST /run must NOT reject a body at the 2 MiB boundary.
//
// Verifies that the lower bound of the limit is at least 2 MiB — i.e. a body
// equal to the assumed limit is accepted.  This guards against an
// implementation that sets the limit too low and rejects legitimate payloads.
//
// PASSES TODAY: Axum's implicit cap is exactly 2 MiB (inclusive), so a body
// of exactly 2 MiB is not rejected.  Must continue to pass after the
// green-phase fix as long as the explicit limit is ≥ 2 MiB.
// ---------------------------------------------------------------------------

/// `POST /run` with a body of exactly `ASSUMED_LIMIT_BYTES` (2 MiB) must NOT
/// return `413`.  The response may be `200`, `400` (bad schema), etc. — the
/// only contract is that body-size is NOT the rejection cause.
///
/// This is a guardian test: PASSES TODAY and must remain passing.
#[tokio::test]
async fn post_run_accepts_body_at_limit_boundary() {
    let boundary_body = padded_body(ASSUMED_LIMIT_BYTES);

    let response = app()
        .oneshot(post_request("/run", boundary_body))
        .await
        .unwrap();

    assert_ne!(
        response.status(),
        StatusCode::PAYLOAD_TOO_LARGE,
        "AUDIT-P1 #3: POST /run with a body of exactly 2 MiB must NOT return \
         413.  A body at the assumed limit boundary must reach the handler. \
         Got: {}",
        response.status()
    );
}
