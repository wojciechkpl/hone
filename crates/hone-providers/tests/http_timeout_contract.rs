//! AUDIT-P1 #2 — HTTP client timeout contract tests (red phase)
//!
//! ## Why these tests fail today
//!
//! `AnthropicProvider`, `OpenAiProvider`, and `OllamaProvider` all construct
//! their `reqwest::Client` with `Client::new()` — a zero-configuration builder
//! that sets neither `timeout()` nor `connect_timeout()`. A stalled upstream
//! will therefore hold a Tokio task open indefinitely (until OS TCP keepalive,
//! which defaults to hours). These tests pin the fix by reading accessor
//! methods that do not exist yet:
//!
//!   - `AnthropicProvider::http_client_timeout() -> Option<Duration>`
//!   - `AnthropicProvider::http_client_connect_timeout() -> Option<Duration>`
//!   - `OpenAiProvider::http_client_timeout() -> Option<Duration>`
//!   - `OpenAiProvider::http_client_connect_timeout() -> Option<Duration>`
//!   - `OllamaProvider::http_client_timeout() -> Option<Duration>`
//!   - `OllamaProvider::http_client_connect_timeout() -> Option<Duration>`
//!
//! Until the green-phase implementer adds these accessors AND configures the
//! `reqwest::ClientBuilder` with real finite timeouts, every test here will
//! produce a compile error (method not found), which counts as a failure.
//!
//! ## Strategy: Option B — structural accessor test
//!
//! Option A (spin up a silent TCP server and assert the call returns `Err`
//! within a deadline) would work for OllamaProvider because it can be pointed
//! at `http://localhost:<port>` without auth. But Anthropic and OpenAI require
//! a real HTTP response shape to get past the `send().await` call — a silent
//! server triggers `reqwest::Error` on missing headers, not a timeout error, so
//! Option A cannot cleanly distinguish "timeout fired" from "server sent
//! garbage". Option B requires only that each adapter exposes its configured
//! timeout value through a thin read accessor; no network traffic is needed.
//!
//! ## Ceilings asserted
//!
//!   - Total request timeout  ≤ 300 s  (5 minutes is already generous for LLM calls)
//!   - Connect timeout        ≤  30 s  (TCP handshake should never need more than this)
//!
//! The green-phase implementer is free to pick any finite value within these
//! bounds; the tests only enforce an upper limit and `.is_some()`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::time::Duration;

use hone_providers::{AnthropicProvider, OllamaProvider, OpenAiProvider};

// ── ceiling constants ────────────────────────────────────────────────────────

const MAX_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

// ── AnthropicProvider ────────────────────────────────────────────────────────

#[tokio::test]
async fn anthropic_client_has_request_timeout() {
    let provider = AnthropicProvider::new("test-key");
    let timeout = provider.http_client_timeout();
    assert!(
        timeout.is_some(),
        "AnthropicProvider must configure a finite request timeout on its HTTP client; \
         got None — Client::new() sets no timeout"
    );
    let t = timeout.unwrap();
    assert!(
        t <= MAX_REQUEST_TIMEOUT,
        "AnthropicProvider request timeout {t:?} exceeds ceiling of {MAX_REQUEST_TIMEOUT:?}"
    );
}

#[tokio::test]
async fn anthropic_client_has_connect_timeout() {
    let provider = AnthropicProvider::new("test-key");
    let timeout = provider.http_client_connect_timeout();
    assert!(
        timeout.is_some(),
        "AnthropicProvider must configure a finite connect timeout on its HTTP client; \
         got None — Client::new() sets no connect timeout"
    );
    let t = timeout.unwrap();
    assert!(
        t <= MAX_CONNECT_TIMEOUT,
        "AnthropicProvider connect timeout {t:?} exceeds ceiling of {MAX_CONNECT_TIMEOUT:?}"
    );
}

#[tokio::test]
async fn anthropic_with_base_url_client_has_request_timeout() {
    let provider = AnthropicProvider::with_base_url("test-key", "http://localhost:9999");
    let timeout = provider.http_client_timeout();
    assert!(
        timeout.is_some(),
        "AnthropicProvider::with_base_url must also configure a finite request timeout"
    );
    assert!(timeout.unwrap() <= MAX_REQUEST_TIMEOUT);
}

#[tokio::test]
async fn anthropic_with_base_url_client_has_connect_timeout() {
    let provider = AnthropicProvider::with_base_url("test-key", "http://localhost:9999");
    let timeout = provider.http_client_connect_timeout();
    assert!(
        timeout.is_some(),
        "AnthropicProvider::with_base_url must also configure a finite connect timeout"
    );
    assert!(timeout.unwrap() <= MAX_CONNECT_TIMEOUT);
}

// ── OpenAiProvider ───────────────────────────────────────────────────────────

#[tokio::test]
async fn openai_client_has_request_timeout() {
    let provider = OpenAiProvider::new("test-key");
    let timeout = provider.http_client_timeout();
    assert!(
        timeout.is_some(),
        "OpenAiProvider must configure a finite request timeout on its HTTP client; \
         got None — Client::new() sets no timeout"
    );
    let t = timeout.unwrap();
    assert!(
        t <= MAX_REQUEST_TIMEOUT,
        "OpenAiProvider request timeout {t:?} exceeds ceiling of {MAX_REQUEST_TIMEOUT:?}"
    );
}

#[tokio::test]
async fn openai_client_has_connect_timeout() {
    let provider = OpenAiProvider::new("test-key");
    let timeout = provider.http_client_connect_timeout();
    assert!(
        timeout.is_some(),
        "OpenAiProvider must configure a finite connect timeout on its HTTP client; \
         got None — Client::new() sets no connect timeout"
    );
    let t = timeout.unwrap();
    assert!(
        t <= MAX_CONNECT_TIMEOUT,
        "OpenAiProvider connect timeout {t:?} exceeds ceiling of {MAX_CONNECT_TIMEOUT:?}"
    );
}

#[tokio::test]
async fn openai_with_base_url_client_has_request_timeout() {
    let provider = OpenAiProvider::with_base_url("test-key", "http://localhost:9999");
    let timeout = provider.http_client_timeout();
    assert!(
        timeout.is_some(),
        "OpenAiProvider::with_base_url must also configure a finite request timeout"
    );
    assert!(timeout.unwrap() <= MAX_REQUEST_TIMEOUT);
}

#[tokio::test]
async fn openai_with_base_url_client_has_connect_timeout() {
    let provider = OpenAiProvider::with_base_url("test-key", "http://localhost:9999");
    let timeout = provider.http_client_connect_timeout();
    assert!(
        timeout.is_some(),
        "OpenAiProvider::with_base_url must also configure a finite connect timeout"
    );
    assert!(timeout.unwrap() <= MAX_CONNECT_TIMEOUT);
}

// ── OllamaProvider ───────────────────────────────────────────────────────────

#[tokio::test]
async fn ollama_client_has_request_timeout() {
    let provider = OllamaProvider::new("http://localhost:11434");
    let timeout = provider.http_client_timeout();
    assert!(
        timeout.is_some(),
        "OllamaProvider must configure a finite request timeout on its HTTP client; \
         got None — Client::new() sets no timeout"
    );
    let t = timeout.unwrap();
    assert!(
        t <= MAX_REQUEST_TIMEOUT,
        "OllamaProvider request timeout {t:?} exceeds ceiling of {MAX_REQUEST_TIMEOUT:?}"
    );
}

#[tokio::test]
async fn ollama_client_has_connect_timeout() {
    let provider = OllamaProvider::new("http://localhost:11434");
    let timeout = provider.http_client_connect_timeout();
    assert!(
        timeout.is_some(),
        "OllamaProvider must configure a finite connect timeout on its HTTP client; \
         got None — Client::new() sets no connect timeout"
    );
    let t = timeout.unwrap();
    assert!(
        t <= MAX_CONNECT_TIMEOUT,
        "OllamaProvider connect timeout {t:?} exceeds ceiling of {MAX_CONNECT_TIMEOUT:?}"
    );
}
