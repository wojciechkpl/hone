//! Integration test: agent loop drives two turns through the persistent
//! ClaudeCliProvider without exiting prematurely.
//!
//! This is the red-phase test for the multi-turn refactor.  It will FAIL
//! against the current implementation because `ClaudeCliProvider` spawns a
//! fresh process per `complete()` call and the stub (which expects persistent
//! stdin) will not receive the second turn's input correctly.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use hone_core::{Agent, AgentConfig, RetryConfig};
use hone_proto::{AgentEvent, ModelId, PricingEntry};
use hone_providers::{ClaudeCliProvider, ModelProvider};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn write_script(dir: &std::path::Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("write script");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("chmod script");
    path.to_str().unwrap().to_owned()
}

/// A persistent stub that handles multiple turns via a long-running read loop.
/// Each NDJSON input line receives one assistant + one success result response.
///
/// `<log_dir>` is baked into the script body so the stub does not depend on a
/// shared `$LOG_DIR` env var (which would race across parallel test threads).
fn write_persistent_fake_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
# Persistent stub: stays alive until stdin closes, handling multiple turns.
LOG_DIR={log_dir}
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then prev=$(cat "$count_file"); echo $((prev + 1)) > "$count_file"; else echo 1 > "$count_file"; fi

turn=0
while IFS= read -r line; do
    [ -z "$line" ] && continue
    echo "$line" >> "$LOG_DIR/received_lines"
    turn=$((turn + 1))
    printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"turn %d response"}}]}}}}\n' "$turn"
    printf '{{"type":"result","subtype":"success","session_id":"stable-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
done
"#
    );
    write_script(dir, "claude", &body)
}

fn base_config_with_cli(binary: &str) -> AgentConfig {
    let provider = Arc::new(ClaudeCliProvider::with_binary(binary));
    // Use the first model reported by the provider itself.
    let model = provider
        .supported_models()
        .into_iter()
        .next()
        .expect("at least one model")
        .id;

    AgentConfig {
        provider: provider as Arc<dyn hone_providers::ModelProvider>,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "You are a helpful assistant.".to_owned(),
        model,
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("claude-cli:claude-sonnet-4-6"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: std::path::PathBuf::from("."),
        memory_store: None,
        retry_config: RetryConfig {
            max_attempts: 1,
            initial_delay: std::time::Duration::from_millis(1),
            max_delay: std::time::Duration::from_millis(1),
            multiplier: 1.0,
        },
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        step_mode: false,
        step_rx: None,
    }
}

// ---------------------------------------------------------------------------
// Test 6: agent_loop_completes_multiturn_conversation
//
// Expected failure today: ClaudeCliProvider spawns a new process for each
// complete() call.  The second call launches a new stub instance which gets
// no stdin input (because the persistent read loop has already been set up
// by a previous process), so it exits immediately without emitting any
// assistant content.  The agent therefore sees an empty turn and the second
// run_turn() either produces an empty response or an error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agent_loop_completes_multiturn_conversation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_persistent_fake_claude(&log_dir);

    let mut agent = Agent::new(base_config_with_cli(&script));

    // --- Turn 1 ---
    let (tx1, mut rx1) = mpsc::channel(32);
    let result1 = agent
        .run_turn("first user message", tx1)
        .await
        .expect("turn 1 should succeed");

    // Collect streamed tokens.
    let mut turn1_tokens = String::new();
    while let Ok(ev) = rx1.try_recv() {
        if let AgentEvent::Token { text } = ev {
            turn1_tokens.push_str(&text);
        }
    }

    assert!(
        !result1.response.is_empty(),
        "turn 1 response must not be empty; got: {:?}",
        result1.response
    );
    assert!(
        result1.response.contains("turn 1") || !turn1_tokens.is_empty(),
        "turn 1 must produce assistant content from the stub"
    );

    // --- Turn 2 ---
    let (tx2, mut rx2) = mpsc::channel(32);
    let result2 = agent
        .run_turn("second user message", tx2)
        .await
        .expect("turn 2 should succeed without re-spawning the CLI");

    let mut turn2_tokens = String::new();
    while let Ok(ev) = rx2.try_recv() {
        if let AgentEvent::Token { text } = ev {
            turn2_tokens.push_str(&text);
        }
    }

    assert!(
        !result2.response.is_empty(),
        "turn 2 response must not be empty; the agent must reuse the \
         persistent CLI process rather than spawning a new one"
    );

    // Verify the stub received exactly two NDJSON lines — one per turn.
    let received = std::fs::read_to_string(log_dir.join("received_lines"))
        .expect("received_lines must exist after two turns");
    let line_count = received.lines().count();
    assert_eq!(
        line_count, 2,
        "stub must receive exactly 2 NDJSON lines (one per turn), got {line_count}"
    );

    // Verify only one process was spawned.
    let spawn_count: u32 = std::fs::read_to_string(log_dir.join("spawn_count"))
        .expect("spawn_count must exist")
        .trim()
        .parse()
        .expect("spawn_count must be a number");
    assert_eq!(
        spawn_count, 1,
        "agent must not re-spawn the CLI between turns; got spawn_count={spawn_count}"
    );
}
