//! Red-phase tests for R9: `build_memory_store` helper that wires the
//! `--encrypted-memory` CLI flag and `HONE_MEMORY_PASSPHRASE` env var to the
//! existing `EncryptedStore` wrapper.
//!
//! Every test in this file MUST fail to compile on `main` because the
//! `memory_init` module does not exist yet.
//!
//! # Contract assumed by the implementer
//!
//! A new module `bins/hone/src/memory_init.rs` (or equivalent `pub` module
//! pulled out of `main.rs`) must expose:
//!
//! ```rust,ignore
//! pub async fn build_memory_store(
//!     sqlite: SqliteStore,
//!     passphrase: Option<&str>,
//! ) -> Result<Arc<Mutex<dyn MemoryStore>>, anyhow::Error>;
//! ```
//!
//! - `passphrase = None`  → return the bare `SqliteStore` as a trait object.
//! - `passphrase = Some(s)` → wrap with `EncryptedStore::new(sqlite, s)` and
//!   return the encrypted store as a trait object.
//! - The "env var missing" check (`--encrypted-memory` set but env var absent)
//!   is the *caller's* responsibility; `build_memory_store` does NOT do it.
//! - An empty passphrase (`Some("")`) is accepted — no extra validation.
//!
//! # Linking strategy
//!
//! `bins/hone` has no `[lib]` target, so the module is imported with `#[path]`
//! mirroring the pattern used by `system_prompt_test.rs` and
//! `memory_cmd_test.rs`.
//!
//! # Test isolation
//!
//! All tests that touch SQLite use a `tempfile::TempDir`-scoped path and call
//! `SqliteStore::open_with_decay_interval(&path, 0)` to skip time-gating, so
//! drop-and-reopen within the same test sees consistent state.

#[path = "../src/memory_init.rs"]
mod memory_init;

use std::sync::Arc;

use hone_memory::{Memory, MemoryKind, MemoryQuery, MemoryStore, SqliteStore};
use memory_init::build_memory_store;

// ---------------------------------------------------------------------------
// Shared helper — construct a bare Memory record for insertion.
// ---------------------------------------------------------------------------

fn make_memory(id: &str, content: &str) -> Memory {
    Memory {
        id: id.to_string(),
        content: content.to_string(),
        tags: vec![],
        weight: 1.0,
        created_at: 1_000,
        decay_at: None,
        kind: MemoryKind::Conversation,
        description: None,
        embedding: None,
    }
}

// ---------------------------------------------------------------------------
// Test 1 — None passphrase returns a plain (unencrypted) SqliteStore.
//
// Proof: insert via the helper, drop, re-open a *second* bare SqliteStore on
// the same path (no passphrase), recall the content — it must match exactly.
// If encryption were applied the raw DB row would be ciphertext, and a bare
// open would also read ciphertext back, so the recalled content would NOT
// equal the original plaintext (or would return a decryption error). Using a
// SECOND bare open is the simplest way to prove the first open wrote plaintext.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_memory_store_without_passphrase_returns_plain_sqlite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("plain.db");

    // First open: insert a memory through the helper with passphrase = None.
    {
        let sqlite = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("open sqlite");
        let store: Arc<dyn MemoryStore> = build_memory_store(sqlite, None)
            .await
            .expect("build_memory_store None");

        store
            .record(make_memory("plain-id", "hello-plaintext"))
            .await
            .expect("record");
    }

    // Second open: re-open with None and recall — must return the exact plaintext.
    {
        let sqlite2 = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("reopen sqlite");
        let store2: Arc<dyn MemoryStore> = build_memory_store(sqlite2, None)
            .await
            .expect("build_memory_store None (second)");

        let results = store2
            .recall(&MemoryQuery {
                text: "hello-plaintext".to_string(),
                limit: 10,
                kind: None,
            })
            .await
            .expect("recall");

        assert_eq!(
            results.len(),
            1,
            "plain store must recall the inserted memory; got {} results",
            results.len()
        );
        assert_eq!(
            results[0].content, "hello-plaintext",
            "content must survive the roundtrip unchanged through a plain store"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 2 — Some(passphrase) wraps with EncryptedStore.
//
// Proof: insert via the encrypted helper, then open the SAME path with
// passphrase = None (bare SqliteStore) and recall "secret-payload-xyz".
// The raw DB bytes are AES-256-GCM ciphertext stored as `enc:v1:<base64>`.
// A bare open reads those raw bytes back as the "content" field, so the
// recalled content will NOT contain the literal "secret-payload-xyz".
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_memory_store_with_passphrase_wraps_encrypted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("encrypted.db");

    // Write via encrypted store.
    {
        let sqlite = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("open sqlite");
        let store: Arc<dyn MemoryStore> = build_memory_store(sqlite, Some("hunter2"))
            .await
            .expect("build_memory_store Some");

        store
            .record(make_memory("enc-id", "secret-payload-xyz"))
            .await
            .expect("record through encrypted store");
    }

    // Re-open plain (no passphrase) and verify the raw content is NOT the plaintext.
    {
        let sqlite2 = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("reopen sqlite plain");
        let store2: Arc<dyn MemoryStore> = build_memory_store(sqlite2, None)
            .await
            .expect("build_memory_store None on encrypted db");

        // Fetch everything so we catch any partially-matching row.
        let results = store2
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 100,
                kind: None,
            })
            .await
            .expect("recall from plain store on encrypted db");

        // The plaintext must not appear in any row's content.
        for mem in &results {
            assert!(
                !mem.content.contains("secret-payload-xyz"),
                "plain open of an encrypted DB must NOT reveal the plaintext in any row; \
                 got content: {:?}",
                mem.content
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Test 3 — Roundtrip with the same passphrase decrypts correctly.
//
// Write via encrypted helper, drop, re-open with the SAME passphrase, recall —
// content must equal the original plaintext.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_memory_store_roundtrips_with_same_passphrase() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("roundtrip.db");

    // First open: write.
    {
        let sqlite = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("open sqlite");
        let store: Arc<dyn MemoryStore> = build_memory_store(sqlite, Some("hunter2"))
            .await
            .expect("build encrypted store (first open)");

        store
            .record(make_memory("rt-id", "secret-payload-xyz"))
            .await
            .expect("record");
    }

    // Second open: re-derive key from persisted salt, decrypt, recall.
    {
        let sqlite2 = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("reopen sqlite");
        let store2: Arc<dyn MemoryStore> = build_memory_store(sqlite2, Some("hunter2"))
            .await
            .expect("build encrypted store (second open)");

        let results = store2
            .recall(&MemoryQuery {
                text: "secret-payload-xyz".to_string(),
                limit: 10,
                kind: None,
            })
            .await
            .expect("recall after reopen with same passphrase");

        assert_eq!(
            results.len(),
            1,
            "roundtrip with same passphrase must recall exactly 1 record; got {}",
            results.len()
        );
        assert_eq!(
            results[0].content, "secret-payload-xyz",
            "roundtrip content must equal the original plaintext"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 4 — Wrong passphrase returns an error on recall.
//
// This pins the `EncryptedStore` behaviour: the store opens successfully
// (salt sentinel is present, a key is derived), but AES-GCM decryption fails
// when the wrong key is applied to the stored ciphertext. `recall` therefore
// returns `Err(...)` rather than silently returning garbage.
//
// If `EncryptedStore` ever changes to return empty results instead of an error,
// the assert below must be updated to match the new behaviour.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_memory_store_fails_with_wrong_passphrase() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("wrong_pw.db");

    // Write with the correct passphrase.
    {
        let sqlite = SqliteStore::open_with_decay_interval(&db_path, 0)
            .await
            .expect("open sqlite");
        let store: Arc<dyn MemoryStore> = build_memory_store(sqlite, Some("hunter2"))
            .await
            .expect("build encrypted store");

        store
            .record(make_memory("wp-id", "sensitive content"))
            .await
            .expect("record");
    }

    // Re-open with a wrong passphrase.
    let sqlite2 = SqliteStore::open_with_decay_interval(&db_path, 0)
        .await
        .expect("reopen sqlite");
    let store2: Arc<dyn MemoryStore> = build_memory_store(sqlite2, Some("wrong-passphrase"))
        .await
        .expect("build with wrong passphrase must succeed (salt sentinel is present)");

    // Recall must fail because the ciphertext cannot be decrypted with the wrong key.
    let result = store2
        .recall(&MemoryQuery {
            text: String::new(),
            limit: 10,
            kind: None,
        })
        .await;

    assert!(
        result.is_err(),
        "recall through an EncryptedStore opened with the wrong passphrase must return Err; \
         got Ok({:?})",
        result.ok()
    );
}

// ---------------------------------------------------------------------------
// Test 5 — Empty passphrase string is accepted without extra validation.
//
// `build_memory_store` must NOT inject additional validation that rejects
// `Some("")`. An empty passphrase is low-entropy but structurally valid input
// to Argon2id / AES-256-GCM. The "--encrypted-memory requires env var" check
// lives at the CLI caller level, not inside `build_memory_store`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_memory_store_with_empty_passphrase_string_still_works() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("empty_pw.db");

    let sqlite = SqliteStore::open_with_decay_interval(&db_path, 0)
        .await
        .expect("open sqlite");

    // This must not return Err — empty string is a valid (if weak) passphrase.
    let store: Arc<dyn MemoryStore> = build_memory_store(sqlite, Some("")).await.expect(
        "build_memory_store with Some(\"\") must succeed; \
             no extra validation should reject an empty passphrase",
    );

    // Insert and recall to prove the store is usable, not just constructed.
    store
        .record(make_memory("ep-id", "content with empty passphrase"))
        .await
        .expect("record through empty-passphrase encrypted store");

    let results = store
        .recall(&MemoryQuery {
            text: "content with empty passphrase".to_string(),
            limit: 5,
            kind: None,
        })
        .await
        .expect("recall through empty-passphrase encrypted store");

    assert_eq!(
        results.len(),
        1,
        "empty-passphrase encrypted store must recall the inserted memory"
    );
    assert_eq!(results[0].content, "content with empty passphrase");
}
