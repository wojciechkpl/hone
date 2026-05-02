//! Red-phase integration test for `memory_cmd::run` with the pre-limit kind
//! filter (R10 fix).
//!
//! This test will FAIL TO COMPILE on `main` because:
//!   - `MemoryQuery` has no `kind` field (required by the store fix that makes
//!     `MemoryCmd::Ls { kind: Some(_) }` return the correct count).
//!
//! Once the implementer adds `MemoryQuery::kind` and wires it through `memory_cmd`,
//! this test must pass without any modification here.
//!
//! # Bug being caught
//!
//! With the current post-limit retain in `memory_cmd.rs`:
//!   recall(limit=5) → returns 5 Conversation memories → retain(kind=User) → 0 rows.
//!
//! With the fix (kind filter pushed into MemoryQuery → pre-limit in the store):
//!   recall(kind=User, limit=5) → returns 3 User memories → 3 rows.

#[path = "../src/memory_cmd.rs"]
mod memory_cmd;

use std::sync::Arc;

use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use memory_cmd::{MemoryCmd, run};

// ---------------------------------------------------------------------------
// Helpers (same pattern as memory_cmd_test.rs)
// ---------------------------------------------------------------------------

fn make_memory(id: &str, content: &str, kind: MemoryKind, created_at: u64) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: None,
    }
}

async fn store_with_memories(memories: Vec<Memory>) -> Arc<dyn MemoryStore> {
    let mut inner = InMemoryStore::new();
    for m in memories {
        inner.record(m).await.expect("record");
    }
    Arc::new(inner) as Arc<dyn MemoryStore>
}

fn capture(cmd: MemoryCmd, store: Arc<dyn MemoryStore>) -> anyhow::Result<String> {
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
// Test 10 — Ls with kind=user and limit=5 against 50 Conversation + 3 User
//
// The 50 Conversation memories are inserted with higher created_at so they
// sort first by recency.  With a buggy post-limit retain:
//   - recall(limit=5) returns the 5 most-recent Conversation memories
//   - retain(kind=User) leaves 0 rows
//
// With the correct pre-limit fix, all 3 User memories appear in the output.
// ---------------------------------------------------------------------------

#[test]
fn ls_kind_filter_is_pre_limit() {
    // 3 User memories with low created_at (inserted first, older).
    let mut memories: Vec<Memory> = (1..=3u64)
        .map(|i| make_memory(&format!("user-{i}"), &format!("user pref {i}"), MemoryKind::User, i))
        .collect();

    // 50 Conversation memories with higher created_at (inserted last, newer).
    // These will be selected first by recall(limit=5, no kind filter).
    for i in 1..=50u64 {
        memories.push(make_memory(
            &format!("conv-{i:03}"),
            &format!("conversation {i}"),
            MemoryKind::Conversation,
            100 + i,
        ));
    }

    let store = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(store_with_memories(memories));

    let output = capture(
        MemoryCmd::Ls {
            kind: Some("user".to_string()),
            limit: Some(5),
        },
        store,
    )
    .expect("Ls with kind=user limit=5 must not error");

    // Count non-empty rows in the output.
    let rows: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();

    assert_eq!(
        rows.len(),
        3,
        "Ls kind=user limit=5 with 50 Conversation + 3 User must return exactly 3 rows; \
         a post-limit retain returns 0 because the 5 recalled rows are all Conversation; \
         got output:\n{output}"
    );

    // All rows must mention "user".
    for row in &rows {
        assert!(
            row.to_lowercase().contains("user"),
            "every output row must be labelled 'user'; got row: {row}\nfull output:\n{output}"
        );
    }

    // None of the rows must mention "conversation".
    for row in &rows {
        assert!(
            !row.to_lowercase().contains("conversation"),
            "output must not contain any Conversation rows; got row: {row}\nfull output:\n{output}"
        );
    }
}
