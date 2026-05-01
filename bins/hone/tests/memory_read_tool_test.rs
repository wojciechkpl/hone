//! Red-phase tests for R7 Phase B: `MemoryReadTool`.
//!
//! Every test in this file MUST fail to compile on `main` because the module
//! `bins/hone/src/tools/memory_read_tool.rs` does not yet exist.
//!
//! # Contract assumed by the implementer
//!
//! A new file `bins/hone/src/tools/memory_read_tool.rs` must expose:
//!
//! ```rust,ignore
//! pub struct MemoryReadTool {
//!     store: Arc<Mutex<dyn MemoryStore>>,
//! }
//!
//! impl MemoryReadTool {
//!     pub fn new(store: Arc<Mutex<dyn MemoryStore>>) -> Self { … }
//! }
//!
//! #[async_trait]
//! impl Tool for MemoryReadTool {
//!     fn name(&self) -> &str { "memory_read" }
//!     fn description(&self) -> &str { /* ~2 sentences */ }
//!     fn schema(&self) -> Value { /* { id: string, required: ["id"] } */ }
//!     async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
//!     fn layer(&self) -> ToolLayer { ToolLayer::Native }
//! }
//! ```
//!
//! # Behaviour pinned by these tests
//!
//! - `id` missing or empty → `ToolError::InvalidInput`.
//! - Known id → `ToolOutput` with JSON containing `id`, `content`, `kind`
//!   (lowercase serde name), `description` (nullable), `weight`, `tags`.
//! - Unknown id → `ToolOutput` with `{ "id": …, "found": false }`. NOT a `ToolError`.
//! - Decayed memory → same as unknown id (`found: false`).
//! - `description` present → field equals the stored string.
//! - `description` absent → field is JSON `null`.
//!
//! # Linking strategy
//!
//! `bins/hone` has no `[lib]` target, so the module is pulled in with `#[path]`
//! mirroring the pattern used by `system_prompt_test.rs`, `memory_cmd_test.rs`,
//! and `encrypted_memory_test.rs`.

#[path = "../src/tools/memory_read_tool.rs"]
mod memory_read_tool;

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use hone_memory::{Memory, MemoryKind, MemoryQuery, MemoryStore, SqliteStore};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolInput};
use memory_read_tool::MemoryReadTool;
use serde_json::{Value, json};
use tempfile::NamedTempFile;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time before epoch")
        .as_secs()
}

fn make_memory(id: &str, content: &str, kind: MemoryKind, description: Option<&str>) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight: 1.0,
        created_at: now_secs(),
        decay_at: None,
        kind,
        description: description.map(String::from),
    }
}

/// Open a file-backed `SqliteStore` (interval=0 skips 24h decay gating) and
/// wrap as `Arc<Mutex<dyn MemoryStore>>`.  The caller must keep `tmp` alive.
async fn open_store(tmp: &NamedTempFile) -> Arc<Mutex<dyn MemoryStore>> {
    let store = SqliteStore::open_with_decay_interval(tmp.path(), 0)
        .await
        .expect("open SqliteStore");
    Arc::new(Mutex::new(store)) as Arc<Mutex<dyn MemoryStore>>
}

/// Build a minimal `ToolContext`.  The `memory_read` tool never touches the
/// sandbox or `working_dir`, so we use `NullSandbox` and a temp path.
fn make_ctx() -> ToolContext {
    use std::path::PathBuf;

    ToolContext {
        sandbox: Arc::new(NullSandbox),
        working_dir: PathBuf::from("/tmp"),
    }
}

// ---------------------------------------------------------------------------
// Test 1 — known id returns content and kind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_read_returns_body_for_known_id() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    {
        let mut guard = store.lock().await;
        guard
            .record(make_memory(
                "mem-001",
                "the full body text",
                MemoryKind::Feedback,
                None,
            ))
            .await
            .expect("record");
    }

    let tool = MemoryReadTool::new(Arc::clone(&store));
    let ctx = make_ctx();
    let input = ToolInput::new(json!({ "id": "mem-001" }));

    let output = tool.call(input, &ctx).await.expect("call must succeed");
    let v = output.into_value();

    assert_eq!(v["id"], "mem-001", "output must echo the requested id");
    assert_eq!(
        v["content"], "the full body text",
        "output must include the memory content"
    );
    assert_eq!(
        v["kind"], "feedback",
        "kind must use the lowercase serde name"
    );
}

// ---------------------------------------------------------------------------
// Test 2 — unknown id returns found:false, not a ToolError
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_read_returns_found_false_for_unknown_id() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    let tool = MemoryReadTool::new(Arc::clone(&store));
    let ctx = make_ctx();
    let input = ToolInput::new(json!({ "id": "nonexistent" }));

    let output = tool
        .call(input, &ctx)
        .await
        .expect("unknown id must not error");
    let v = output.into_value();

    assert_eq!(
        v["found"],
        json!(false),
        "unknown id must produce {{ found: false }}"
    );
    assert_eq!(
        v["id"], "nonexistent",
        "output must echo the requested id even when not found"
    );
}

// ---------------------------------------------------------------------------
// Test 3 — missing id field returns ToolError::InvalidInput
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_read_rejects_missing_id() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    let tool = MemoryReadTool::new(Arc::clone(&store));
    let ctx = make_ctx();
    let input = ToolInput::new(json!({}));

    let err = tool
        .call(input, &ctx)
        .await
        .expect_err("missing id must return ToolError");

    assert!(
        matches!(err, hone_tools::ToolError::InvalidInput { .. }),
        "error must be ToolError::InvalidInput, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 4 — empty id string returns ToolError::InvalidInput
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_read_rejects_empty_id() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    let tool = MemoryReadTool::new(Arc::clone(&store));
    let ctx = make_ctx();
    let input = ToolInput::new(json!({ "id": "" }));

    let err = tool
        .call(input, &ctx)
        .await
        .expect_err("empty id must return ToolError");

    assert!(
        matches!(err, hone_tools::ToolError::InvalidInput { .. }),
        "error must be ToolError::InvalidInput for empty id, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 5 — decayed memory returns found:false
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_read_returns_found_false_for_decayed_id() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    {
        let mut guard = store.lock().await;
        // decay_at = 1 means this memory expired at Unix epoch + 1 second.
        // Call decay(0.0, now) to remove all memories whose decay_at <= now.
        let expired = Memory {
            id: "mem-expired".to_string(),
            content: "will decay".to_string(),
            tags: vec![],
            weight: 1.0,
            created_at: 1,
            decay_at: Some(1),
            kind: MemoryKind::Conversation,
            description: None,
        };
        guard.record(expired).await.expect("record");
        // Decay with factor 0.0 and a large `now` so everything with a
        // decay_at <= now is purged.
        guard
            .decay(0.0, now_secs())
            .await
            .expect("decay must succeed");
    }

    let tool = MemoryReadTool::new(Arc::clone(&store));
    let ctx = make_ctx();
    let input = ToolInput::new(json!({ "id": "mem-expired" }));

    let output = tool
        .call(input, &ctx)
        .await
        .expect("decayed id must not error");
    let v = output.into_value();

    assert_eq!(
        v["found"],
        json!(false),
        "decayed memory must produce {{ found: false }}"
    );
}

// ---------------------------------------------------------------------------
// Test 6 — description field present in output when stored
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_read_includes_description_when_present() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    {
        let mut guard = store.lock().await;
        guard
            .record(make_memory(
                "mem-desc",
                "body text",
                MemoryKind::User,
                Some("the why"),
            ))
            .await
            .expect("record");
    }

    let tool = MemoryReadTool::new(Arc::clone(&store));
    let ctx = make_ctx();
    let input = ToolInput::new(json!({ "id": "mem-desc" }));

    let output = tool.call(input, &ctx).await.expect("call must succeed");
    let v = output.into_value();

    assert_eq!(
        v["description"], "the why",
        "output must include the stored description"
    );
}

// ---------------------------------------------------------------------------
// Test 7 — description field is JSON null when absent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn memory_read_nulls_description_when_absent() {
    let tmp = NamedTempFile::new().expect("create temp file");
    let store = open_store(&tmp).await;

    {
        let mut guard = store.lock().await;
        guard
            .record(make_memory(
                "mem-nodesc",
                "body text",
                MemoryKind::Project,
                None,
            ))
            .await
            .expect("record");
    }

    let tool = MemoryReadTool::new(Arc::clone(&store));
    let ctx = make_ctx();
    let input = ToolInput::new(json!({ "id": "mem-nodesc" }));

    let output = tool.call(input, &ctx).await.expect("call must succeed");
    let v = output.into_value();

    assert_eq!(
        v["description"],
        Value::Null,
        "absent description must serialize as JSON null"
    );
}
