//! Red-phase tests for R7 Phase A: `hone_core::memory_index` module.
//!
//! These tests MUST FAIL to compile because `hone_core::memory_index` does not
//! exist yet.  Do not modify these tests during the green phase — they are the
//! contract.
//!
//! # Contract decisions pinned here
//!
//! - Truncation message:  `_(... N more memories truncated)_`
//! - Content fallback:    full content when ≤ 80 chars, else first 80 chars + `...`
//! - Empty corpus:        `render_index` returns `""` (not a header-only string)
//! - Decayed memories:    excluded from both `render_index` and `read_memory`
//!   (decay means `decay_at <= now`; simulated by calling `store.decay(1.0, far_future)`)
//! - `read_memory` with unknown id:  `Ok(None)`
//! - Kind order:  User → Feedback → Project → Reference → Outcome → Conversation

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use hone_core::memory_index::{IndexBudget, read_memory, render_index};
use hone_memory::{Memory, MemoryFilter, MemoryKind, MemoryQuery, MemoryStore, SqliteStore};
use tempfile::NamedTempFile;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time before epoch")
        .as_secs()
}

fn make_memory(
    id: &str,
    content: &str,
    kind: MemoryKind,
    weight: f64,
    description: Option<&str>,
    decay_at: Option<u64>,
) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight,
        created_at: now_secs(),
        decay_at,
        kind,
        description: description.map(String::from),
        embedding: None,
    }
}

/// Open a file-backed SqliteStore (interval=0 to skip 24h gating) and wrap in
/// `Arc<dyn MemoryStore>`.  The caller is responsible for keeping the
/// `NamedTempFile` alive for the duration of the test.
async fn open_store(tmp: &NamedTempFile) -> Arc<dyn MemoryStore> {
    let store = SqliteStore::open_with_decay_interval(tmp.path(), 0)
        .await
        .expect("open SqliteStore");
    Arc::new(store) as Arc<dyn MemoryStore>
}

// ---------------------------------------------------------------------------
// render_index tests
// ---------------------------------------------------------------------------

/// Test 1: fresh store with no memories returns the empty string.
#[tokio::test]
async fn render_empty_store_returns_empty_string() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    let result = render_index(store.as_ref(), IndexBudget::default())
        .await
        .expect("render_index must not error on empty store");

    assert_eq!(
        result, "",
        "empty store must return empty string; got: {result:?}"
    );
}

/// Test 2: one memory per kind — output contains a `### <Kind>` header for
/// each of the six kinds in the specified order.
#[tokio::test]
async fn render_groups_by_kind_with_headers() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    store
        .record(make_memory(
            "u1",
            "user content",
            MemoryKind::User,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record User");
    store
        .record(make_memory(
            "f1",
            "feedback content",
            MemoryKind::Feedback,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record Feedback");
    store
        .record(make_memory(
            "p1",
            "project content",
            MemoryKind::Project,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record Project");
    store
        .record(make_memory(
            "r1",
            "reference content",
            MemoryKind::Reference,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record Reference");
    store
        .record(make_memory(
            "o1",
            "outcome content",
            MemoryKind::Outcome,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record Outcome");
    store
        .record(make_memory(
            "c1",
            "conversation content",
            MemoryKind::Conversation,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record Conversation");

    let result = render_index(store.as_ref(), IndexBudget::default())
        .await
        .expect("render_index");

    // All six headers must be present.
    assert!(
        result.contains("### User"),
        "missing ### User; got:\n{result}"
    );
    assert!(
        result.contains("### Feedback"),
        "missing ### Feedback; got:\n{result}"
    );
    assert!(
        result.contains("### Project"),
        "missing ### Project; got:\n{result}"
    );
    assert!(
        result.contains("### Reference"),
        "missing ### Reference; got:\n{result}"
    );
    assert!(
        result.contains("### Outcome"),
        "missing ### Outcome; got:\n{result}"
    );
    assert!(
        result.contains("### Conversation"),
        "missing ### Conversation; got:\n{result}"
    );

    // Order: User before Feedback before Project before Reference before Outcome before Conversation.
    let pos_user = result.find("### User").expect("### User");
    let pos_feedback = result.find("### Feedback").expect("### Feedback");
    let pos_project = result.find("### Project").expect("### Project");
    let pos_reference = result.find("### Reference").expect("### Reference");
    let pos_outcome = result.find("### Outcome").expect("### Outcome");
    let pos_conversation = result.find("### Conversation").expect("### Conversation");

    assert!(pos_user < pos_feedback, "User must precede Feedback");
    assert!(pos_feedback < pos_project, "Feedback must precede Project");
    assert!(
        pos_project < pos_reference,
        "Project must precede Reference"
    );
    assert!(
        pos_reference < pos_outcome,
        "Reference must precede Outcome"
    );
    assert!(
        pos_outcome < pos_conversation,
        "Outcome must precede Conversation"
    );
}

/// Test 3: only Feedback + User memories → only those two headers appear.
#[tokio::test]
async fn render_skips_kinds_with_no_memories() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    store
        .record(make_memory(
            "u1",
            "user content",
            MemoryKind::User,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record User");
    store
        .record(make_memory(
            "f1",
            "feedback content",
            MemoryKind::Feedback,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record Feedback");

    let result = render_index(store.as_ref(), IndexBudget::default())
        .await
        .expect("render_index");

    assert!(
        result.contains("### User"),
        "### User must be present; got:\n{result}"
    );
    assert!(
        result.contains("### Feedback"),
        "### Feedback must be present; got:\n{result}"
    );
    assert!(
        !result.contains("### Project"),
        "### Project must NOT be present; got:\n{result}"
    );
    assert!(
        !result.contains("### Reference"),
        "### Reference must NOT be present; got:\n{result}"
    );
    assert!(
        !result.contains("### Outcome"),
        "### Outcome must NOT be present; got:\n{result}"
    );
    assert!(
        !result.contains("### Conversation"),
        "### Conversation must NOT be present; got:\n{result}"
    );
}

/// Test 4: memory with `description = Some(…)` → description text is used as
/// the entry label; memory without description → content is used (full if ≤ 80
/// chars, else first 80 chars + "...").
#[tokio::test]
async fn render_uses_description_when_present_and_content_otherwise() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    let long_content = "x".repeat(200); // definitely > 80 chars

    // First memory: has a description.
    store
        .record(make_memory(
            "d1",
            &long_content,
            MemoryKind::User,
            1.0,
            Some("the why"),
            None,
        ))
        .await
        .expect("record d1");

    // Second memory: no description, short content.
    store
        .record(make_memory(
            "d2",
            "fallback short",
            MemoryKind::User,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record d2");

    let result = render_index(store.as_ref(), IndexBudget::default())
        .await
        .expect("render_index");

    assert!(
        result.contains("the why"),
        "description must be used when present; got:\n{result}"
    );
    // The long content body must NOT appear verbatim (it would be > 80 chars).
    assert!(
        !result.contains(&long_content),
        "long content body must not appear verbatim; got:\n{result}"
    );
    assert!(
        result.contains("fallback short"),
        "short content must appear verbatim as fallback; got:\n{result}"
    );
}

/// Test 5: within a kind, entries are ordered weight DESC then created_at DESC.
#[tokio::test]
async fn render_orders_by_weight_then_recency_within_kind() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    // Insert in order 1.0 → 3.0 → 2.0.
    store
        .record(make_memory(
            "w1",
            "weight one",
            MemoryKind::Feedback,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record w1");
    store
        .record(make_memory(
            "w3",
            "weight three",
            MemoryKind::Feedback,
            3.0,
            None,
            None,
        ))
        .await
        .expect("record w3");
    store
        .record(make_memory(
            "w2",
            "weight two",
            MemoryKind::Feedback,
            2.0,
            None,
            None,
        ))
        .await
        .expect("record w2");

    let result = render_index(store.as_ref(), IndexBudget::default())
        .await
        .expect("render_index");

    let pos_three = result
        .find("weight three")
        .expect("weight three must appear");
    let pos_two = result.find("weight two").expect("weight two must appear");
    let pos_one = result.find("weight one").expect("weight one must appear");

    assert!(
        pos_three < pos_two,
        "weight=3.0 must precede weight=2.0; got:\n{result}"
    );
    assert!(
        pos_two < pos_one,
        "weight=2.0 must precede weight=1.0; got:\n{result}"
    );
}

/// Test 6: 60 memories, max_entries=10 → exactly 10 entry lines, truncation
/// message at the end.
#[tokio::test]
async fn render_truncates_at_max_entries() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    for i in 0..60 {
        let id = format!("entry-{i:03}");
        let content = format!("memory entry number {i}");
        store
            .record(make_memory(
                &id,
                &content,
                MemoryKind::User,
                1.0,
                None,
                None,
            ))
            .await
            .expect("record");
    }

    let budget = IndexBudget {
        max_entries: 10,
        max_chars: 100_000,
    };
    let result = render_index(store.as_ref(), budget)
        .await
        .expect("render_index");

    // Count entry lines (lines starting with "- [").
    let entry_count = result.lines().filter(|l| l.starts_with("- [")).count();
    assert_eq!(
        entry_count, 10,
        "exactly 10 entry lines expected; got {entry_count}; output:\n{result}"
    );

    // Truncation message must be present.
    assert!(
        result.contains("_(... 50 more memories truncated)_"),
        "truncation message must appear; got:\n{result}"
    );
}

/// Test 7: 50 memories with ~200-char descriptions, max_chars=1000 → rendered
/// string is ≤ 1000 chars and ends with truncation message.
#[tokio::test]
async fn render_truncates_at_max_chars() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    let long_desc = "a".repeat(200);

    for i in 0..50 {
        let id = format!("long-{i:03}");
        store
            .record(make_memory(
                &id,
                "body",
                MemoryKind::User,
                1.0,
                Some(&long_desc),
                None,
            ))
            .await
            .expect("record");
    }

    let budget = IndexBudget {
        max_entries: 100,
        max_chars: 1000,
    };
    let result = render_index(store.as_ref(), budget)
        .await
        .expect("render_index");

    assert!(
        result.len() <= 1000,
        "rendered length {} must be ≤ 1000; output:\n{result}",
        result.len()
    );
    assert!(
        result.contains("_(... ") && result.contains(" more memories truncated)_"),
        "truncation message must appear; got:\n{result}"
    );
    // No half-line: every '- [' must be on a complete line.
    for line in result.lines() {
        if line.starts_with("- [") {
            assert!(
                line.contains(']'),
                "entry line must be complete (no half-line): {line:?}"
            );
        }
    }
}

/// Test 8: 5 memories with generous budget → no truncation message.
#[tokio::test]
async fn render_truncation_message_does_not_appear_when_all_fit() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    for i in 0..5 {
        let id = format!("fit-{i}");
        store
            .record(make_memory(
                &id,
                "short content",
                MemoryKind::User,
                1.0,
                None,
                None,
            ))
            .await
            .expect("record");
    }

    let budget = IndexBudget {
        max_entries: 100,
        max_chars: 100_000,
    };
    let result = render_index(store.as_ref(), budget)
        .await
        .expect("render_index");

    assert!(
        !result.contains("truncated"),
        "truncation message must NOT appear when all memories fit; got:\n{result}"
    );
}

/// Test 9: each entry line contains its memory id in `[brackets]`.
#[tokio::test]
async fn render_includes_id_for_each_entry() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    let ids = ["alpha-id", "beta-id", "gamma-id"];

    for id in &ids {
        store
            .record(make_memory(
                id,
                "some content",
                MemoryKind::User,
                1.0,
                None,
                None,
            ))
            .await
            .expect("record");
    }

    let result = render_index(store.as_ref(), IndexBudget::default())
        .await
        .expect("render_index");

    for id in &ids {
        let bracketed = format!("[{id}]");
        assert!(
            result.contains(&bracketed),
            "entry for id {id} must appear as [{id}] in output; got:\n{result}"
        );
    }
}

/// Test 10: a memory with `decay_at` in the past must not appear in the index.
///
/// Strategy: insert the memory with `decay_at = 1` (epoch + 1 second, well in
/// the past), then call `store.decay(1.0, now)` to trigger removal before
/// wrapping for render.  This mirrors what `open_with_decay_interval(path, 0)`
/// does on open.
#[tokio::test]
async fn render_skips_decayed_memories() {
    let tmp = NamedTempFile::new().expect("create temp file");

    // Open the bare store first so we can call decay directly.
    let mut bare = SqliteStore::open_with_decay_interval(tmp.path(), 0)
        .await
        .expect("open SqliteStore");

    let past_decay_at = 1u64; // epoch + 1 second, guaranteed in the past.

    bare.record(make_memory(
        "decayed-1",
        "should not appear",
        MemoryKind::User,
        1.0,
        None,
        Some(past_decay_at),
    ))
    .await
    .expect("record decayed memory");

    // Trigger decay to actually remove the expired memory.
    bare.decay(1.0, now_secs()).await.expect("decay");

    // Now wrap in Arc<dyn MemoryStore> for the render call.
    let store: Arc<dyn MemoryStore> = Arc::new(bare);

    let result = render_index(store.as_ref(), IndexBudget::default())
        .await
        .expect("render_index");

    assert_eq!(
        result, "",
        "decayed memory must not appear; store should be empty after decay; got:\n{result}"
    );
}

// ---------------------------------------------------------------------------
// read_memory tests
// ---------------------------------------------------------------------------

/// Test 11: `read_memory` with an existing id returns `Ok(Some(Memory))` with
/// the correct content.
#[tokio::test]
async fn read_existing_id_returns_full_body() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    store
        .record(make_memory(
            "read-me",
            "full body content",
            MemoryKind::User,
            1.0,
            None,
            None,
        ))
        .await
        .expect("record");

    let result = read_memory(store.as_ref(), "read-me")
        .await
        .expect("read_memory must not error");

    assert!(result.is_some(), "existing id must return Some(Memory)");
    let memory = result.unwrap();
    assert_eq!(
        memory.content, "full body content",
        "content must match; got: {:?}",
        memory.content
    );
}

/// Test 12: `read_memory` with an unknown id returns `Ok(None)`.
#[tokio::test]
async fn read_unknown_id_returns_none() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    let result = read_memory(store.as_ref(), "nonexistent_id")
        .await
        .expect("read_memory must not error on unknown id");

    assert!(
        result.is_none(),
        "unknown id must return None; got: {result:?}"
    );
}

/// Test 13: `read_memory` after `forget(MemoryFilter::Tag)` returns `Ok(None)`.
///
/// Since `MemoryStore` has no `forget_by_id`, we tag the memory with a sentinel
/// tag and then forget by that tag.
#[tokio::test]
async fn read_after_forget_returns_none() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    let mut m = make_memory(
        "forget-me",
        "to be forgotten",
        MemoryKind::User,
        1.0,
        None,
        None,
    );
    m.tags = vec!["sentinel-forget-tag".to_string()];
    store.record(m).await.expect("record");

    // Verify it exists first.
    let before = read_memory(store.as_ref(), "forget-me")
        .await
        .expect("read_memory before forget");
    assert!(before.is_some(), "memory must exist before forget");

    // Forget by the sentinel tag.
    store
        .forget(&MemoryFilter::Tag("sentinel-forget-tag".to_string()))
        .await
        .expect("forget");

    // Now read_memory must return None.
    let after = read_memory(store.as_ref(), "forget-me")
        .await
        .expect("read_memory after forget");
    assert!(
        after.is_none(),
        "read_memory must return None after forget; got: {after:?}"
    );
}

/// Test 14: `read_memory` for a decayed memory (decay_at in the past) returns
/// `Ok(None)`.
///
/// Strategy: insert the memory, call `store.decay(1.0, far_future)` via
/// the trait to trigger removal, then call `read_memory`.
#[tokio::test]
async fn read_decayed_memory_returns_none() {
    let tmp = NamedTempFile::new().expect("create temp file");

    // Open the bare store first so we can call decay directly.
    let mut bare = SqliteStore::open_with_decay_interval(tmp.path(), 0)
        .await
        .expect("open SqliteStore");

    let past_decay_at = 1u64;

    bare.record(make_memory(
        "will-decay",
        "doomed content",
        MemoryKind::User,
        1.0,
        None,
        Some(past_decay_at),
    ))
    .await
    .expect("record");

    // Trigger decay to remove the expired memory.
    bare.decay(1.0, now_secs()).await.expect("decay");

    // Wrap and call read_memory.
    let store: Arc<dyn MemoryStore> = Arc::new(bare);

    let result = read_memory(store.as_ref(), "will-decay")
        .await
        .expect("read_memory must not error");

    assert!(
        result.is_none(),
        "decayed memory must return None from read_memory; got: {result:?}"
    );
}
