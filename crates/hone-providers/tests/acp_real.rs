#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

//! Integration tests for ACP provider against a real ACP binary.
//!
//! These tests are #[ignore]d so they don't run in default `cargo test`.
//! To run them, set `HONE_ACP_BIN` to a real ACP binary path:
//!
//! ```text
//! HONE_ACP_BIN=claude cargo test --test acp_real -- --ignored
//! ```
//!
//! Or use the Makefile target:
//!
//! ```text
//! HONE_ACP_BIN=claude make test-acp
//! ```
//!
//! Compatible binaries (as of 2026):
//! - `claude --acp`  (Anthropic Claude Code as ACP server)
//! - `gemini --acp`  (Google Gemini as ACP server)
//! - any other binary speaking the ACP protocol over stdio
//!
//! The env var should be just the binary name (e.g. `claude`); the `--acp`
//! flag is appended automatically.  To pass custom arguments, extend
//! `acp_binary_or_skip` or supply a wrapper script.

use std::env;
use std::pin::Pin;

use futures::{Stream, StreamExt};
use hone_proto::{Message, ModelId, ProviderEvent, ProviderHealth};
use hone_providers::{AcpProvider, CompletionRequest, ModelProvider};

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

/// Return (binary, args) from `HONE_ACP_BIN`, or print a skip message and
/// return `None`.  The `--acp` flag is always appended as the first argument.
fn acp_binary_or_skip() -> Option<(String, Vec<String>)> {
    match env::var("HONE_ACP_BIN") {
        Ok(b) if !b.is_empty() => Some((b, vec!["--acp".to_owned()])),
        _ => {
            eprintln!("HONE_ACP_BIN not set — skipping real ACP test");
            eprintln!("  Set HONE_ACP_BIN=claude (or another ACP binary) to enable.");
            None
        }
    }
}

fn make_provider(binary: &str, args: &[String]) -> AcpProvider {
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    AcpProvider::custom(binary, &args_ref, "acp-real", "ACP (real)", vec![])
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore]
async fn test_acp_real_basic_completion() {
    let Some((binary, args)) = acp_binary_or_skip() else {
        return;
    };

    let provider = make_provider(&binary, &args);

    let request = CompletionRequest::new(
        ModelId::new("acp-real:default"),
        vec![Message::user(
            "Reply with the single word OK and nothing else.",
        )],
    );

    let stream: Pin<Box<dyn Stream<Item = ProviderEvent> + Send>> = provider
        .complete(request)
        .await
        .expect("provider.complete failed");

    let events: Vec<ProviderEvent> = stream.collect().await;

    let has_token = events
        .iter()
        .any(|e| matches!(e, ProviderEvent::Token { .. }));
    let has_done = events.iter().any(|e| matches!(e, ProviderEvent::Done));

    assert!(
        has_token,
        "expected at least one Token event from real ACP binary; got {events:?}"
    );
    assert!(
        has_done,
        "expected Done event from real ACP binary; got {events:?}"
    );

    let text: String = events
        .iter()
        .filter_map(|e| match e {
            ProviderEvent::Token { text } => Some(text.as_str()),
            _ => None,
        })
        .collect();

    println!("ACP response text: {text}");
    // Do not assert specific content — real models may phrase responses differently.
}

#[tokio::test]
#[ignore]
async fn test_acp_real_done_is_last_event() {
    let Some((binary, args)) = acp_binary_or_skip() else {
        return;
    };

    let provider = make_provider(&binary, &args);

    let request = CompletionRequest::new(
        ModelId::new("acp-real:default"),
        vec![Message::user("Say anything brief.")],
    );

    let stream: Pin<Box<dyn Stream<Item = ProviderEvent> + Send>> = provider
        .complete(request)
        .await
        .expect("provider.complete failed");

    let events: Vec<ProviderEvent> = stream.collect().await;

    assert!(!events.is_empty(), "expected at least one event");

    let last = events.last().unwrap();
    assert_eq!(
        last,
        &ProviderEvent::Done,
        "Done must be the last event; got {last:?}"
    );
}

#[tokio::test]
#[ignore]
async fn test_acp_real_health_check() {
    let Some((binary, args)) = acp_binary_or_skip() else {
        return;
    };

    let provider = make_provider(&binary, &args);

    let health = provider.health().await;

    match &health {
        ProviderHealth::Healthy => {
            println!("ACP binary '{binary}' is healthy");
        }
        ProviderHealth::Degraded { reason } => {
            eprintln!("ACP binary '{binary}' degraded: {reason}");
            // Degraded is acceptable — the binary is present and (partially) functional.
        }
        ProviderHealth::Unavailable { reason } => {
            panic!("ACP binary '{binary}' is unavailable: {reason}");
        }
    }
}

#[tokio::test]
#[ignore]
async fn test_acp_real_supported_models() {
    let Some((binary, args)) = acp_binary_or_skip() else {
        return;
    };

    let provider = make_provider(&binary, &args);

    let models = provider.supported_models();
    println!(
        "Supported models from '{binary}': {:?}",
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );

    // Real binaries may return an empty list if the protocol does not expose
    // model enumeration — just verify the call does not panic.
}
