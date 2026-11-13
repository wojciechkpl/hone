//! CC10 of `docs/plans/claude-cli-turn-context-spec.md`: a process other than main's that has
//! been idle for more than the idle timeout (5 minutes by default) is closed the next time the
//! pool is used; main's process is never closed by idleness; a process mid-turn is never closed.
//!
//! TEST HOOK (new API): `ClaudeCliProvider::with_idle_timeout(self, Duration) -> Self`, chained
//! like `with_mcp_config`. The default is 5 minutes. "Closed" means its stdin is closed and the
//! process ends (the stub writes `exited_<n>` when its stdin reaches EOF); a later turn on that
//! base spawns a new process.

#![cfg(unix)]
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use futures::StreamExt;
use hone_proto::{Message, ProviderEvent};
use hone_providers::{ClaudeCliProvider, CompletionRequest, ModelProvider};

const IDLE: Duration = Duration::from_millis(400);

/// A stub that logs its spawn, its stdin per process and `exited_<n>` when its stdin ends; a line
/// containing SLOW takes one second to answer.
fn stub(dir: &Path) -> String {
    let log_dir = dir.to_str().unwrap();
    let path = dir.join("claude");
    std::fs::write(
        &path,
        format!(
            r#"#!/bin/sh
LOG_DIR={log_dir}
count_file="$LOG_DIR/spawn_count"
if [ -f "$count_file" ]; then prev=$(cat "$count_file"); n=$((prev + 1)); else n=1; fi
echo $n > "$count_file"
while IFS= read -r line; do
    [ -z "$line" ] && continue
    printf '%s\n' "$line" >> "$LOG_DIR/received_$n"
    case "$line" in *SLOW*) sleep 1;; esac
    printf '{{"type":"assistant","message":{{"role":"assistant","content":[{{"type":"text","text":"stub response"}}]}}}}\n'
    printf '{{"type":"result","subtype":"success","session_id":"s","usage":{{"input_tokens":5,"output_tokens":3,"cache_read_input_tokens":0}}}}\n'
done
: > "$LOG_DIR/exited_$n"
"#
        ),
    )
    .unwrap();
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    path.to_str().unwrap().to_owned()
}

fn request(provider: &ClaudeCliProvider, base: &str, user: &str) -> CompletionRequest {
    let model = provider.supported_models().into_iter().next().unwrap().id;
    CompletionRequest::new(model, vec![Message::system(base), Message::user(user)])
}

async fn turn(provider: &ClaudeCliProvider, base: &str, user: &str) {
    let stream = tokio::time::timeout(Duration::from_secs(10), provider.complete(request(provider, base, user)))
        .await
        .expect("complete() hung")
        .expect("complete() failed");
    let events: Vec<ProviderEvent> = tokio::time::timeout(Duration::from_secs(10), stream.collect())
        .await
        .expect("stream hung");
    assert!(events.iter().any(|e| matches!(e, ProviderEvent::Done)), "{events:?}");
}

fn spawns(dir: &Path) -> u32 {
    std::fs::read_to_string(dir.join("spawn_count")).unwrap().trim().parse().unwrap()
}

fn exited(dir: &Path, n: u32) -> bool {
    dir.join(format!("exited_{n}")).exists()
}

async fn wait_exited(dir: &Path, n: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if exited(dir, n) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    exited(dir, n)
}

#[tokio::test]
async fn test_cc10_an_idle_side_process_should_be_closed_on_the_next_pool_use() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path())).with_idle_timeout(IDLE);
    turn(&provider, "BASE MAIN", "m1").await;
    turn(&provider, "BASE WORKER", "w1").await; // process 2
    assert!(!exited(dir.path(), 2), "still within the timeout");

    tokio::time::sleep(IDLE * 3).await;
    turn(&provider, "BASE MAIN", "m2").await; // uses the pool: the idle worker is closed

    assert!(wait_exited(dir.path(), 2).await, "the idle worker process was closed");
    turn(&provider, "BASE WORKER", "w2").await;
    assert_eq!(spawns(dir.path()), 3, "a later worker turn spawns anew");
}

#[tokio::test]
async fn test_cc10_a_side_process_used_within_the_timeout_should_stay_open() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path())).with_idle_timeout(IDLE * 5);
    turn(&provider, "BASE MAIN", "m1").await;
    turn(&provider, "BASE WORKER", "w1").await;
    tokio::time::sleep(IDLE).await;
    turn(&provider, "BASE MAIN", "m2").await;

    turn(&provider, "BASE WORKER", "w2").await;

    assert_eq!(spawns(dir.path()), 2);
    assert!(!exited(dir.path(), 2));
}

#[tokio::test]
async fn test_cc10_main_should_never_be_closed_by_idleness() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path())).with_idle_timeout(IDLE);
    turn(&provider, "BASE MAIN", "m1").await;

    tokio::time::sleep(IDLE * 3).await;
    turn(&provider, "BASE WORKER", "w1").await; // pool use long after main went idle
    tokio::time::sleep(Duration::from_millis(300)).await;

    assert!(!exited(dir.path(), 1), "main's process is still open");
    turn(&provider, "BASE MAIN", "m2").await;
    assert_eq!(spawns(dir.path()), 2, "main's later turn used the original process");
}

#[tokio::test]
async fn test_cc10_a_process_mid_turn_should_not_be_closed() {
    let dir = tempfile::tempdir().unwrap();
    let provider = ClaudeCliProvider::with_binary(&stub(dir.path())).with_idle_timeout(IDLE);
    turn(&provider, "BASE MAIN", "m1").await;
    // A worker turn is in flight (the stub takes a second to answer it).
    let stream = provider
        .complete(request(&provider, "BASE WORKER", "w1 SLOW"))
        .await
        .expect("complete() failed");
    tokio::time::sleep(IDLE * 2).await; // longer than the idle timeout, but the process is busy

    tokio::time::timeout(Duration::from_secs(5), provider.complete(request(&provider, "BASE MAIN", "m2")))
        .await
        .expect("a pool use while another process is mid-turn must not block")
        .expect("complete() failed")
        .collect::<Vec<_>>()
        .await;

    let events: Vec<ProviderEvent> = tokio::time::timeout(Duration::from_secs(10), stream.collect())
        .await
        .expect("the in-flight turn must finish");
    assert!(events.iter().any(|e| matches!(e, ProviderEvent::Done)), "the mid-turn process answered: {events:?}");
    assert!(!exited(dir.path(), 2), "it was not closed under the turn");
}
