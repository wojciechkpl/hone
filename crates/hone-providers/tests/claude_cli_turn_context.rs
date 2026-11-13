//! CC1-CC4 of `docs/plans/claude-cli-turn-context-spec.md`: the `claude-cli:` adapter keeps one
//! persistent `claude` process, so only Hone's base prompt (the first system message) is fixed at
//! spawn (`--system-prompt`); every other system message of a turn (memories, skills, gathered
//! context) travels with that turn's user text in one delimited block before it; messages already
//! sent to this process aren't sent again; a changed base prompt or a re-spawn resets that.
//!
//! The block's wording is not pinned beyond: it contains "context" and "this turn"
//! (case-insensitive) before the first message, and every message appears whole and in order.
//! Stub-binary harness as in `claude_cli_multiturn.rs`: it records, per spawn, the
//! `--system-prompt` value (`sp_<n>`), and every NDJSON line written to stdin.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use futures::StreamExt;
use hone_proto::{Message, ProviderEvent};
use hone_providers::{ClaudeCliProvider, CompletionRequest, ModelProvider};

fn write_script(dir: &Path, body: &str) -> String {
    let path = dir.join("claude");
    std::fs::write(&path, body).unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path.to_str().unwrap().to_owned()
}

/// `one_shot`: the process answers one line and exits (a death between turns).
fn stub(dir: &Path, one_shot: bool) -> String {
    let log_dir = dir.to_str().unwrap();
    let exit_after = if one_shot { "    exit 0\n" } else { "" };
    write_script(
        dir,
        &format!(
            r#"#!/bin/sh
LOG_DIR={log_dir}
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then prev=$(cat "$count_file"); n=$((prev + 1)); else n=1; fi
echo $n > "$count_file"
prev=""
for a in "$@"; do
    if [ "$prev" = "--system-prompt" ]; then printf '%s' "$a" > "$LOG_DIR/sp_$n"; fi
    prev="$a"
done
while IFS= read -r line; do
    [ -z "$line" ] && continue
    echo "$line" >> "$LOG_DIR/received_lines"
    printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"stub response"}}]}}}}\n'
    printf '{{"type":"result","subtype":"success","session_id":"s","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
{exit_after}done
"#
        ),
    )
}

async fn turn(provider: &ClaudeCliProvider, messages: Vec<Message>) {
    let model = provider.supported_models().into_iter().next().unwrap().id;
    let stream = tokio::time::timeout(
        Duration::from_secs(10),
        provider.complete(CompletionRequest::new(model, messages)),
    )
    .await
    .expect("complete() hung")
    .expect("complete() failed");
    let events: Vec<ProviderEvent> = tokio::time::timeout(Duration::from_secs(10), stream.collect())
        .await
        .expect("stream hung");
    assert!(
        events.iter().any(|e| matches!(e, ProviderEvent::Done)),
        "the turn finished: {events:?}"
    );
}

/// The text of each NDJSON user line the stub received, in order.
fn payloads(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("received_lines"))
        .unwrap_or_default()
        .lines()
        .map(|l| {
            let v: serde_json::Value = serde_json::from_str(l).expect("NDJSON");
            v.pointer("/message/content/0/text").and_then(|t| t.as_str()).unwrap().to_owned()
        })
        .collect()
}

fn spawns(dir: &Path) -> u32 {
    std::fs::read_to_string(dir.join("spawn_count")).unwrap().trim().parse().unwrap()
}

fn spawn_prompt(dir: &Path, n: u32) -> String {
    std::fs::read_to_string(dir.join(format!("sp_{n}"))).unwrap_or_default()
}

const BASE: &str = "You are Hone, the base prompt.";
const MEM_A: &str = "# Relevant Memories\n\n- MEMORY-A the deploy window is Friday\n\n- MEMORY-A2 second line";
const MEM_B: &str = "# Relevant Memories\n\n- MEMORY-B the cache TTL is five minutes";
const SKILL: &str = "## Skills\nSKILL-S rebase before merging";

fn msgs(base: &str, ctx: &[&str], user: &str) -> Vec<Message> {
    let mut v = vec![Message::system(base)];
    v.extend(ctx.iter().map(|c| Message::system(*c)));
    v.push(Message::user(user));
    v
}

// ---- CC1 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_cc1_system_prompt_at_spawn_should_hold_only_the_base_prompt() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path(), false));

    turn(&provider, msgs(BASE, &[MEM_A, SKILL], "turn one")).await;

    assert_eq!(spawn_prompt(dir.path(), 1), BASE, "memories and skills are not in --system-prompt");
}

// ---- CC2 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_cc2_turn_one_should_carry_its_context_in_a_block_before_the_user_text() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path(), false));

    turn(&provider, msgs(BASE, &[MEM_A, SKILL], "turn one")).await;

    let text = &payloads(dir.path())[0];
    let (a, s, u) = (text.find(MEM_A), text.find(SKILL), text.rfind("turn one"));
    assert!(a.is_some(), "memory A is kept whole in the payload:\n{text}");
    assert!(s.is_some(), "skill S is kept whole in the payload:\n{text}");
    assert!(a < s && s < u, "context in order, then the user text last:\n{text}");
    assert!(text.trim_end().ends_with("turn one"), "{text}");
    let head = text[..a.unwrap()].to_lowercase();
    assert!(head.contains("context") && head.contains("this turn"), "the block opens by saying what it is:\n{text}");
    assert!(!text.contains(BASE), "the base prompt is not repeated in the turn");
}

#[tokio::test]
async fn test_cc2_turn_two_should_carry_its_own_context_not_turn_ones() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path(), false));
    turn(&provider, msgs(BASE, &[MEM_A, SKILL], "turn one")).await;

    turn(&provider, msgs(BASE, &[MEM_B, SKILL], "turn two")).await;

    assert_eq!(spawns(dir.path()), 1, "same base prompt: the process is kept");
    let text = &payloads(dir.path())[1];
    assert!(text.contains(MEM_B), "turn 2's memory reaches the model:\n{text}");
    assert!(!text.contains("MEMORY-A"), "turn 1's memory is not repeated:\n{text}");
    assert!(!text.contains("SKILL-S"), "the skill was already sent (CC3):\n{text}");
    assert!(text.trim_end().ends_with("turn two"), "{text}");
    assert!(text.to_lowercase().contains("this turn"), "{text}");
}

// ---- CC3 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_cc3_a_turn_with_nothing_new_should_be_sent_without_a_block() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path(), false));
    turn(&provider, msgs(BASE, &[MEM_B, SKILL], "turn one")).await;

    turn(&provider, msgs(BASE, &[MEM_B, SKILL], "turn two")).await;

    assert_eq!(payloads(dir.path())[1], "turn two", "no block, just the user text");
}

#[tokio::test]
async fn test_cc3_after_a_death_triggered_respawn_everything_non_base_should_be_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path(), true));
    turn(&provider, msgs(BASE, &[MEM_A, SKILL], "turn one")).await;

    // The one-shot stub exited after turn 1: turn 2 re-spawns it.
    turn(&provider, msgs(BASE, &[MEM_A, SKILL], "turn two")).await;

    assert!(spawns(dir.path()) >= 2, "re-spawned");
    let text = &payloads(dir.path())[1];
    assert!(text.contains(MEM_A) && text.contains(SKILL), "a fresh process hasn't seen them:\n{text}");
    assert!(text.find(MEM_A) < text.find(SKILL), "in order:\n{text}");
    assert_eq!(spawn_prompt(dir.path(), 2), BASE);
}

// ---- CC4 -------------------------------------------------------------------------------------------

#[tokio::test]
async fn test_cc4_a_changed_base_prompt_should_respawn_with_the_new_prompt_and_resend_the_context() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path(), false));
    turn(&provider, msgs("BASE ONE", &[MEM_A], "turn one")).await;

    turn(&provider, msgs("BASE TWO", &[MEM_A], "turn two")).await;

    assert_eq!(spawns(dir.path()), 2, "the session was restarted");
    assert_eq!(spawn_prompt(dir.path(), 1), "BASE ONE");
    assert_eq!(spawn_prompt(dir.path(), 2), "BASE TWO", "the new base prompt, not the first one");
    let text = &payloads(dir.path())[1];
    assert!(text.contains(MEM_A), "the dedupe reset with the new process:\n{text}");
    assert!(text.trim_end().ends_with("turn two"), "{text}");
}

#[tokio::test]
async fn test_cc4_an_unchanged_base_prompt_should_not_respawn() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path(), false));

    turn(&provider, msgs(BASE, &[MEM_A], "turn one")).await;
    turn(&provider, msgs(BASE, &[MEM_B], "turn two")).await;
    turn(&provider, msgs(BASE, &[MEM_A], "turn three")).await;

    assert_eq!(spawns(dir.path()), 1);
}
