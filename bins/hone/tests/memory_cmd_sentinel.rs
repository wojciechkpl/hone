//! ES6 of `docs/plans/encrypted-store-safety-spec.md` for the CLI: `hone memory ls` and
//! `hone memory show` hide the encrypted store's salt sentinel row
//! (`__encrypted_store_salt__`). Same linking strategy as `memory_cmd_test.rs`.

#[path = "../src/memory_cmd.rs"]
mod memory_cmd;

use std::sync::Arc;

use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use memory_cmd::{MemoryCmd, run};

const SENTINEL: &str = "__encrypted_store_salt__";
const SALT: &str = "QUFBQUFBQUFBQUFBQUFBQQ==";

fn memory(id: &str, content: &str, kind: MemoryKind, created_at: u64) -> Memory {
    Memory {
        id: id.to_owned(),
        content: content.to_owned(),
        tags: vec![],
        weight: 1.0,
        created_at,
        decay_at: None,
        kind,
        description: None,
        embedding: None,
    }
}

fn sentinel() -> Memory {
    Memory {
        id: SENTINEL.to_owned(),
        content: SALT.to_owned(),
        tags: vec!["__internal__".to_owned()],
        weight: 0.0,
        created_at: 0,
        decay_at: None,
        kind: MemoryKind::Conversation,
        description: None,
        embedding: None,
    }
}

async fn store() -> Arc<dyn MemoryStore> {
    let inner = InMemoryStore::new();
    inner.record(sentinel()).await.unwrap();
    inner.record(memory("a1", "alpha content", MemoryKind::Conversation, 1_000)).await.unwrap();
    inner.record(memory("b2", "beta content", MemoryKind::Feedback, 2_000)).await.unwrap();
    Arc::new(inner)
}

async fn capture(cmd: MemoryCmd) -> anyhow::Result<String> {
    let mut buf: Vec<u8> = Vec::new();
    run(cmd, store().await, &mut buf).await?;
    Ok(String::from_utf8(buf).unwrap())
}

#[tokio::test]
async fn test_es6_ls_should_not_list_the_salt_sentinel() {
    let out = capture(MemoryCmd::Ls { limit: None, kind: None }).await.unwrap();

    assert!(out.contains("alpha content") || out.contains("a1"), "{out}");
    assert!(out.contains("beta content") || out.contains("b2"), "{out}");
    assert!(!out.contains("__encryp"), "the sentinel is an implementation detail: {out}");
    assert!(!out.contains(SALT), "{out}");
    assert!(!out.contains("__internal__"), "{out}");
}

#[tokio::test]
async fn test_es6_ls_by_conversation_kind_should_not_list_the_salt_sentinel() {
    let out = capture(MemoryCmd::Ls { limit: None, kind: Some("conversation".to_owned()) }).await.unwrap();

    assert!(out.contains("alpha content") || out.contains("a1"), "{out}");
    assert!(!out.contains("__encryp") && !out.contains(SALT), "{out}");
}

#[tokio::test]
async fn test_es6_a_generous_limit_should_still_not_list_the_sentinel() {
    let out = capture(MemoryCmd::Ls { limit: Some(3), kind: None }).await.unwrap();

    assert!(!out.contains("__encryp"), "{out}");
    assert!(
        (out.contains("alpha content") || out.contains("a1")) && (out.contains("beta content") || out.contains("b2")),
        "both ordinary memories fit once the sentinel is hidden: {out}"
    );
}

#[tokio::test]
async fn test_es6_show_should_not_reveal_the_salt_sentinel() {
    for id in [SENTINEL, "__encrypted"] {
        let result = capture(MemoryCmd::Show { id: id.to_owned() }).await;
        match result {
            Err(e) => assert!(e.to_string().contains("not found"), "{id}: {e}"),
            Ok(out) => assert!(!out.contains(SALT) && !out.contains("__internal__"), "{id}: {out}"),
        }
    }
}

#[tokio::test]
async fn test_es6_show_should_still_show_ordinary_memories() {
    let out = capture(MemoryCmd::Show { id: "a1".to_owned() }).await.unwrap();
    assert!(out.contains("alpha content"), "{out}");
}
