use std::os::unix::fs::PermissionsExt;

use futures::StreamExt;
use hone_proto::{Message, ProviderEvent, ProviderHealth};
use hone_providers::adapters::cli_common::{
    estimate_tokens_heuristic, messages_to_prompt, spawn_cli_stream,
};
use hone_providers::{ClaudeCliProvider, GeminiCliProvider, ModelProvider};
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Health checks — missing binary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_claude_cli_health_when_binary_missing() {
    let provider = ClaudeCliProvider::with_binary("nonexistent_binary_xyz");
    let health = provider.health().await;
    assert!(
        matches!(health, ProviderHealth::Unavailable { .. }),
        "expected Unavailable, got {health:?}"
    );
}

#[tokio::test]
async fn test_gemini_cli_health_when_binary_missing() {
    let provider = GeminiCliProvider::with_binary("nonexistent_binary_xyz");
    let health = provider.health().await;
    assert!(
        matches!(health, ProviderHealth::Unavailable { .. }),
        "expected Unavailable, got {health:?}"
    );
}

// ---------------------------------------------------------------------------
// Supported models
// ---------------------------------------------------------------------------

#[test]
fn test_claude_cli_supported_models() {
    let provider = ClaudeCliProvider::new();
    let models = provider.supported_models();
    assert_eq!(models.len(), 3, "expected 3 models, got {}", models.len());

    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"claude-cli:claude-sonnet-4-5"));
    assert!(ids.contains(&"claude-cli:claude-opus-4-5"));
    assert!(ids.contains(&"claude-cli:claude-haiku-3-5"));
}

#[test]
fn test_gemini_cli_supported_models() {
    let provider = GeminiCliProvider::new();
    let models = provider.supported_models();
    assert_eq!(models.len(), 2, "expected 2 models, got {}", models.len());

    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert!(ids.contains(&"gemini-cli:gemini-2.5-pro"));
    assert!(ids.contains(&"gemini-cli:gemini-2.5-flash"));
}

// ---------------------------------------------------------------------------
// messages_to_prompt
// ---------------------------------------------------------------------------

#[test]
fn test_messages_to_prompt() {
    let messages = vec![
        Message::system("You are a helpful assistant."),
        Message::user("What is 2 + 2?"),
        Message::assistant("The answer is 4."),
    ];

    let prompt = messages_to_prompt(&messages);

    assert!(prompt.contains("You are a helpful assistant."));
    assert!(prompt.contains("What is 2 + 2?"));
    assert!(prompt.contains("The answer is 4."));
    assert!(prompt.contains("SYSTEM"));
    assert!(prompt.contains("USER"));
    assert!(prompt.contains("ASSISTANT"));
}

// ---------------------------------------------------------------------------
// estimate_tokens_heuristic
// ---------------------------------------------------------------------------

#[test]
fn test_estimate_tokens_heuristic() {
    // 400 chars -> ~100 tokens
    let content = "a".repeat(400);
    let messages = vec![Message::user(content)];
    let estimate = estimate_tokens_heuristic(&messages);
    assert_eq!(estimate, 100, "expected 100 tokens for 400 chars, got {estimate}");
}

// ---------------------------------------------------------------------------
// spawn_cli_stream helpers
// ---------------------------------------------------------------------------

/// Write a small shell script to a temp file, make it executable, and return
/// the path as a String.
fn write_script(dir: &std::path::Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("failed to write script");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("failed to set permissions");
    path.to_str().unwrap().to_owned()
}

#[tokio::test]
async fn test_spawn_cli_stream_captures_output() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let script = write_script(
        dir.path(),
        "echo_lines.sh",
        "#!/bin/sh\necho 'line one'\necho 'line two'\necho 'line three'\n",
    );

    let cmd = Command::new(&script);
    let stream = spawn_cli_stream(cmd);

    let events: Vec<ProviderEvent> = stream.collect().await;

    let token_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, ProviderEvent::Token { .. }))
        .collect();

    assert_eq!(
        token_events.len(),
        3,
        "expected 3 Token events, got {}: {events:?}",
        token_events.len()
    );

    assert!(events.contains(&ProviderEvent::Done), "expected Done event");
}

#[tokio::test]
async fn test_spawn_cli_stream_reports_error_on_failure() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let script = write_script(
        dir.path(),
        "fail.sh",
        "#!/bin/sh\necho 'about to fail' >&2\nexit 1\n",
    );

    let cmd = Command::new(&script);
    let stream = spawn_cli_stream(cmd);

    let events: Vec<ProviderEvent> = stream.collect().await;

    let has_error = events
        .iter()
        .any(|e| matches!(e, ProviderEvent::Error { .. }));

    assert!(
        has_error,
        "expected an Error event for non-zero exit, got {events:?}"
    );

    assert!(
        events.contains(&ProviderEvent::Done),
        "expected Done event after error, got {events:?}"
    );

    // Error must come before Done.
    let error_pos = events
        .iter()
        .position(|e| matches!(e, ProviderEvent::Error { .. }))
        .unwrap();
    let done_pos = events
        .iter()
        .position(|e| *e == ProviderEvent::Done)
        .unwrap();
    assert!(
        error_pos < done_pos,
        "Error must appear before Done, positions: error={error_pos} done={done_pos}"
    );
}
