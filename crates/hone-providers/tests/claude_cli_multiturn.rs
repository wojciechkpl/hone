//! Failing tests for the multi-turn ClaudeCliProvider refactor.
//!
//! These tests define the contract for the new persistent-process behaviour:
//! - spawn once per provider instance, keep stdin/stdout open
//! - per-turn NDJSON user messages (not flat prompts)
//! - stable session_id UUID across all turns
//! - --input-format stream-json --output-format stream-json flags, no -p
//! - Done emitted only on result.subtype == "success" (not on "error_max_turns")
//!
//! ALL tests in this file are expected to FAIL against the current implementation.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;

use futures::StreamExt;
use hone_proto::{Message, ProviderEvent};
use hone_providers::{ClaudeCliProvider, CompletionRequest, ModelProvider};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write a shell script into `dir`, make it executable, and return its path.
fn write_script(dir: &std::path::Path, name: &str, body: &str) -> String {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("failed to write script");
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).expect("failed to chmod script");
    path.to_str().unwrap().to_owned()
}

/// Build a CompletionRequest for the given user text, using the first
/// supported model from ClaudeCliProvider.
fn make_request(provider: &ClaudeCliProvider, user_text: &str) -> CompletionRequest {
    let model = provider
        .supported_models()
        .into_iter()
        .next()
        .expect("at least one supported model")
        .id;
    CompletionRequest::new(model, vec![Message::user(user_text)])
}

/// Build a CompletionRequest with explicit messages (history + new user turn).
fn make_request_with_history(
    provider: &ClaudeCliProvider,
    messages: Vec<Message>,
) -> CompletionRequest {
    let model = provider
        .supported_models()
        .into_iter()
        .next()
        .expect("at least one supported model")
        .id;
    CompletionRequest::new(model, messages)
}

/// Drain a provider stream to completion and return all events.
async fn drain(provider: &ClaudeCliProvider, request: CompletionRequest) -> Vec<ProviderEvent> {
    let stream = provider
        .complete(request)
        .await
        .expect("complete() should not fail at call time");
    stream.collect::<Vec<_>>().await
}

// ---------------------------------------------------------------------------
// The fake `claude` stub factory
//
// The stub is a shell script that:
//   1. Increments a per-process spawn counter in <log_dir>/spawn_count.
//   2. Reads NDJSON lines from stdin in a loop.
//   3. Logs each received line to <log_dir>/received_lines.
//   4. Logs its own argv to <log_dir>/argv.
//   5. For each input line emits one assistant message + one success result.
//
// `<log_dir>` is baked into the script body by the Rust caller so each test
// has a fully self-contained stub. We avoid `$LOG_DIR` env-var coupling
// because cargo runs unit tests in parallel threads of the same process and
// `std::env::set_var` calls would clobber each other across threads.
// ---------------------------------------------------------------------------

fn write_fake_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
# Fake claude stub for multi-turn tests.
LOG_DIR={log_dir}

# Record argv so tests can assert on flags.
echo "$@" >> "$LOG_DIR/argv"

# Increment spawn counter (file contains a single integer).
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then
    prev=$(cat "$count_file")
    echo $((prev + 1)) > "$count_file"
else
    echo 1 > "$count_file"
fi

# Main loop: read one NDJSON line at a time and respond.
while IFS= read -r line; do
    [ -z "$line" ] && continue

    # Log every received line.
    echo "$line" >> "$LOG_DIR/received_lines"

    # Emit an assistant message.
    printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"stub response"}}]}}}}\n'

    # Emit a success result so the provider knows the turn ended.
    printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
done
"#
    );
    write_script(dir, "claude", &body)
}

/// Write a variant stub that first emits an error_max_turns result and then
/// a success result for each input line, to exercise the subtype gate.
fn write_fake_claude_error_then_success(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
LOG_DIR={log_dir}
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then prev=$(cat "$count_file"); echo $((prev + 1)) > "$count_file"; else echo 1 > "$count_file"; fi

# First input line: emit error_max_turns, then success.
IFS= read -r first_line
echo "$first_line" >> "$LOG_DIR/received_lines"
printf '{{"type":"result","subtype":"error_max_turns","session_id":"s","usage":{{"input_tokens":1,"output_tokens":0,"cache_read_input_tokens":0}}}}\n'
printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"retry response"}}]}}}}\n'
printf '{{"type":"result","subtype":"success","session_id":"s","usage":{{"input_tokens":2,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
"#
    );
    write_script(dir, "claude", &body)
}

// ---------------------------------------------------------------------------
// Test 1: spawns_cli_once_across_multiple_complete_calls
//
// Expected failure: current code spawns a NEW process per complete() call,
// so spawn_count will be 2, not 1.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawns_cli_once_across_multiple_complete_calls() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_fake_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // First turn.
    let req1 = make_request(&provider, "hello from turn one");
    drain(&provider, req1).await;

    // Second turn using the SAME provider instance — must reuse the process.
    let req2 = make_request(&provider, "hello from turn two");
    drain(&provider, req2).await;

    // Read the spawn counter.
    let spawn_count_str = std::fs::read_to_string(log_dir.join("spawn_count"))
        .expect("spawn_count file must exist after two complete() calls");
    let spawn_count: u32 = spawn_count_str
        .trim()
        .parse()
        .expect("spawn_count must be a number");

    assert_eq!(
        spawn_count, 1,
        "ClaudeCliProvider must spawn the CLI exactly once per instance, \
         not once per complete() call; got spawn_count={spawn_count}"
    );
}

// ---------------------------------------------------------------------------
// Test 2: sends_only_latest_user_message_per_turn
//
// Expected failure: current code calls messages_to_prompt() which serialises
// the ENTIRE history into one blob. The new code must send only the latest
// user message as a single NDJSON line.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sends_only_latest_user_message_per_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_fake_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // History: system + user1 + assistant1 + new user2.
    let turn1_messages = vec![
        Message::system("You are a helpful assistant."),
        Message::user("user message one"),
        Message::assistant("assistant reply one"),
        Message::user("user message two"),
    ];
    let req1 = make_request_with_history(&provider, turn1_messages);
    drain(&provider, req1).await;

    // Second turn: previous context + user3.
    let turn2_messages = vec![
        Message::system("You are a helpful assistant."),
        Message::user("user message one"),
        Message::assistant("assistant reply one"),
        Message::user("user message two"),
        Message::assistant("assistant reply two"),
        Message::user("user message three"),
    ];
    let req2 = make_request_with_history(&provider, turn2_messages);
    drain(&provider, req2).await;

    let received = std::fs::read_to_string(log_dir.join("received_lines"))
        .expect("received_lines must be written by the stub");

    let lines: Vec<&str> = received.lines().collect();

    // Must have received exactly two NDJSON lines (one per turn).
    assert_eq!(
        lines.len(),
        2,
        "provider must send exactly one NDJSON line per turn, got {}; lines={lines:?}",
        lines.len()
    );

    // First NDJSON line: must contain user2 text, must NOT contain user1.
    let line1: serde_json::Value =
        serde_json::from_str(lines[0]).expect("received line 1 must be valid JSON");
    let text1 = line1
        .pointer("/message/content/0/text")
        .and_then(|v| v.as_str())
        .expect("first line must have /message/content/0/text");
    assert!(
        text1.contains("user message two"),
        "turn 1 NDJSON must contain the latest user message 'user message two'; got: {text1:?}"
    );
    assert!(
        !text1.contains("user message one"),
        "turn 1 NDJSON must NOT include earlier user messages; got: {text1:?}"
    );
    assert!(
        !text1.contains("assistant reply one"),
        "turn 1 NDJSON must NOT include assistant history; got: {text1:?}"
    );

    // Second NDJSON line: must contain user3 text, must NOT contain user1/user2/assistant.
    let line2: serde_json::Value =
        serde_json::from_str(lines[1]).expect("received line 2 must be valid JSON");
    let text2 = line2
        .pointer("/message/content/0/text")
        .and_then(|v| v.as_str())
        .expect("second line must have /message/content/0/text");
    assert!(
        text2.contains("user message three"),
        "turn 2 NDJSON must contain 'user message three'; got: {text2:?}"
    );
    assert!(
        !text2.contains("user message one"),
        "turn 2 NDJSON must not contain 'user message one'; got: {text2:?}"
    );
    assert!(
        !text2.contains("user message two"),
        "turn 2 NDJSON must not contain 'user message two'; got: {text2:?}"
    );
    assert!(
        !text2.contains("assistant reply"),
        "turn 2 NDJSON must not contain assistant history; got: {text2:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 3: reuses_session_id_across_turns
//
// Expected failure: current code does not maintain a session_id at all.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reuses_session_id_across_turns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_fake_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    drain(&provider, make_request(&provider, "turn one")).await;
    drain(&provider, make_request(&provider, "turn two")).await;

    let received =
        std::fs::read_to_string(log_dir.join("received_lines")).expect("received_lines must exist");

    let lines: Vec<&str> = received.lines().collect();
    assert!(
        lines.len() >= 2,
        "need at least two NDJSON lines to compare session_ids"
    );

    let session_id_1 = serde_json::from_str::<serde_json::Value>(lines[0])
        .expect("line 1 must be valid JSON")
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .expect("NDJSON line 1 must contain a top-level 'session_id' field");

    let session_id_2 = serde_json::from_str::<serde_json::Value>(lines[1])
        .expect("line 2 must be valid JSON")
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .expect("NDJSON line 2 must contain a top-level 'session_id' field");

    assert!(
        !session_id_1.is_empty(),
        "session_id must be a non-empty UUID string"
    );
    assert_eq!(
        session_id_1, session_id_2,
        "session_id must be stable across multiple turns on the same provider instance; \
         got {session_id_1:?} then {session_id_2:?}"
    );

    // Verify it looks like a UUID (has the canonical 36-char 8-4-4-4-12 format).
    assert_eq!(
        session_id_1.len(),
        36,
        "session_id must be a 36-character UUID string; got len={}",
        session_id_1.len()
    );
}

// ---------------------------------------------------------------------------
// Test 4: launches_with_stream_json_flags_no_dash_p
//
// Expected failure: current code passes `-p`, `--output-format stream-json`,
// `--verbose`, `--permission-mode`, `--model` but does NOT pass
// `--input-format stream-json` and DOES pass `-p`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn launches_with_stream_json_flags_no_dash_p() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_fake_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);
    drain(&provider, make_request(&provider, "probe")).await;

    let argv_str = std::fs::read_to_string(log_dir.join("argv"))
        .expect("argv file must be written by the stub");

    // Must include --input-format stream-json.
    assert!(
        argv_str.contains("--input-format") && argv_str.contains("stream-json"),
        "CLI must be launched with '--input-format stream-json'; got argv: {argv_str:?}"
    );

    // Must include --output-format stream-json.
    assert!(
        argv_str.contains("--output-format") && argv_str.contains("stream-json"),
        "CLI must be launched with '--output-format stream-json'; got argv: {argv_str:?}"
    );

    // Must NOT include -p (print / one-shot mode).
    // Split on whitespace and check tokens exactly to avoid false positives
    // (e.g. "--permission-mode" contains the letter 'p' but is not "-p").
    let tokens: Vec<&str> = argv_str.split_whitespace().collect();
    assert!(
        !tokens.contains(&"-p"),
        "CLI must NOT be launched with '-p'; got argv: {argv_str:?}"
    );
    assert!(
        !tokens.contains(&"--print"),
        "CLI must NOT be launched with '--print'; got argv: {argv_str:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 5: turn_ends_only_on_result_subtype_success
//
// Expected failure: current code breaks on ANY "result" line. It checks
// `subtype == "error"` but not `subtype == "error_max_turns"`, so it would
// emit Done on the error_max_turns line instead of waiting for success.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_ends_only_on_result_subtype_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    // Use the variant stub that emits error_max_turns before success.
    let script = write_fake_claude_error_then_success(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);
    let events = drain(&provider, make_request(&provider, "probe")).await;

    // Find the positions of Done events.
    let done_positions: Vec<usize> = events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            if *e == ProviderEvent::Done {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    // There must be exactly ONE Done event.
    assert_eq!(
        done_positions.len(),
        1,
        "provider must emit exactly one Done per turn; got positions: {done_positions:?}, \
         all events: {events:?}"
    );

    // The Done must come AFTER the Token carrying "retry response" (which
    // was emitted after the success result, not the error_max_turns).
    let done_pos = done_positions[0];
    let has_retry_token_before_done = events[..done_pos]
        .iter()
        .any(|e| matches!(e, ProviderEvent::Token { text } if text.contains("retry response")));

    assert!(
        has_retry_token_before_done,
        "Done must not be emitted until after the 'success' result; \
         the 'retry response' token (from the assistant message after \
         error_max_turns) must precede Done; events: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 6: ndjson_line_type_is_user
//
// The NDJSON envelope sent per turn must have "type":"user" at the top level,
// matching the protocol spec.
//
// Expected failure: current code writes a flat text prompt to stdin, not JSON.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ndjson_line_has_correct_type_and_role() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_fake_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);
    drain(&provider, make_request(&provider, "structural check")).await;

    let received =
        std::fs::read_to_string(log_dir.join("received_lines")).expect("received_lines must exist");
    let first_line = received.lines().next().expect("at least one received line");

    let json: serde_json::Value =
        serde_json::from_str(first_line).expect("received line must be valid JSON");

    assert_eq!(
        json.get("type").and_then(|v| v.as_str()),
        Some("user"),
        "top-level 'type' must be 'user'; got: {json}"
    );
    assert_eq!(
        json.pointer("/message/role").and_then(|v| v.as_str()),
        Some("user"),
        "/message/role must be 'user'; got: {json}"
    );
    assert_eq!(
        json.pointer("/message/content/0/type")
            .and_then(|v| v.as_str()),
        Some("text"),
        "/message/content/0/type must be 'text'; got: {json}"
    );
    assert!(
        json.pointer("/message/content/0/text")
            .and_then(|v| v.as_str())
            .is_some(),
        "/message/content/0/text must be a string; got: {json}"
    );
}

// ---------------------------------------------------------------------------
// Re-spawn contract (Tests 7 & 8)
//
// When the CLI child process dies between turns (EOF on stdout before a
// `result.subtype == "success"` line is seen), the provider MUST transparently
// re-spawn the binary and complete the next turn normally.  The re-spawn must
// use the same binary path the provider was constructed with.
//
// Current behaviour (the bug):  complete() detects EOF, pushes
// `ProviderEvent::Error` and sets `*guard = None` (the "tombstone").  The next
// complete() call finds a None guard and returns `ProviderError::Http` with
// "CLI process exited unexpectedly".  No Token events are produced for any turn
// after the first death.
//
// Required behaviour (the fix):  after a death, the NEXT complete() call
// re-spawns the child using the stored binary path and proceeds normally.
// Tests assert:
//   - turn N+1 after a death contains at least one non-empty Token event
//   - spawn_count >= 2 (the binary was invoked more than once)
//   - received_lines records the correct user text in order
// ---------------------------------------------------------------------------

/// Write a one-shot stub: increments spawn_count, reads ONE NDJSON line,
/// emits one assistant message + one success result, then exits.
/// Because the stub exits after a single turn, every new child invocation
/// starts fresh — this is the "process dies between turns" scenario.
fn write_oneshot_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
# One-shot stub: responds to exactly one NDJSON line then exits.
LOG_DIR={log_dir}

# Record argv.
echo "$@" >> "$LOG_DIR/argv"

# Increment spawn counter.
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then
    prev=$(cat "$count_file")
    echo $((prev + 1)) > "$count_file"
else
    echo 1 > "$count_file"
fi

# Read exactly one NDJSON line, respond, then exit.
IFS= read -r line
[ -z "$line" ] && exit 0

echo "$line" >> "$LOG_DIR/received_lines"

printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"stub response"}}]}}}}\n'
printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'

exit 0
"#
    );
    write_script(dir, "claude", &body)
}

// ---------------------------------------------------------------------------
// Test 7: respawns_after_process_dies_between_turns
//
// Expected failure (current): turn 2 calls complete() on a tombstoned slot,
// which returns ProviderError::Http.  The test asserts that turn 2 contains
// at least one Token event — that assertion never fires because the stream
// returned for turn 2 (or the complete() call itself) carries only an Error.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn respawns_after_process_dies_between_turns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_oneshot_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // Turn 1: the child processes this turn, emits success, then exits.
    let turn1_events = drain(&provider, make_request(&provider, "turn one")).await;
    // Turn 1 must have succeeded (Token + Done, no early return).
    let turn1_has_token = turn1_events
        .iter()
        .any(|e| matches!(e, ProviderEvent::Token { text } if !text.is_empty()));
    assert!(
        turn1_has_token,
        "turn 1 must produce at least one non-empty Token event; got: {turn1_events:?}"
    );

    // Turn 2: the stub child for turn 1 has already exited.
    // The provider must detect the death and re-spawn before servicing this turn.
    // We call complete() directly so we can assert on both Ok and Err outcomes.
    let turn2_result = provider.complete(make_request(&provider, "turn two")).await;

    let turn2_events: Vec<ProviderEvent> = match turn2_result {
        Ok(stream) => stream.collect::<Vec<_>>().await,
        Err(e) => {
            panic!(
                "turn 2 complete() must not return Err after process death — \
                 provider must re-spawn; got Err: {e:?}"
            );
        }
    };

    // Turn 2 must contain at least one non-empty Token event.
    let turn2_has_token = turn2_events
        .iter()
        .any(|e| matches!(e, ProviderEvent::Token { text } if !text.is_empty()));
    assert!(
        turn2_has_token,
        "turn 2 must produce at least one non-empty Token event after re-spawn; \
         got events: {turn2_events:?}"
    );

    // Verify the binary was invoked more than once (re-spawn happened).
    let spawn_count_str = std::fs::read_to_string(log_dir.join("spawn_count"))
        .expect("spawn_count file must exist after two complete() calls");
    let spawn_count: u32 = spawn_count_str
        .trim()
        .parse()
        .expect("spawn_count must be a number");
    assert!(
        spawn_count >= 2,
        "spawn_count must be >= 2 after one death and one re-spawn; got {spawn_count}"
    );

    // Verify both user texts were received by the stub, in order.
    let received = std::fs::read_to_string(log_dir.join("received_lines"))
        .expect("received_lines must exist after two complete() calls");
    let lines: Vec<&str> = received.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "received_lines must contain exactly 2 NDJSON lines (one per turn); got {}: {lines:?}",
        lines.len()
    );

    let line1_json: serde_json::Value =
        serde_json::from_str(lines[0]).expect("received line 1 must be valid JSON");
    let text1 = line1_json
        .pointer("/message/content/0/text")
        .and_then(|v| v.as_str())
        .expect("line 1 must have /message/content/0/text");
    assert!(
        text1.contains("turn one"),
        "received line 1 must contain 'turn one'; got: {text1:?}"
    );

    let line2_json: serde_json::Value =
        serde_json::from_str(lines[1]).expect("received line 2 must be valid JSON");
    let text2 = line2_json
        .pointer("/message/content/0/text")
        .and_then(|v| v.as_str())
        .expect("line 2 must have /message/content/0/text");
    assert!(
        text2.contains("turn two"),
        "received line 2 must contain 'turn two'; got: {text2:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 8: respawn_recovers_from_repeated_deaths
//
// Expected failure (current): same tombstone bug — after the first death the
// slot is None, so turn 2 returns ProviderError::Http, and the test panics
// asserting that every turn must produce a Token event.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn respawn_recovers_from_repeated_deaths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_oneshot_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    let turn_texts = ["turn one", "turn two", "turn three"];

    for (i, user_text) in turn_texts.iter().enumerate() {
        let turn_num = i + 1;

        let result = provider.complete(make_request(&provider, user_text)).await;

        let events: Vec<ProviderEvent> = match result {
            Ok(stream) => stream.collect::<Vec<_>>().await,
            Err(e) => {
                panic!(
                    "turn {turn_num} complete() must not return Err — \
                     provider must re-spawn after each death; got Err: {e:?}"
                );
            }
        };

        let has_token = events
            .iter()
            .any(|e| matches!(e, ProviderEvent::Token { text } if !text.is_empty()));
        assert!(
            has_token,
            "turn {turn_num} must produce at least one non-empty Token event; \
             got events: {events:?}"
        );

        let has_done = events.contains(&ProviderEvent::Done);
        assert!(
            has_done,
            "turn {turn_num} must produce a Done event; got events: {events:?}"
        );
    }

    // All three child processes must have been spawned.
    let spawn_count_str = std::fs::read_to_string(log_dir.join("spawn_count"))
        .expect("spawn_count file must exist after three complete() calls");
    let spawn_count: u32 = spawn_count_str
        .trim()
        .parse()
        .expect("spawn_count must be a number");
    assert!(
        spawn_count >= 3,
        "spawn_count must be >= 3 after three one-shot turns; got {spawn_count}"
    );

    // All three user texts must appear in received_lines in order.
    let received = std::fs::read_to_string(log_dir.join("received_lines"))
        .expect("received_lines must exist after three complete() calls");
    let lines: Vec<&str> = received.lines().collect();
    assert_eq!(
        lines.len(),
        3,
        "received_lines must contain exactly 3 NDJSON lines; got {}: {lines:?}",
        lines.len()
    );

    for (i, expected_text) in turn_texts.iter().enumerate() {
        let json: serde_json::Value =
            serde_json::from_str(lines[i]).expect("received line must be valid JSON");
        let text = json
            .pointer("/message/content/0/text")
            .and_then(|v| v.as_str())
            .expect("line must have /message/content/0/text");
        assert!(
            text.contains(expected_text),
            "received line {} must contain {expected_text:?}; got: {text:?}",
            i + 1
        );
    }
}

// ---------------------------------------------------------------------------
// Stderr-surfacing contract (Tests C & D)
//
// When the CLI child writes to stderr and then exits without producing a
// successful turn, that stderr text MUST appear in the error surfaced to the
// caller.  Today, stderr is drained by a background task and only emitted at
// debug level — the user sees "claude process exited unexpectedly" with no
// diagnostic.
//
// Contract:
//   C. If every spawn attempt dies after writing stderr, the Err returned by
//      complete() must contain the stderr text in its body.  Spawn count: 2
//      (both attempts ran).
//
//   D. When attempts emit DIFFERENT stderr lines, the most-recent attempt's
//      stderr (the one that decided the final Err) must be captured.  This
//      prevents stale stderr from a previous attempt leaking past a
//      newly-spawned process whose own stderr is the diagnostic the operator
//      needs.
// ---------------------------------------------------------------------------

/// Write a stub that dies after writing a diagnostic to stderr, without
/// emitting any stdout assistant content.  Models an auth-failure-style crash.
fn write_stderr_death_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
LOG_DIR={log_dir}
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then prev=$(cat "$count_file"); echo $((prev + 1)) > "$count_file"; else echo 1 > "$count_file"; fi

# Read one NDJSON line, write a diagnostic to stderr, then exit without
# emitting any stdout.  Simulates auth-failure-style crashes where the CLI
# dies before producing any assistant content.
IFS= read -r line
echo "$line" >> "$LOG_DIR/received_lines"
echo "FATAL: simulated authentication failure (stub-marker-AUTH123)" 1>&2
exit 1
"#
    );
    write_script(dir, "claude", &body)
}

// ---------------------------------------------------------------------------
// Test C: surfaces_stderr_in_returned_err_body
//
// Expected failure (current): stderr is drained at debug level via tracing
// in a background task and never reaches the Err body.  complete() exhausts
// both spawn attempts internally, then returns
// `Err(ProviderError::Http { body: "...claude process exited unexpectedly" })`
// — opaque.  This test asserts the body contains the stderr marker; it will
// fail against the current code.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn surfaces_stderr_in_returned_err_body() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_stderr_death_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // Single complete() call: attempt 0 dies (stub writes stderr, exits),
    // attempt 1 re-spawns and also dies (same stub).  Loop exhausts both
    // attempts and returns Err(ProviderError::Http).
    let result = provider.complete(make_request(&provider, "probe")).await;

    let body = match result {
        Err(hone_providers::ProviderError::Http { body, .. }) => body,
        Ok(_) => panic!(
            "complete() must return Err when every spawn attempt dies before \
             producing a successful turn"
        ),
        Err(other) => {
            panic!("expected ProviderError::Http when all spawn attempts fail; got: {other:?}")
        }
    };

    assert!(
        body.contains("stub-marker-AUTH123"),
        "Err(ProviderError::Http {{ body }}) must contain the stderr marker \
         'stub-marker-AUTH123' so operators can diagnose auth failures; \
         got body: {body:?}"
    );

    // Both spawn attempts must have run.
    let spawn_count_str = std::fs::read_to_string(log_dir.join("spawn_count"))
        .expect("spawn_count file must exist after complete() call");
    let spawn_count: u32 = spawn_count_str
        .trim()
        .parse()
        .expect("spawn_count must be a number");
    assert_eq!(
        spawn_count, 2,
        "both spawn attempts must run when each dies before success; got spawn_count={spawn_count}"
    );
}

// ---------------------------------------------------------------------------
// Test D: surfaces_most_recent_stderr_after_respawn
//
// Each spawn writes a DIFFERENT stderr marker.  After the loop exhausts both
// attempts and returns Err, the body must contain the SECOND attempt's marker
// — the diagnostic from the spawn that actually decided the failure.  Without
// per-attempt stderr capture, the implementation might leak the first
// attempt's stderr or only show generic text.
//
// Expected failure (current): body contains neither marker (stderr never
// reaches it), so the assertion on the marker-B substring fails.
// ---------------------------------------------------------------------------

/// Stub that branches on spawn count: first invocation writes marker-A,
/// second writes marker-B.  Both exit before emitting any stdout content.
fn write_per_spawn_stderr_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
LOG_DIR={log_dir}
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then prev=$(cat "$count_file"); echo $((prev + 1)) > "$count_file"; else echo 1 > "$count_file"; fi
new_count=$(cat "$count_file")

IFS= read -r line
echo "$line" >> "$LOG_DIR/received_lines"

if [ "$new_count" = "1" ]; then
    echo "first-attempt-stderr (stub-marker-A-FIRST)" 1>&2
else
    echo "second-attempt-stderr (stub-marker-B-SECOND)" 1>&2
fi
exit 1
"#
    );
    write_script(dir, "claude", &body)
}

#[tokio::test]
async fn surfaces_most_recent_stderr_after_respawn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_per_spawn_stderr_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    let result = provider.complete(make_request(&provider, "probe")).await;

    let body = match result {
        Err(hone_providers::ProviderError::Http { body, .. }) => body,
        Ok(_) => panic!("complete() must return Err when every spawn dies before producing a turn"),
        Err(other) => panic!("expected ProviderError::Http; got: {other:?}"),
    };

    // The body MUST contain the second attempt's marker — that is the diagnostic
    // for the spawn whose failure decided the final Err.
    assert!(
        body.contains("stub-marker-B-SECOND"),
        "Err body must contain the most-recent attempt's stderr marker \
         'stub-marker-B-SECOND'; got body: {body:?}"
    );

    let spawn_count_str =
        std::fs::read_to_string(log_dir.join("spawn_count")).expect("spawn_count file must exist");
    let spawn_count: u32 = spawn_count_str.trim().parse().expect("number");
    assert_eq!(
        spawn_count, 2,
        "both spawn attempts must run; got spawn_count={spawn_count}"
    );
}

// ===========================================================================
// System-prompt passthrough contract (Tests SP-1 through SP-5)
//
// These tests define the contract for passing `Role::System` messages from
// `CompletionRequest` through to the `claude` CLI via the `--system-prompt`
// spawn-time flag.
//
// Key architectural requirements pinned by these tests:
//
//   1. The spawn is LAZY — it happens on the first `complete()` call, not in
//      `with_binary()`, so we can read `request.messages` before spawning.
//   2. `Role::System` messages are extracted before the process is spawned and
//      passed as `--system-prompt <value>`.  Multiple System messages are joined
//      with `\n\n` (two newlines, no trailing newline).
//   3. When there are no System messages, neither `--system-prompt` nor
//      `--append-system-prompt` appears in the spawn argv.
//   4. The system prompt is a per-session decision captured from the FIRST
//      `complete()` call.  Subsequent calls may carry different System messages
//      but those are silently dropped — the session is already running.
//
// ALL five tests are expected to FAIL against the current implementation
// because:
//   - `with_binary()` spawns eagerly, so the process is already alive before
//     `complete()` ever reads `request.messages`.
//   - `spawn_process_now()` does not accept a system-prompt argument and never
//     passes `--system-prompt` to the CLI.
// ===========================================================================

// ---------------------------------------------------------------------------
// Helper: write_argv_logging_claude
//
// Like `write_fake_claude` but logs each argv token on its own line (one arg
// per line) so tests can reliably detect `--system-prompt` and read its value
// without ambiguity.  The stub still responds to any number of NDJSON turns so
// multi-turn tests work normally.
//
// Spawn count and received_lines are tracked identically to `write_fake_claude`.
// ---------------------------------------------------------------------------

fn write_argv_logging_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
# Fake claude stub that logs each argv token on its own line.
LOG_DIR={log_dir}

# Log each argument on a separate line so tests can parse reliably.
printf '%s\n' "$@" >> "$LOG_DIR/argv_args"

# Capture the exact byte value of --system-prompt (if present) to a dedicated
# file.  Splitting argv via lines() in Rust strips '\n' and is lossy for
# multi-line values; a separate file preserves embedded newlines verbatim.
prev=""
for a in "$@"; do
    if [ "$prev" = "--system-prompt" ]; then
        printf '%s' "$a" > "$LOG_DIR/system_prompt_value"
        break
    fi
    prev="$a"
done

# Increment spawn counter.
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then
    prev=$(cat "$count_file")
    echo $((prev + 1)) > "$count_file"
else
    echo 1 > "$count_file"
fi

# Main loop: read one NDJSON line at a time and respond.
while IFS= read -r line; do
    [ -z "$line" ] && continue

    echo "$line" >> "$LOG_DIR/received_lines"

    printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"stub response"}}]}}}}\n'
    printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
done
"#
    );
    write_script(dir, "claude", &body)
}

// ---------------------------------------------------------------------------
// Test SP-1: system_prompt_from_messages_appears_in_spawn_argv
//
// When `request.messages` contains [System("you are a helpful assistant"),
// User("hi")], the spawned `claude` process argv must contain `--system-prompt`
// immediately followed by `"you are a helpful assistant"`.
//
// Expected failure (current): `spawn_process_now` never receives or passes
// `--system-prompt`, so `argv_args` will contain none of those tokens.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn system_prompt_from_messages_appears_in_spawn_argv() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    let messages = vec![
        Message::system("you are a helpful assistant"),
        Message::user("hi"),
    ];
    let request = make_request_with_history(&provider, messages);

    let events = tokio::time::timeout(std::time::Duration::from_secs(5), drain(&provider, request))
        .await
        .expect("complete() must resolve within 5 s");

    // The turn must have succeeded (Done event present).
    assert!(
        events.contains(&ProviderEvent::Done),
        "turn must complete successfully; got events: {events:?}"
    );

    // Read per-arg log.
    let argv_args = std::fs::read_to_string(log_dir.join("argv_args"))
        .expect("argv_args must be written by the stub after complete()");

    let args: Vec<&str> = argv_args.lines().collect();

    // Find `--system-prompt` in the arg list.
    let sys_prompt_pos = args.iter().position(|a| *a == "--system-prompt").expect(
        "spawn argv must contain '--system-prompt' when a System message is present; \
             got args: {args:?}",
    );

    // The token immediately after `--system-prompt` must be the system text.
    let sys_prompt_value = args
        .get(sys_prompt_pos + 1)
        .copied()
        .expect("'--system-prompt' must be followed by a value arg");

    assert_eq!(
        sys_prompt_value, "you are a helpful assistant",
        "system-prompt value must equal the System message text; got: {sys_prompt_value:?}"
    );
}

// ---------------------------------------------------------------------------
// Test SP-2: absent_system_prompt_means_no_system_prompt_flag
//
// When `request.messages` contains ONLY User messages (no System role),
// neither `--system-prompt` nor `--append-system-prompt` must appear in the
// spawn argv.
//
// This test also pins the lazy-spawn contract: `argv_args` must NOT exist
// before `complete()` is called.  The polling loop (same mechanism as SP-5)
// gives an eagerly-spawned script up to 1 s to write the file; if it does,
// the "no spawn before complete()" assertion fires immediately.
//
// Expected failure (current): `with_binary()` spawns eagerly, so `argv_args`
// and `spawn_count` are written before `complete()` is called.  The polling
// loop detects the file and the assertion fires.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn absent_system_prompt_means_no_system_prompt_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // Poll up to 1 second for argv_args to appear (same logic as SP-5).
    // An eager spawn will write the file quickly; a lazy one never writes it
    // here at all.
    let poll_deadline = std::time::Instant::now() + std::time::Duration::from_millis(1000);
    while std::time::Instant::now() < poll_deadline {
        if log_dir.join("argv_args").exists() || log_dir.join("spawn_count").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // Assert: no spawn must have occurred before complete() is called.
    let argv_exists_before = log_dir.join("argv_args").exists();
    assert!(
        !argv_exists_before,
        "spawn must be LAZY: argv_args must not exist before complete() is called; \
         it was written eagerly during with_binary()"
    );

    // No System message — only User.
    let request = make_request(&provider, "hello with no system prompt");

    tokio::time::timeout(std::time::Duration::from_secs(5), drain(&provider, request))
        .await
        .expect("complete() must resolve within 5 s");

    let argv_args = std::fs::read_to_string(log_dir.join("argv_args"))
        .expect("argv_args must be written by the stub after complete()");

    let args: Vec<&str> = argv_args.lines().collect();

    assert!(
        !args.contains(&"--system-prompt"),
        "spawn argv must NOT contain '--system-prompt' when no System message is present; \
         got args: {args:?}"
    );
    assert!(
        !args.contains(&"--append-system-prompt"),
        "spawn argv must NOT contain '--append-system-prompt' when no System message is present; \
         got args: {args:?}"
    );
}

// ---------------------------------------------------------------------------
// Test SP-3: multiple_system_messages_are_concatenated_with_blank_line
//
// When `request.messages` contains [System("first rule"), System("second rule"),
// User("hi")], the `--system-prompt` value passed to the CLI must be exactly
// `"first rule\n\nsecond rule"` — two newlines separating the two blocks, no
// trailing newline.
//
// Join policy rationale: a blank line is the conventional markdown separator
// between distinct instruction blocks.  Using a single `\n` risks lines
// bleeding into each other visually.  Using `\n\n\n` (three newlines) would
// create unnecessary vertical whitespace.  Two newlines (`\n\n`) is idiomatic.
//
// Expected failure (current): `spawn_process_now` never passes `--system-prompt`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn multiple_system_messages_are_concatenated_with_blank_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    let messages = vec![
        Message::system("first rule"),
        Message::system("second rule"),
        Message::user("hi"),
    ];
    let request = make_request_with_history(&provider, messages);

    tokio::time::timeout(std::time::Duration::from_secs(5), drain(&provider, request))
        .await
        .expect("complete() must resolve within 5 s");

    // Read the dedicated value file written by the stub: this preserves
    // embedded newlines that an argv_args.lines() parse would strip.
    let sys_prompt_value = std::fs::read_to_string(log_dir.join("system_prompt_value")).expect(
        "system_prompt_value must be written by the stub when '--system-prompt' \
             is present in argv",
    );

    assert_eq!(
        sys_prompt_value, "first rule\n\nsecond rule",
        "multiple System messages must be joined with '\\n\\n' (blank-line separator), \
         no trailing newline; got: {sys_prompt_value:?}"
    );
}

// ---------------------------------------------------------------------------
// Test SP-4: system_prompt_captured_only_on_first_spawn
//
// Call `complete()` twice on the same provider instance.  First call has
// System("rule A"), second call has System("rule B").
//
// Contract:
//   - Exactly ONE `claude` process is spawned (spawn_count == 1).
//   - That single spawn used "rule A" (the first call's system text).
//   - The second call does NOT re-spawn (system prompt is a per-session flag).
//
// This pins the "system prompt is session-scoped, not turn-scoped" semantics.
//
// Expected failure (current): no `--system-prompt` flag is ever passed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn system_prompt_captured_only_on_first_spawn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // First turn: System("rule A") + User("turn one").
    let req1 = make_request_with_history(
        &provider,
        vec![Message::system("rule A"), Message::user("turn one")],
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), drain(&provider, req1))
        .await
        .expect("turn 1 must complete within 5 s");

    // Second turn: System("rule B") + history + User("turn two").
    let req2 = make_request_with_history(
        &provider,
        vec![
            Message::system("rule B"),
            Message::user("turn one"),
            Message::assistant("stub response"),
            Message::user("turn two"),
        ],
    );
    tokio::time::timeout(std::time::Duration::from_secs(5), drain(&provider, req2))
        .await
        .expect("turn 2 must complete within 5 s");

    // Read spawn counter.
    let spawn_count_str = std::fs::read_to_string(log_dir.join("spawn_count"))
        .expect("spawn_count must exist after two complete() calls");
    let spawn_count: u32 = spawn_count_str
        .trim()
        .parse()
        .expect("spawn_count must be a number");

    assert_eq!(
        spawn_count, 1,
        "exactly one spawn must occur across both turns; got spawn_count={spawn_count}"
    );

    // The single spawn's argv must contain "rule A" as the system-prompt value.
    let argv_args = std::fs::read_to_string(log_dir.join("argv_args"))
        .expect("argv_args must exist after complete() calls");
    let args: Vec<&str> = argv_args.lines().collect();

    let sys_prompt_pos = args
        .iter()
        .position(|a| *a == "--system-prompt")
        .expect("spawn argv must contain '--system-prompt'; got args: {args:?}");

    let sys_prompt_value = args
        .get(sys_prompt_pos + 1)
        .copied()
        .expect("'--system-prompt' must be followed by a value arg");

    assert_eq!(
        sys_prompt_value, "rule A",
        "the single spawn must use the FIRST call's system prompt 'rule A', \
         not the second call's 'rule B'; got: {sys_prompt_value:?}"
    );

    // Confirm "rule B" does NOT appear in the argv (it would indicate a bad
    // re-spawn or per-turn re-injection).
    assert!(
        !argv_args.contains("rule B"),
        "second call's System message 'rule B' must never appear in spawn argv; \
         got argv_args: {argv_args:?}"
    );
}

// ===========================================================================
// MCP config passthrough contract (Tests MCP-1 through MCP-3)
//
// These tests define the contract for passing an MCP server configuration
// file path to the `claude` CLI via the `--mcp-config <path>` spawn-time flag.
//
// API design: chained builder method
//   `ClaudeCliProvider::with_binary(binary).with_mcp_config(path)`
//
// Rationale: the chained builder pattern is consistent with how `with_binary`
// is already used throughout the codebase and avoids combinatorial explosion
// if more optional flags are added later (vs a combined constructor).  The
// path type is `impl Into<PathBuf>` — idiomatic for filesystem paths in Rust
// and more type-safe than `impl Into<String>`.
//
// Key contracts:
//   1. `with_mcp_config(path)` causes `--mcp-config <path>` to appear in
//      every spawn's argv for this provider instance.
//   2. Providers constructed without `with_mcp_config` must NOT emit
//      `--mcp-config` or `--strict-mcp-config` in the spawn argv.
//   3. After a process death+re-spawn, the re-spawned process's argv must
//      still contain `--mcp-config <same path>` — the config is provider-level,
//      not turn-level.
//
// All three tests are expected to FAIL (compile-error) against the current
// implementation because `with_mcp_config` does not yet exist on
// `ClaudeCliProvider`.  The compile failure names the missing method clearly
// so rust-expert can prioritise adding the stub before other changes.
// ===========================================================================

// ---------------------------------------------------------------------------
// Test MCP-1: mcp_config_path_appears_in_spawn_argv
//
// When the provider is constructed with `.with_mcp_config("test/fake/mcp.json")`,
// the spawned `claude` process's argv must contain `--mcp-config` immediately
// followed by `"test/fake/mcp.json"`.  The path does not need to point to a
// real file — the fake claude stub never opens it.
//
// Expected failure: compile error — `no method named 'with_mcp_config' found
// for struct ClaudeCliProvider`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_config_path_appears_in_spawn_argv() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script).with_mcp_config("test/fake/mcp.json");

    let request = make_request(&provider, "probe mcp config");

    let events = tokio::time::timeout(std::time::Duration::from_secs(5), drain(&provider, request))
        .await
        .expect("complete() must resolve within 5 s");

    // The turn must have succeeded.
    assert!(
        events.contains(&ProviderEvent::Done),
        "turn must complete successfully; got events: {events:?}"
    );

    // Read the per-arg log file written by the stub.
    let argv_args = std::fs::read_to_string(log_dir.join("argv_args"))
        .expect("argv_args must be written by the stub after complete()");

    let args: Vec<&str> = argv_args.lines().collect();

    // Find `--mcp-config` in the arg list.
    let mcp_config_pos = args.iter().position(|a| *a == "--mcp-config").expect(
        "spawn argv must contain '--mcp-config' when provider is \
             constructed with with_mcp_config(); got args: {args:?}",
    );

    // The token immediately after `--mcp-config` must be the path value.
    let mcp_config_value = args
        .get(mcp_config_pos + 1)
        .copied()
        .expect("'--mcp-config' must be followed by a value arg");

    assert_eq!(
        mcp_config_value, "test/fake/mcp.json",
        "mcp-config value must equal the path supplied to with_mcp_config(); \
         got: {mcp_config_value:?}"
    );
}

// ---------------------------------------------------------------------------
// Test MCP-2: absent_mcp_config_means_no_flag
//
// When the provider is constructed via `new()` or `with_binary(...)` without
// calling `with_mcp_config()`, neither `--mcp-config` nor `--strict-mcp-config`
// must appear in the spawn argv.
//
// This also re-checks the lazy-spawn contract (reusing the same mechanism as
// SP-2): argv_args must not exist before complete() is called.
//
// Expected failure: does NOT compile-fail on its own (it only uses existing
// API), but will be blocked by the compile error in MCP-1 and MCP-3.
// If compiled in isolation, this test currently PASSES (no --mcp-config flag
// is emitted today), but when the full test file compiles after MCP-1/3 are
// implemented, this test must continue to pass.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn absent_mcp_config_means_no_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_claude(&log_dir);

    // Default constructor — no MCP config.
    let provider = ClaudeCliProvider::with_binary(&script);

    // Verify lazy spawn: argv_args must not exist before complete().
    let poll_deadline = std::time::Instant::now() + std::time::Duration::from_millis(1000);
    while std::time::Instant::now() < poll_deadline {
        if log_dir.join("argv_args").exists() || log_dir.join("spawn_count").exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let argv_exists_before = log_dir.join("argv_args").exists();
    assert!(
        !argv_exists_before,
        "spawn must be LAZY: argv_args must not exist before complete() is called; \
         got eagerly-written argv_args before any complete() call"
    );

    // Make a request with no MCP config set.
    let request = make_request(&provider, "probe no mcp");

    tokio::time::timeout(std::time::Duration::from_secs(5), drain(&provider, request))
        .await
        .expect("complete() must resolve within 5 s");

    let argv_args = std::fs::read_to_string(log_dir.join("argv_args"))
        .expect("argv_args must be written by the stub after complete()");

    let args: Vec<&str> = argv_args.lines().collect();

    assert!(
        !args.contains(&"--mcp-config"),
        "spawn argv must NOT contain '--mcp-config' when no MCP config was set; \
         got args: {args:?}"
    );
    assert!(
        !args.contains(&"--strict-mcp-config"),
        "spawn argv must NOT contain '--strict-mcp-config' when no MCP config was set; \
         got args: {args:?}"
    );
}

// ---------------------------------------------------------------------------
// Test MCP-3: mcp_config_persists_across_respawn
//
// When a process dies mid-session and the provider re-spawns, the re-spawned
// process's argv must still contain `--mcp-config <same path>`.  This pins
// the "MCP config is provider-level, not turn-level — survives re-spawn"
// contract.
//
// Mechanism: use the `write_oneshot_claude`/`write_argv_logging_claude` hybrid
// via `write_argv_logging_oneshot_claude` (defined below) which both logs argv
// on separate lines AND exits after a single turn.  After two turns, two
// subprocess invocations will have each appended their argv to `argv_args`.
// We then count occurrences of `--mcp-config` in the appended file — must be 2.
//
// The `write_argv_logging_claude` helper appends with `>>` on line
// `printf '%s\n' "$@" >> "$LOG_DIR/argv_args"` so all spawns' argvs are
// concatenated in order — both spawns' `--mcp-config` tokens are visible in
// the final file.
//
// Expected failure: compile error — `no method named 'with_mcp_config' found
// for struct ClaudeCliProvider`.
// ---------------------------------------------------------------------------

/// Write a one-shot stub that logs argv on separate lines (like
/// `write_argv_logging_claude`) but exits after a single NDJSON turn (like
/// `write_oneshot_claude`).  Each spawn appends its argv to `argv_args` and
/// increments `spawn_count`.
fn write_argv_logging_oneshot_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
# One-shot stub that logs argv per-arg and responds to one turn then exits.
LOG_DIR={log_dir}

# Log each argument on a separate line (appends across re-spawns).
printf '%s\n' "$@" >> "$LOG_DIR/argv_args"

# Increment spawn counter.
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then
    prev=$(cat "$count_file")
    echo $((prev + 1)) > "$count_file"
else
    echo 1 > "$count_file"
fi

# Read exactly one NDJSON line, respond, then exit.
IFS= read -r line
[ -z "$line" ] && exit 0

echo "$line" >> "$LOG_DIR/received_lines"

printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"stub response"}}]}}}}\n'
printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'

exit 0
"#
    );
    write_script(dir, "claude", &body)
}

#[tokio::test]
async fn mcp_config_persists_across_respawn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_oneshot_claude(&log_dir);

    // Provider with MCP config — process will die after each turn (oneshot stub).
    let provider = ClaudeCliProvider::with_binary(&script).with_mcp_config("test/fake/mcp.json");

    // Turn 1: child responds once, then exits.
    let turn1_events = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        drain(&provider, make_request(&provider, "turn one")),
    )
    .await
    .expect("turn 1 must complete within 5 s");

    assert!(
        turn1_events.contains(&ProviderEvent::Done),
        "turn 1 must produce Done; got events: {turn1_events:?}"
    );

    // Turn 2: first child is dead; provider must re-spawn.
    let turn2_result = provider.complete(make_request(&provider, "turn two")).await;

    let turn2_events: Vec<ProviderEvent> = match turn2_result {
        Ok(stream) => stream.collect::<Vec<_>>().await,
        Err(e) => {
            panic!(
                "turn 2 complete() must not return Err after process death — \
                 provider must re-spawn with the same MCP config; got Err: {e:?}"
            );
        }
    };

    assert!(
        turn2_events.contains(&ProviderEvent::Done),
        "turn 2 must produce Done after re-spawn; got events: {turn2_events:?}"
    );

    // Both spawns must have run.
    let spawn_count_str = std::fs::read_to_string(log_dir.join("spawn_count"))
        .expect("spawn_count must exist after two turns");
    let spawn_count: u32 = spawn_count_str
        .trim()
        .parse()
        .expect("spawn_count must be a number");
    assert_eq!(
        spawn_count, 2,
        "two spawns must have occurred (one per one-shot turn); got spawn_count={spawn_count}"
    );

    // Read the concatenated argv_args from both spawns.
    let argv_args = std::fs::read_to_string(log_dir.join("argv_args"))
        .expect("argv_args must exist after two complete() calls");

    // Count occurrences of `--mcp-config` across both spawns' appended argv lines.
    let mcp_config_count = argv_args
        .lines()
        .filter(|line| *line == "--mcp-config")
        .count();

    assert_eq!(
        mcp_config_count, 2,
        "both spawns must include '--mcp-config' in their argv (once each); \
         found {mcp_config_count} occurrences in argv_args:\n{argv_args}"
    );

    // Verify the path value appears twice as well (once per spawn).
    let path_value_count = argv_args
        .lines()
        .filter(|line| *line == "test/fake/mcp.json")
        .count();

    assert_eq!(
        path_value_count, 2,
        "both spawns must have 'test/fake/mcp.json' as the --mcp-config value; \
         found {path_value_count} occurrences in argv_args:\n{argv_args}"
    );
}

// ===========================================================================
// Strict-MCP-config flag contract (Tests MCP-strict-1 and MCP-strict-2)
//
// `--strict-mcp-config` is a boolean spawn flag added to the `claude` CLI:
// when present it instructs the CLI to use ONLY the `--mcp-config` file and
// ignore all other MCP server sources.  No value follows the flag.
//
// API design: `pub fn with_strict_mcp_config(mut self) -> Self`
//   - Chain-style builder, takes no parameter (boolean toggle).
//   - When called, an internal `strict_mcp_config: bool` field (defaulting to
//     `false` in `with_binary`) flips to `true`.
//   - Idempotent: calling it multiple times is equivalent to calling it once.
//   - Intended to be combined with `with_mcp_config(path)` — strict mode is
//     meaningless without a config file, but the tests do not enforce the
//     combination at the type level (it is a logical constraint for the user).
//
// MCP-strict-1: verifies the flag appears when `with_strict_mcp_config()` is
//               chained after `with_mcp_config()`.
// MCP-strict-2: verifies the flag is absent (opt-in) when `with_strict_mcp_config()`
//               is NOT called, even when `with_mcp_config()` IS called.
//
// Both tests are expected to FAIL as compile errors because
// `with_strict_mcp_config` does not yet exist on `ClaudeCliProvider`.
// ===========================================================================

// ---------------------------------------------------------------------------
// Test MCP-strict-1: strict_mcp_config_flag_appears_in_spawn_argv
//
// Provider constructed via:
//   ClaudeCliProvider::with_binary(script)
//       .with_mcp_config("test/fake/mcp.json")
//       .with_strict_mcp_config()
//
// The spawned `claude` process's argv must contain `--strict-mcp-config` as a
// standalone token (i.e. it appears as its own line in `argv_args`).
//
// The `--mcp-config` flag must also be present (verified separately so the
// test checks both flags together in the realistic combined use-case).
//
// Expected failure: compile error — `no method named 'with_strict_mcp_config'
// found for struct ClaudeCliProvider`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn strict_mcp_config_flag_appears_in_spawn_argv() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_claude(&log_dir);

    // Chain with_mcp_config first, then with_strict_mcp_config on top.
    let provider = ClaudeCliProvider::with_binary(&script)
        .with_mcp_config("test/fake/mcp.json")
        .with_strict_mcp_config();

    let request = make_request(&provider, "probe strict mcp");

    let events = tokio::time::timeout(std::time::Duration::from_secs(5), drain(&provider, request))
        .await
        .expect("complete() must resolve within 5 s");

    // The turn must have succeeded (Done event present).
    assert!(
        events.contains(&ProviderEvent::Done),
        "turn must complete successfully; got events: {events:?}"
    );

    // Read the per-arg log file written by the stub.
    let argv_args = std::fs::read_to_string(log_dir.join("argv_args"))
        .expect("argv_args must be written by the stub after complete()");

    let args: Vec<&str> = argv_args.lines().collect();

    // `--strict-mcp-config` must appear as a standalone token.
    assert!(
        args.contains(&"--strict-mcp-config"),
        "spawn argv must contain '--strict-mcp-config' as a standalone token when \
         provider is constructed with with_strict_mcp_config(); got args: {args:?}"
    );

    // `--mcp-config` must also be present (realistic combined use).
    assert!(
        args.contains(&"--mcp-config"),
        "spawn argv must still contain '--mcp-config' alongside '--strict-mcp-config'; \
         got args: {args:?}"
    );
}

// ---------------------------------------------------------------------------
// Test MCP-strict-2: absent_strict_mcp_config_means_no_flag
//
// Provider constructed via:
//   ClaudeCliProvider::with_binary(script).with_mcp_config("test/fake/mcp.json")
//   (i.e. with_strict_mcp_config() is NOT called)
//
// Neither `--strict-mcp-config` nor any variant must appear in the spawn argv.
// This verifies the flag is strictly opt-in and defaults to off.
//
// Expected failure: blocked from running by the compile error in MCP-strict-1.
// When compiled in isolation this test would pass today (the flag is never
// emitted by the current implementation), but it is included to pin the
// "default off" contract explicitly.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn absent_strict_mcp_config_means_no_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_claude(&log_dir);

    // Constructed with with_mcp_config but WITHOUT with_strict_mcp_config.
    let provider = ClaudeCliProvider::with_binary(&script).with_mcp_config("test/fake/mcp.json");

    let request = make_request(&provider, "probe no strict mcp");

    tokio::time::timeout(std::time::Duration::from_secs(5), drain(&provider, request))
        .await
        .expect("complete() must resolve within 5 s");

    let argv_args = std::fs::read_to_string(log_dir.join("argv_args"))
        .expect("argv_args must be written by the stub after complete()");

    let args: Vec<&str> = argv_args.lines().collect();

    // `--strict-mcp-config` must NOT appear when with_strict_mcp_config() was
    // not called — the flag is opt-in, default off.
    assert!(
        !args.contains(&"--strict-mcp-config"),
        "spawn argv must NOT contain '--strict-mcp-config' when with_strict_mcp_config() \
         was not called; got args: {args:?}"
    );
}

// ===========================================================================
// Lazy-streaming + drop-cleanup contract (Tests LZ-1 through LZ-3)
//
// These tests pin the contract for the upcoming "true lazy streaming" refactor
// of `ClaudeCliProvider::complete()`.
//
// Today, `complete()` synchronously collects ALL events from the CLI child
// into a `Vec<ProviderEvent>` and then returns `stream::iter(events)`.  The
// stream is just an iterator over a pre-collected Vec — by the time the caller
// first polls `.next()`, all work is already done.  This blocks mid-stream
// cancellation and forces callers to wait for `result.success` before seeing
// the first token.
//
// Target contract:
//   1. `complete()` performs the WRITE (NDJSON → stdin) with the existing
//      2-attempt retry-on-write-failure logic synchronously.
//   2. Once the write succeeds, `Ok(stream)` is returned immediately — before
//      any output line is read.
//   3. The returned stream lazily reads stdout on each `poll_next` call.
//   4. The stream owns the lock on the process slot for its entire lifetime
//      (so two concurrent `complete()` calls never interleave I/O).
//   5. When the stream is dropped before yielding `Done`, the process is
//      tombstoned (next `complete()` re-spawns) and the lock is released.
//   6. When the stream completes naturally (yields `Done`), the lock is
//      released cleanly without tombstoning.
//
// LZ-1: events_yield_lazily_not_all_at_once
//   First token arrives before the CLI has emitted the success line.
//   Impossible if the implementation eagerly drains the whole turn into a Vec.
//
// LZ-2: dropping_unpoll_stream_releases_mutex_for_next_call
//   Dropping the stream without polling must release the lock so the next
//   `complete()` call is not deadlocked.
//
// LZ-3: dropping_stream_mid_turn_tombstones_process
//   Abandoning a stream mid-turn must tombstone the process so the next
//   `complete()` re-spawns (rather than writing into a process that still has
//   unread bytes from the abandoned turn in its stdout buffer).
//
// LZ-4 (SKIPPED): natural_completion_does_not_tombstone_process
//   This contract is already pinned by
//   `spawns_cli_once_across_multiple_complete_calls` (Test 1), which asserts
//   `spawn_count == 1` after two full turns — conclusive proof that natural
//   completion leaves the process alive.  Adding a redundant LZ-4 would
//   increase test runtime without adding coverage.
//
// ALL three tests are expected to FAIL against today's eager-collection
// implementation (compile-pass, runtime-fail).
// ===========================================================================

// ---------------------------------------------------------------------------
// Helper: write_lazy_proof_claude
//
// Writes a fake `claude` stub that:
//   1. Touches `<log_dir>/wait_started` (signals: we are about to block).
//   2. Emits ONE assistant token line immediately.
//   3. Busy-waits (50 ms sleep loop) until `<log_dir>/proceed` exists.
//   4. Touches `<log_dir>/wait_finished` (signals: wait loop is done).
//   5. Emits the `result.subtype == "success"` line.
//
// The stub also increments `spawn_count` and responds to additional NDJSON
// lines after the first (for multi-turn uses).
//
// Marker files summary:
//   wait_started  — written before the busy-wait begins; used as a signal
//                   that the first token has been emitted and the stub is now
//                   blocking.
//   wait_finished — written after the busy-wait exits; the test asserts this
//                   does NOT exist when the first token has been yielded but
//                   the `proceed` sentinel has not yet been touched.
//   proceed       — written by the test to unblock the stub; its presence
//                   causes the while-loop exit.
// ---------------------------------------------------------------------------

fn write_lazy_proof_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
# Lazy-proof stub: emits one token, blocks until <log_dir>/proceed exists,
# then emits the success result.  Subsequent NDJSON lines get normal responses.
LOG_DIR={log_dir}

# Increment spawn counter.
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then
    prev=$(cat "$count_file")
    echo $((prev + 1)) > "$count_file"
else
    echo 1 > "$count_file"
fi

# Record argv.
echo "$@" >> "$LOG_DIR/argv"

# Main loop: read one NDJSON line at a time and respond.
first=1
while IFS= read -r line; do
    [ -z "$line" ] && continue
    echo "$line" >> "$LOG_DIR/received_lines"

    if [ "$first" = "1" ]; then
        first=0

        # Emit the first token immediately (before blocking).
        printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"lazy token"}}]}}}}\n'

        # Signal that we are about to wait.
        touch "$LOG_DIR/wait_started"

        # Busy-wait until the test touches the proceed sentinel.
        while [ ! -f "$LOG_DIR/proceed" ]; do
            sleep 0.05
        done

        # Signal that the wait is over.
        touch "$LOG_DIR/wait_finished"

        # Now emit the success result.
        printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
    else
        # Subsequent turns: normal immediate response.
        printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"stub response"}}]}}}}\n'
        printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
    fi
done
"#
    );
    write_script(dir, "claude", &body)
}

// ---------------------------------------------------------------------------
// Test LZ-1: events_yield_lazily_not_all_at_once
//
// The fake `claude` script emits one assistant token, then blocks until a
// sentinel file appears, then emits the success result.
//
// The test:
//   1. Calls `complete()` → gets `Ok(stream)`.
//   2. Polls `stream.next().await` → must yield the first Token quickly (under
//      500 ms), proving the stream returned before the success line was read.
//   3. Asserts that `wait_finished` does NOT exist (the stub is still blocking
//      inside its busy-wait — the success line has not been emitted yet).
//   4. Touches the sentinel → `proceed` file exists.
//   5. Drains the rest of the stream to completion.
//
// Expected failure (current): today's `complete()` eagerly drains stdout in
// a loop until it sees `result.success`, so by the time `Ok(stream)` is
// returned, `wait_finished` already exists and the first assertion fires.
// The test is also likely to time out on step 2 (the stream was already fully
// collected, so polling for more events blocks forever on an exhausted iterator
// — actually it would return immediately from the pre-collected Vec, so the
// failure is the `wait_finished` assertion on step 3 being true).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn events_yield_lazily_not_all_at_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_lazy_proof_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);
    let request = make_request(&provider, "lazy test");

    // Step 1: complete() must return Ok(stream) before the success line.
    // Give it 5 s — the write is synchronous so this should be nearly instant.
    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        provider.complete(request),
    )
    .await
    .expect("complete() must return within 5 s")
    .expect("complete() must return Ok(stream)");

    // Step 2: the first poll must yield a Token quickly.
    // The stub emits the token before its busy-wait, so this must arrive in
    // well under 500 ms on any reasonably loaded machine.
    let first_event = tokio::time::timeout(std::time::Duration::from_millis(500), stream.next())
        .await
        .expect(
            "first Token must be yielded within 500 ms — if this times out the stream is not lazy",
        )
        .expect("stream must yield at least one event");

    assert!(
        matches!(first_event, ProviderEvent::Token { .. }),
        "first event must be a Token; got: {first_event:?}"
    );

    // Step 3: assert the stub is still inside its busy-wait — i.e. the success
    // line has NOT been emitted yet (wait_finished must not exist).
    // Give the shell a moment to write wait_started so we know it started the
    // wait (not that it skipped it entirely).
    let wait_started_deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while !log_dir.join("wait_started").exists()
        && std::time::Instant::now() < wait_started_deadline
    {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        log_dir.join("wait_started").exists(),
        "stub must have touched wait_started (confirmed the first token was emitted \
         and the busy-wait began); check write_lazy_proof_claude body"
    );
    assert!(
        !log_dir.join("wait_finished").exists(),
        "wait_finished must NOT exist yet — the stub is still blocking, meaning the \
         success line has not been emitted.  If this fails, complete() is eagerly \
         reading all stdout (not lazy): it drained the whole turn before returning."
    );

    // Step 4: unblock the stub by writing the proceed sentinel.
    std::fs::write(log_dir.join("proceed"), b"").expect("failed to write proceed sentinel");

    // Step 5: drain the rest of the stream; must complete within 5 s.
    let remaining: Vec<ProviderEvent> = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.collect::<Vec<_>>(),
    )
    .await
    .expect("remaining stream must drain within 5 s after proceed sentinel is written");

    // The drained events must include Done.
    assert!(
        remaining.contains(&ProviderEvent::Done),
        "stream must yield Done after success line is read; got remaining: {remaining:?}"
    );
}

// ---------------------------------------------------------------------------
// Test LZ-2: dropping_unpoll_stream_releases_mutex_for_next_call
//
// Steps:
//   1. Call `complete()` → get `Ok(stream1)`.
//   2. Drop `stream1` immediately, without polling at all.
//   3. Call `complete()` again → must NOT deadlock.
//   4. Drain the second stream to completion within 5 s.
//   5. Assert the second turn's events contain Done.
//
// Expected failure (current): today's `complete()` acquires the tokio Mutex
// guard at the top of the function and only releases it via the explicit
// `drop(guard)` at the very end (line 807 of claude_cli.rs), AFTER the events
// Vec has been fully populated.  When `stream1` is dropped, the guard is
// already gone (it was dropped at line 807 before `Ok(stream)` was returned),
// so today's implementation accidentally passes this test for the wrong reason
// — the guard is not inside the stream at all.  After the refactor, the guard
// will move into the stream struct so that it is released on stream drop; the
// test verifies this works correctly.
//
// For the current (eager) implementation this test may PASS (because the guard
// is already out of the stream), which is acceptable — we are writing tests
// that FAIL against the lazy implementation's intermediate state where the guard
// might be held incorrectly, and they must PASS against the completed
// implementation.  The test is here to catch any regression where the guard
// leaks into the stream and is never released.
//
// The critical failure mode is: if the implementer holds the guard inside the
// stream but does not release it on drop, step 3 deadlocks.  The 5 s timeout
// surfaces this as a timeout error, not a panic.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dropping_unpoll_stream_releases_mutex_for_next_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    // Use the standard fake claude (multi-turn, normal responses).
    let script = write_fake_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // Step 1: get stream1.
    let stream1 = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        provider.complete(make_request(&provider, "first turn")),
    )
    .await
    .expect("first complete() must return within 5 s")
    .expect("first complete() must return Ok(stream)");

    // Step 2: drop stream1 without polling.
    drop(stream1);

    // Step 3 + 4: second complete() must NOT deadlock.
    // If the mutex guard leaked into stream1 and was not released on drop,
    // this call will block until the 5 s timeout fires.
    let second_events = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let stream2 = provider
            .complete(make_request(&provider, "second turn"))
            .await
            .expect(
                "second complete() must return Ok(stream) — if this deadlocks, \
                         the stream is not releasing the mutex guard on drop",
            );
        stream2.collect::<Vec<ProviderEvent>>().await
    })
    .await
    .expect(
        "second complete() + drain must finish within 5 s; \
         a timeout here means the mutex guard from stream1 was not released on drop \
         (deadlock: the second complete() call cannot acquire the process lock)",
    );

    // Step 5: assert second turn produced valid events.
    assert!(
        second_events.contains(&ProviderEvent::Done),
        "second turn must produce Done; got: {second_events:?}"
    );
}

// ---------------------------------------------------------------------------
// Test LZ-3: dropping_stream_mid_turn_tombstones_process
//
// Steps:
//   1. Call `complete()` → get `stream1`.
//   2. Poll exactly ONE event from `stream1` (the first Token).
//   3. Drop `stream1` mid-turn without polling for Done.
//   4. Call `complete()` again → drain to completion.
//   5. Assert `spawn_count == 2` — the first process was tombstoned because
//      the consumer abandoned its turn; the second `complete()` had to
//      re-spawn.
//
// Rationale: when a stream is dropped before yielding Done, the process is
// in an unknown state — its stdout still has the rest of the turn's output
// buffered.  If the next `complete()` wrote a new NDJSON line into that
// process, the responses would interleave with the leftover bytes from the
// abandoned turn, producing garbage.  The correct fix is to tombstone the
// process on mid-turn drop so the next `complete()` gets a clean slate.
//
// Expected failure (current): today's implementation eagerly collects the
// ENTIRE turn into a Vec before returning `Ok(stream)`.  Dropping the stream
// after one poll just drops a Vec iterator — there is no interaction with the
// live process.  So `spawn_count == 1` after two turns (no tombstone, no
// re-spawn), and the assert fires.
//
// Implementation hint: the lazy stream's Drop impl should check whether the
// stream reached `Done`.  If not, it should tombstone the process slot (set it
// to `None`) before releasing the owned mutex guard.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dropping_stream_mid_turn_tombstones_process() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    // Use the standard fake claude: multi-turn capable.
    let script = write_fake_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // Step 1: get stream1.
    let mut stream1 = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        provider.complete(make_request(&provider, "first turn")),
    )
    .await
    .expect("first complete() must return within 5 s")
    .expect("first complete() must return Ok(stream)");

    // Step 2: poll exactly one event.
    let first_event = tokio::time::timeout(std::time::Duration::from_secs(5), stream1.next())
        .await
        .expect("first event must be yielded within 5 s")
        .expect("stream must yield at least one event before Done");

    // The event should be a Token (the fake claude emits an assistant line first).
    assert!(
        !matches!(first_event, ProviderEvent::Done),
        "first event must not be Done — we need to poll mid-turn; got: {first_event:?}"
    );

    // Step 3: drop stream1 mid-turn.
    drop(stream1);

    // Give the drop impl a brief moment to tombstone the process.  The tokio
    // executor needs a yield point to run any cleanup logic.
    tokio::task::yield_now().await;

    // Step 4: second complete() → drain.  This must re-spawn because the first
    // process was tombstoned.  Use a separate timeout so a deadlock (if the
    // drop did not release the guard) times out clearly.
    let second_events = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let stream2 = provider
            .complete(make_request(&provider, "second turn"))
            .await
            .expect("second complete() must return Ok(stream)");
        stream2.collect::<Vec<ProviderEvent>>().await
    })
    .await
    .expect(
        "second complete() + drain must finish within 5 s; \
         if this times out, the mid-turn drop did not release the mutex guard",
    );

    assert!(
        second_events.contains(&ProviderEvent::Done),
        "second turn must produce Done; got: {second_events:?}"
    );

    // Step 5: assert spawn_count == 2 — tombstone triggered a re-spawn.
    let spawn_count_str = std::fs::read_to_string(log_dir.join("spawn_count"))
        .expect("spawn_count must exist after two complete() calls");
    let spawn_count: u32 = spawn_count_str
        .trim()
        .parse()
        .expect("spawn_count must be a number");

    assert_eq!(
        spawn_count, 2,
        "dropping a stream mid-turn must tombstone the process so the next \
         complete() re-spawns; expected spawn_count == 2, got {spawn_count}.  \
         If spawn_count == 1, the drop did not tombstone (the process was reused \
         despite having unread stdout bytes from the abandoned turn)."
    );
}

// ===========================================================================
// Explicit CancellationToken contract (Tests CT-1 and CT-2)
//
// These tests pin the contract for the new `cancel: Option<CancellationToken>`
// field on `CompletionRequest`.
//
// API:
//   `CompletionRequest::cancel: Option<tokio_util::sync::CancellationToken>`
//   Defaults to `None` in `CompletionRequest::new`.
//
// Contracts:
//   CT-1  Mid-flight cancellation (Option A): the consumer cancels the token
//         after `complete()` returns `Ok(stream)` but before the stream yields
//         all events.  The stream must terminate with `[..., Error, Done]`.
//         The Error message must contain "cancel" (case-insensitive).
//
//   CT-2  Cancel mid-stream tombstones the process so the NEXT `complete()`
//         call re-spawns a fresh child.  Assert `spawn_count == 2`.
//
//   CT-3  SKIPPED — `spawns_cli_once_across_multiple_complete_calls` (Test 1)
//         already pins the no-cancel happy path with `cancel: None` (two turns,
//         spawn_count == 1).  Writing a duplicate test here adds no coverage.
//
//   CT-4  SKIPPED — CT-2 already exercises the guard-release path: if
//         spawn_count == 2 then the second `complete()` succeeded, which proves
//         the guard was released after cancellation.
//
// These tests will compile-fail until `CompletionRequest::cancel` is added
// (the field does not exist yet).  The compile error itself is the expected
// "red" signal.
// ===========================================================================

// ---------------------------------------------------------------------------
// Helper: write_cancel_proof_claude
//
// A stub that:
//   1. Increments spawn_count and logs argv (standard bookkeeping).
//   2. For the first NDJSON input line:
//      a. Emits ONE assistant Token immediately (so `complete()` returns and
//         the test can cancel before the stream drains the success line).
//      b. Sleeps for 10 seconds (plenty of time for the test to cancel).
//      c. Emits the success result (never reached in the cancellation test).
//   3. For subsequent NDJSON lines: standard immediate response (so the
//      post-cancellation second turn in CT-2 succeeds).
//
// This shape (emit one token quickly, then block) corresponds to Option A in
// the spec: the first-line eager read in `complete()` returns fast because
// the token is emitted before the sleep, so `complete()` itself completes
// without blocking.  The test then cancels the token, and the lazy poll of
// the *next* event observes the cancellation.
// ---------------------------------------------------------------------------

fn write_cancel_proof_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
# Cancel-proof stub: emits one token quickly, sleeps long enough for the
# test to cancel, then emits the success result (unreachable in practice).
# Subsequent NDJSON turns get normal immediate responses so CT-2 works.
LOG_DIR={log_dir}

# Increment spawn counter.
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then
    prev=$(cat "$count_file")
    echo $((prev + 1)) > "$count_file"
else
    echo 1 > "$count_file"
fi

# Record argv.
echo "$@" >> "$LOG_DIR/argv"

# Main loop: read one NDJSON line at a time and respond.
first=1
while IFS= read -r line; do
    [ -z "$line" ] && continue
    echo "$line" >> "$LOG_DIR/received_lines"

    if [ "$first" = "1" ]; then
        first=0

        # Emit one token immediately so complete() can return Ok(stream).
        printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"pre-cancel token"}}]}}}}\n'

        # Block long enough for the test to call token.cancel().
        sleep 10

        # This line is never reached in CT-1 (the stream is cancelled first).
        printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
    else
        # Subsequent turns: immediate normal response so CT-2 can verify
        # the second complete() call after cancellation works.
        printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"post-cancel response"}}]}}}}\n'
        printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
    fi
done
"#
    );
    write_script(dir, "claude", &body)
}

// ---------------------------------------------------------------------------
// Test CT-1: cancel_before_first_poll_yields_error_then_done
//
// Option A: the stub emits one token immediately, then sleeps for 10 s.
// `complete()` reads the first line (the token) synchronously and returns
// `Ok(stream)`.  The test cancels the token *before* polling the stream.
// The first `.next()` call yields the buffered Token that was already read;
// the second `.next()` observes the cancellation and yields Error + Done.
//
// Contract:
//   - The stream terminates within 5 s despite the 10 s sleep in the stub.
//   - The final two events are Error (message contains "cancel",
//     case-insensitive) and Done, in that order.
//
// Expected failure: compile error — `CompletionRequest` has no `cancel` field.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_before_first_poll_yields_error_then_done() {
    use tokio_util::sync::CancellationToken;

    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    // The cancel-proof stub: emits one token, then sleeps 10 s.
    let script = write_cancel_proof_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // Build a request with a cancellation token attached.
    let token = CancellationToken::new();
    let mut request = make_request(&provider, "cancellation test");
    // This line is expected to compile-fail until the `cancel` field exists:
    request.cancel = Some(token.clone());

    // complete() performs the NDJSON write synchronously and reads the first
    // output line (the Token) before returning Ok(stream).  It must return
    // in well under 5 s even though the stub will then sleep for 10 s.
    let stream = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        provider.complete(request),
    )
    .await
    .expect("complete() must return within 5 s — the stub emits the first token quickly")
    .expect("complete() must return Ok(stream)");

    // Cancel the token before polling the stream.
    // The first-line token was already read into the stream's internal buffer
    // by the eager-read in complete(), so:
    //   - poll 1: yields the buffered Token (pre-cancel token)
    //   - poll 2: observes cancellation → yields Error
    //   - poll 3: yields Done
    //   - poll 4: yields None (stream ended)
    token.cancel();

    // Drain the entire stream within 5 s.  If cancellation is not observed,
    // the stream would block for 10 s waiting for the stub's sleep to finish
    // — the timeout will fire and the test fails with a timeout, not a panic.
    let events: Vec<ProviderEvent> = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream.collect::<Vec<_>>(),
    )
    .await
    .expect(
        "stream must terminate within 5 s after cancellation; \
         a timeout means the stream is not observing the CancellationToken",
    );

    // The stream must contain the pre-buffered Token (read before complete()
    // returned), the Error, and Done — at least three events.  This proves
    // that pre-buffered events are flushed even when the token was cancelled
    // before the first poll.
    assert!(
        events.len() >= 3,
        "stream must yield pre-buffered Token + Error + Done; got: {events:?}"
    );

    // The first event must be the pre-buffered Token (text emitted by the
    // stub before it began sleeping).  If the implementation skipped the
    // pre-buffered flush on already-cancelled tokens, this assertion catches it.
    match &events[0] {
        ProviderEvent::Token { .. } => {}
        other => panic!(
            "first event must be the pre-buffered Token (flushed before cancel \
             takes effect); got: {other:?}; all events: {events:?}"
        ),
    }

    // The second-to-last event must be an Error whose message contains "cancel".
    let second_to_last = &events[events.len() - 2];
    match second_to_last {
        ProviderEvent::Error { message } => {
            assert!(
                message.to_lowercase().contains("cancel"),
                "Error message must contain 'cancel' (case-insensitive) to indicate \
                 cancellation was the cause; got message: {message:?}"
            );
        }
        other => {
            panic!(
                "second-to-last event must be Error (cancellation); \
                 got: {other:?}; all events: {events:?}"
            );
        }
    }

    // The last event must be Done.
    assert_eq!(
        events.last(),
        Some(&ProviderEvent::Done),
        "last event must be Done; got: {events:?}"
    );
}

// ---------------------------------------------------------------------------
// Test CT-2: cancel_mid_stream_tombstones_process_for_next_call
//
// Steps:
//   1. Build a cancel-proof stub (emits one token quickly, then sleeps 10 s).
//   2. First turn: pass `cancel: Some(token.clone())`.  Call `complete()` →
//      poll the first event (the Token) → cancel the token → drain remaining
//      events → expect terminating [... Error, Done].
//   3. Second turn: NEW CompletionRequest with `cancel: None` (no token).
//      Call `complete()` → drain → must succeed (Done event present, no Error
//      from cancellation).
//   4. Assert `spawn_count == 2` — the cancelled turn tombstoned the process;
//      the second turn triggered a re-spawn.
//
// Why spawn_count == 2?  The cancel-proof stub's first invocation sleeps for
// 10 s after emitting the token.  When the consumer cancels, the stream must
// tombstone the process (because the turn did not reach Done naturally).
// The second complete() sees a tombstoned slot and re-spawns the binary.
// The second invocation responds immediately to a new NDJSON line.
//
// Expected failure: compile error — `CompletionRequest` has no `cancel` field.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_mid_stream_tombstones_process_for_next_call() {
    use tokio_util::sync::CancellationToken;

    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    // The cancel-proof stub handles two spawns: first sleeps, second responds
    // immediately (the `else` branch in the stub's `while` loop handles
    // subsequent NDJSON lines on the same invocation, but since the first
    // invocation is cancelled/tombstoned, the second complete() will re-spawn
    // the stub, which starts a fresh invocation — its `first` flag is 1 again,
    // but we feed it a fresh NDJSON line so it goes into the `else` branch on
    // the SAME fresh process which won't sleep).
    //
    // Wait — the cancel-proof stub's second-and-beyond lines get immediate
    // responses (the `else` branch).  After cancellation, the first process is
    // tombstoned (killed or stdin closed).  The second complete() re-spawns a
    // new child, which starts fresh with `first=1`.  We need the second child
    // to respond quickly WITHOUT sleeping.  Use write_fake_claude for the
    // second turn; but since both turns use the same script path, we can't
    // switch stubs mid-test.
    //
    // Solution: use the cancel-proof stub for both.  After the first child is
    // tombstoned (killed by the Drop impl or by SIGPIPE on stdin close), the
    // second spawn starts fresh.  The second child reads the new NDJSON line,
    // sees `first=1`, emits the token, THEN sleeps.  But we don't cancel the
    // second call — we just need it to complete.  The 10 s sleep would block
    // the test.
    //
    // Better approach: use a stub that sleeps only on the very first spawn
    // (checking spawn_count == 1), and on subsequent spawns responds normally.
    let script = write_cancel_proof_respawning_claude(&log_dir);

    let provider = ClaudeCliProvider::with_binary(&script);

    // --- First turn: cancel mid-stream ---
    let token = CancellationToken::new();
    let mut first_request = make_request(&provider, "first turn — will be cancelled");
    // Compile-fail until the field exists:
    first_request.cancel = Some(token.clone());

    let mut stream1 = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        provider.complete(first_request),
    )
    .await
    .expect("first complete() must return within 5 s")
    .expect("first complete() must return Ok(stream)");

    // Poll one event (the eager first token).
    let first_event = tokio::time::timeout(std::time::Duration::from_secs(5), stream1.next())
        .await
        .expect("first event must be yielded within 5 s")
        .expect("stream must yield at least one event");

    assert!(
        matches!(first_event, ProviderEvent::Token { .. }),
        "first event must be a Token; got: {first_event:?}"
    );

    // Cancel mid-stream.
    token.cancel();

    // Drain the remaining events — must terminate quickly with Error + Done.
    let remaining1: Vec<ProviderEvent> = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        stream1.collect::<Vec<_>>(),
    )
    .await
    .expect(
        "first stream must terminate within 5 s after cancellation; \
         a timeout means cancellation is not observed",
    );

    // The final two events of the remaining events must be Error + Done.
    assert!(
        remaining1.len() >= 2,
        "remaining events after cancellation must include at least [Error, Done]; \
         got: {remaining1:?}"
    );

    let second_to_last = &remaining1[remaining1.len() - 2];
    assert!(
        matches!(second_to_last, ProviderEvent::Error { .. }),
        "second-to-last remaining event must be Error; got: {second_to_last:?}; \
         remaining: {remaining1:?}"
    );
    assert_eq!(
        remaining1.last(),
        Some(&ProviderEvent::Done),
        "last remaining event must be Done; got: {remaining1:?}"
    );

    // Allow the Drop impl to tombstone the process.
    tokio::task::yield_now().await;

    // --- Second turn: no cancel token, must re-spawn and succeed ---
    let second_request = make_request(&provider, "second turn — no cancellation");
    // Deliberately leave cancel: None (the default from CompletionRequest::new).

    let second_events = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let stream2 = provider
            .complete(second_request)
            .await
            .expect("second complete() must return Ok(stream) after re-spawn");
        stream2.collect::<Vec<ProviderEvent>>().await
    })
    .await
    .expect(
        "second complete() + drain must finish within 5 s; \
         a timeout or panic here means the re-spawn after cancellation failed",
    );

    // The second turn must succeed cleanly.
    assert!(
        second_events.contains(&ProviderEvent::Done),
        "second turn must produce Done; got: {second_events:?}"
    );

    // No Error event from cancellation must appear in the second turn.
    let has_cancel_error = second_events.iter().any(|e| {
        matches!(e, ProviderEvent::Error { message } if message.to_lowercase().contains("cancel"))
    });
    assert!(
        !has_cancel_error,
        "second turn (no cancel token) must not produce a cancellation Error; \
         got: {second_events:?}"
    );

    // spawn_count == 2: cancelled turn tombstoned process, second turn re-spawned.
    let spawn_count_str = std::fs::read_to_string(log_dir.join("spawn_count"))
        .expect("spawn_count must exist after two complete() calls");
    let spawn_count: u32 = spawn_count_str
        .trim()
        .parse()
        .expect("spawn_count must be a number");

    assert_eq!(
        spawn_count, 2,
        "cancelling a turn mid-stream must tombstone the process so the next \
         complete() re-spawns; expected spawn_count == 2, got {spawn_count}. \
         If spawn_count == 1, the process was not tombstoned after cancellation."
    );
}

// ---------------------------------------------------------------------------
// Helper: write_cancel_proof_respawning_claude
//
// A stub that checks its own spawn_count at startup:
//   - On the FIRST spawn (spawn_count == 1): same as write_cancel_proof_claude —
//     emits one token immediately, then sleeps 10 s.
//   - On SUBSEQUENT spawns (spawn_count >= 2): behaves like write_fake_claude —
//     responds to each NDJSON line immediately (no sleep).
//
// This lets CT-2's second turn complete without waiting for a 10 s sleep.
// ---------------------------------------------------------------------------

fn write_cancel_proof_respawning_claude(dir: &std::path::Path) -> String {
    let log_dir = dir.to_str().expect("log dir path must be utf-8");
    let body = format!(
        r#"#!/bin/sh
# Respawning-aware cancel-proof stub.
# First spawn (count == 1): emits one token, sleeps 10 s (simulates blocking work).
# Subsequent spawns (count >= 2): standard immediate multi-turn responses.
LOG_DIR={log_dir}

# Increment spawn counter and capture new count.
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then
    prev=$(cat "$count_file")
    new_count=$((prev + 1))
    echo "$new_count" > "$count_file"
else
    new_count=1
    echo 1 > "$count_file"
fi

# Record argv.
echo "$@" >> "$LOG_DIR/argv"

# Main loop.
if [ "$new_count" = "1" ]; then
    # First spawn: read one line, emit token, sleep, emit success.
    IFS= read -r line
    [ -z "$line" ] && exit 0
    echo "$line" >> "$LOG_DIR/received_lines"

    # Emit one token immediately so complete() can return Ok(stream).
    printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"pre-cancel token"}}]}}}}\n'

    # Block long enough for the test to cancel.
    sleep 10

    # This is never reached in the cancellation test.
    printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
else
    # Subsequent spawns: behave like the standard fake claude.
    while IFS= read -r line; do
        [ -z "$line" ] && continue
        echo "$line" >> "$LOG_DIR/received_lines"
        printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"post-cancel response"}}]}}}}\n'
        printf '{{"type":"result","subtype":"success","session_id":"stub-session","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
    done
fi
"#
    );
    write_script(dir, "claude", &body)
}

// ---------------------------------------------------------------------------
// Test SP-5: spawn_is_lazy_until_first_complete_call
//
// After calling `ClaudeCliProvider::with_binary(script)` and NOT calling
// `complete()`, the stub process must NOT have been spawned yet — the
// `argv_args` file must not exist because the script body never ran.
//
// This pins the lazy-spawn architecture required by SP-1 through SP-4:
// the spawn can only happen during `complete()` when `request.messages` is
// available.
//
// Expected failure (current): `with_binary()` calls `spawn_process_now()`
// synchronously, so the stub runs immediately and writes `argv_args` before
// `complete()` is ever called.  The 200 ms sleep below is long enough for the
// shell process to start up and write its first log lines, so this test will
// detect an eager spawn reliably.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_is_lazy_until_first_complete_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let log_dir = dir.path().to_owned();
    let script = write_argv_logging_claude(&log_dir);

    // Construct the provider but do NOT call complete().
    let _provider = ClaudeCliProvider::with_binary(&script);

    // Poll for up to 1 second waiting for argv_args to appear.
    //
    // A lazy implementation never spawns the child, so argv_args never appears;
    // we time out and the subsequent assertion fires on an absent file (passes).
    //
    // An eager implementation spawns the child in with_binary(), causing the
    // script to write argv_args immediately (typically within a few ms).  The
    // poll loop detects the file as soon as it exists, which makes the failure
    // visible quickly without a mandatory 1 s wait.
    // Poll up to 5 seconds for either log file to appear.  Under parallel test
    // execution the OS may delay the child's first write; 5 s is a generous
    // upper bound.  A lazy implementation never writes these files here, so the
    // poll simply times out and the subsequent assertions see absent files.
    let poll_deadline = std::time::Instant::now() + std::time::Duration::from_millis(5000);
    while std::time::Instant::now() < poll_deadline {
        if log_dir.join("argv_args").exists() || log_dir.join("spawn_count").exists() {
            break; // file appeared — stop polling early, assert below will fire
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    // The argv_args file must NOT exist because the stub script never ran.
    let argv_exists = log_dir.join("argv_args").exists();
    let argv_contents = std::fs::read_to_string(log_dir.join("argv_args")).unwrap_or_default();

    assert!(
        !argv_exists || argv_contents.trim().is_empty(),
        "no claude process must be spawned before complete() is called; \
         argv_args file exists with content: {argv_contents:?}"
    );

    // spawn_count file must also not exist.
    let spawn_count_exists = log_dir.join("spawn_count").exists();
    assert!(
        !spawn_count_exists,
        "spawn_count must not exist before complete() is called — process was spawned eagerly"
    );
}
