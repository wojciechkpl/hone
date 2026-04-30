//! Red-phase tests for R8: `hone memory ls / show / forget` CLI subcommand.
//!
//! These tests reference `crate::memory_cmd` (a module that does not yet exist
//! in `bins/hone/src/`) and `MemoryCmd` (an enum that does not yet exist).
//! Every test in this file will fail to compile on `main` — that is the
//! intended red-phase state.
//!
//! # Contract assumed by the implementer
//!
//! The module `bins/hone/src/memory_cmd.rs` must expose:
//!
//! ```rust,ignore
//! pub enum MemoryCmd {
//!     Ls   { limit: Option<usize>, kind: Option<String> },
//!     Show { id: String },
//!     Forget { tag: Option<String>, kind: Option<String>, before: Option<String> },
//! }
//!
//! pub async fn run(
//!     cmd: MemoryCmd,
//!     store: Arc<Mutex<dyn MemoryStore>>,
//!     out: &mut dyn std::io::Write,
//! ) -> anyhow::Result<()>;
//! ```
//!
//! The `out` parameter is how tests capture output without redirecting stdio.
//! If the implementer chooses a different output-injection strategy they may
//! adjust the call sites, but the observable behaviour pinned by each test
//! must be preserved.
//!
//! # Linking strategy
//!
//! `bins/hone` has no `[lib]` target, so modules are pulled in with `#[path]`.
//! Only `memory_cmd` is imported; it must have no undeclared deps on other
//! private `bins/hone` modules.

#[path = "../src/memory_cmd.rs"]
mod memory_cmd;

use std::sync::Arc;

use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryQuery, MemoryStore};
use memory_cmd::{MemoryCmd, run};
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

fn make_memory(
    id: &str,
    content: &str,
    kind: MemoryKind,
    description: Option<&str>,
    tags: Vec<String>,
    created_at: u64,
) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags,
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: description.map(String::from),
    }
}

async fn store_with_memories(memories: Vec<Memory>) -> Arc<Mutex<dyn MemoryStore>> {
    let mut inner = InMemoryStore::new();
    for m in memories {
        inner.record(m).await.expect("record");
    }
    Arc::new(Mutex::new(inner)) as Arc<Mutex<dyn MemoryStore>>
}

fn capture(cmd: MemoryCmd, store: Arc<Mutex<dyn MemoryStore>>) -> anyhow::Result<String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut buf: Vec<u8> = Vec::new();
        run(cmd, store, &mut buf).await?;
        Ok(String::from_utf8(buf).expect("utf8"))
    })
}

// ---------------------------------------------------------------------------
// Test 1 — ls prints one row per memory
// ---------------------------------------------------------------------------

#[test]
fn ls_prints_one_row_per_memory() {
    let memories = vec![
        make_memory("a1", "alpha content", MemoryKind::Conversation, None, vec![], 1_000),
        make_memory("b2", "beta content",  MemoryKind::User,         None, vec![], 2_000),
        make_memory("c3", "gamma content", MemoryKind::Feedback,     None, vec![], 3_000),
    ];
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(memories));

    let output = capture(MemoryCmd::Ls { limit: None, kind: None }, store)
        .expect("ls should succeed");

    let rows: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        rows.len(),
        3,
        "ls with 3 memories must print exactly 3 rows; got:\n{output}"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — ls respects limit
// ---------------------------------------------------------------------------

#[test]
fn ls_respects_limit() {
    let memories = vec![
        make_memory("a1", "alpha content", MemoryKind::Conversation, None, vec![], 1_000),
        make_memory("b2", "beta content",  MemoryKind::User,         None, vec![], 2_000),
        make_memory("c3", "gamma content", MemoryKind::Feedback,     None, vec![], 3_000),
        make_memory("d4", "delta content", MemoryKind::Project,      None, vec![], 4_000),
    ];
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(memories));

    let output = capture(MemoryCmd::Ls { limit: Some(2), kind: None }, store)
        .expect("ls with limit should succeed");

    let rows: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        rows.len(),
        2,
        "ls with limit=2 must print exactly 2 rows; got:\n{output}"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — ls filters by kind
// ---------------------------------------------------------------------------

#[test]
fn ls_filters_by_kind() {
    let memories = vec![
        make_memory("c1", "conversation one", MemoryKind::Conversation, None, vec![], 1_000),
        make_memory("c2", "conversation two", MemoryKind::Conversation, None, vec![], 2_000),
        make_memory("f1", "feedback entry",   MemoryKind::Feedback,     None, vec![], 3_000),
    ];
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(memories));

    let output = capture(
        MemoryCmd::Ls { limit: None, kind: Some("feedback".to_string()) },
        store,
    )
    .expect("ls with kind filter should succeed");

    let rows: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(
        rows.len(),
        1,
        "ls filtered by kind=feedback must show exactly 1 row; got:\n{output}"
    );
    assert!(
        output.to_lowercase().contains("feedback"),
        "the single row must mention 'feedback'; got:\n{output}"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — ls truncates long descriptions to 60 chars
// ---------------------------------------------------------------------------

#[test]
fn ls_truncates_long_descriptions_to_60_chars() {
    // 120-char description (well over the 60-char column limit).
    let long_desc = "A".repeat(120);
    let memories = vec![make_memory(
        "t1",
        "truncation test content",
        MemoryKind::User,
        Some(&long_desc),
        vec![],
        1_000,
    )];
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(memories));

    let output = capture(MemoryCmd::Ls { limit: None, kind: None }, store)
        .expect("ls should succeed for truncation test");

    // The 120-char long_desc must NOT appear verbatim in any single row.
    assert!(
        !output.contains(&long_desc),
        "ls must truncate the 120-char description; it must not appear verbatim in:\n{output}"
    );
    // The row must be non-empty (something was printed).
    assert!(
        !output.trim().is_empty(),
        "ls must print at least one row for the inserted memory"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — show prints full body for an existing id
// ---------------------------------------------------------------------------

#[test]
fn show_prints_full_body_for_existing_id() {
    let full_content = "This is the full multi-line body of memory mem-123. It contains all the \
                        important details that must not be truncated when shown.";
    let memories = vec![make_memory(
        "mem-123",
        full_content,
        MemoryKind::Feedback,
        Some("short description"),
        vec!["tag-a".to_string()],
        1_746_000_000,
    )];
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(memories));

    let output = capture(MemoryCmd::Show { id: "mem-123".to_string() }, store)
        .expect("show should succeed for existing id");

    assert!(
        output.contains("mem-123"),
        "show output must contain the memory id; got:\n{output}"
    );
    assert!(
        output.to_lowercase().contains("feedback"),
        "show output must contain the kind label; got:\n{output}"
    );
    assert!(
        output.contains(full_content),
        "show output must contain the full untruncated body; got:\n{output}"
    );
    // RFC3339 timestamp check — expect at least a year string.
    assert!(
        output.contains("2025") || output.contains("2026") || output.contains("1970"),
        "show output must contain a RFC3339-formatted created_at timestamp; got:\n{output}"
    );
}

// ---------------------------------------------------------------------------
// Test 6 — show errors on missing id
// ---------------------------------------------------------------------------

#[test]
fn show_errors_on_missing_id() {
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(vec![]));

    let result = capture(MemoryCmd::Show { id: "nope".to_string() }, store);

    assert!(
        result.is_err(),
        "show with a missing id must return Err; got Ok"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.to_lowercase().contains("not found") || err_msg.contains("nope"),
        "error message must mention 'not found' or the missing id; got: {err_msg}"
    );
}

// ---------------------------------------------------------------------------
// Test 7 — forget by tag returns count of removed memories
// ---------------------------------------------------------------------------

#[test]
fn forget_by_tag_returns_removed_count() {
    let memories = vec![
        make_memory("t1", "tagged one",   MemoryKind::Conversation, None, vec!["foo".to_string()], 1_000),
        make_memory("t2", "tagged two",   MemoryKind::Conversation, None, vec!["foo".to_string()], 2_000),
        make_memory("t3", "not tagged",   MemoryKind::Conversation, None, vec!["bar".to_string()], 3_000),
    ];
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(memories));

    let output = capture(
        MemoryCmd::Forget { tag: Some("foo".to_string()), kind: None, before: None },
        store.clone(),
    )
    .expect("forget by tag should succeed");

    assert!(
        output.contains('2'),
        "forget output must contain the count '2'; got:\n{output}"
    );

    // Verify the store actually has 1 record left.
    let remaining = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            store
                .lock()
                .await
                .recall(&MemoryQuery { text: String::new(), limit: 100 })
                .await
                .expect("recall after forget")
        });
    assert_eq!(
        remaining.len(),
        1,
        "after forgetting 2 'foo'-tagged memories, 1 should remain"
    );
}

// ---------------------------------------------------------------------------
// Test 8 — forget by kind returns count of removed memories
// ---------------------------------------------------------------------------

#[test]
fn forget_by_kind_returns_removed_count() {
    let memories = vec![
        make_memory("p1", "project one",    MemoryKind::Project,      None, vec![], 1_000),
        make_memory("p2", "project two",    MemoryKind::Project,      None, vec![], 2_000),
        make_memory("c1", "conversation",   MemoryKind::Conversation, None, vec![], 3_000),
    ];
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(memories));

    let output = capture(
        MemoryCmd::Forget { tag: None, kind: Some("project".to_string()), before: None },
        store.clone(),
    )
    .expect("forget by kind should succeed");

    assert!(
        output.contains('2'),
        "forget output must contain the count '2'; got:\n{output}"
    );

    let remaining = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            store
                .lock()
                .await
                .recall(&MemoryQuery { text: String::new(), limit: 100 })
                .await
                .expect("recall after forget by kind")
        });
    assert_eq!(
        remaining.len(),
        1,
        "after forgetting 2 Project memories, 1 Conversation should remain"
    );
    assert_eq!(
        remaining[0].kind,
        MemoryKind::Conversation,
        "the remaining memory must be the Conversation one"
    );
}

// ---------------------------------------------------------------------------
// Test 9 — forget by before removes old records
// ---------------------------------------------------------------------------
//
// Epoch layout:
//   created_at 1_000  → seconds since Unix epoch; maps to 1970-01-01T00:16:40Z
//   created_at 2_000  → 1970-01-01T00:33:20Z
//   created_at 9_999  → 1970-01-01T02:46:39Z
//
// `--before 1970-01-01T01:00:00Z` is unix epoch 3600.
// Only the two records with created_at < 3600 should be deleted.

#[test]
fn forget_by_before_removes_old_records() {
    let memories = vec![
        make_memory("old1", "old memory one", MemoryKind::Conversation, None, vec![], 1_000),
        make_memory("old2", "old memory two", MemoryKind::Conversation, None, vec![], 2_000),
        make_memory("new1", "new memory",     MemoryKind::Conversation, None, vec![], 9_999),
    ];
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(memories));

    let output = capture(
        MemoryCmd::Forget {
            tag: None,
            kind: None,
            before: Some("1970-01-01T01:00:00Z".to_string()),
        },
        store.clone(),
    )
    .expect("forget by before should succeed");

    assert!(
        output.contains('2'),
        "forget output must contain the count '2'; got:\n{output}"
    );

    let remaining = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            store
                .lock()
                .await
                .recall(&MemoryQuery { text: String::new(), limit: 100 })
                .await
                .expect("recall after forget by before")
        });
    assert_eq!(
        remaining.len(),
        1,
        "after forgetting the 2 old memories, 1 should remain"
    );
    assert_eq!(remaining[0].id, "new1", "the surviving memory must be 'new1'");
}

// ---------------------------------------------------------------------------
// Test 10 — forget errors when no filter is provided
// ---------------------------------------------------------------------------

#[test]
fn forget_errors_on_no_filter() {
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(vec![]));

    let result = capture(
        MemoryCmd::Forget { tag: None, kind: None, before: None },
        store,
    );

    assert!(
        result.is_err(),
        "forget with no filter must return Err; a bulk delete with no scope is too dangerous"
    );
}

// ---------------------------------------------------------------------------
// Test 11 — forget errors when multiple filters are provided
// ---------------------------------------------------------------------------

#[test]
fn forget_errors_on_multiple_filters() {
    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(vec![]));

    let result = capture(
        MemoryCmd::Forget {
            tag: Some("a".to_string()),
            kind: Some("feedback".to_string()),
            before: None,
        },
        store,
    );

    assert!(
        result.is_err(),
        "forget with tag AND kind both set must return Err; exactly one filter is required"
    );
}
