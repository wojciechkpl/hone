#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G36 (compile-time part) of `docs/plans/fts5-recall-spec.md`, review round 5: the atomic and
//! sealed-vector operations are an explicit capability, `hone_memory::SealedVectorStore`, with
//! NO defaults, and `EncryptedStore<S>` requires `S: MemoryStore + SealedVectorStore`. A wrapper
//! that does not forward it cannot be wrapped (a compile error, not a runtime failure after a
//! write). These tests fail to COMPILE until the trait exists; once it does they pin:
//! - a forwarding wrapper (implementing both traits) works end to end under `EncryptedStore`;
//! - `record_sealed(memory, Option<blob>)` writes the row and its sealed vector together, and
//!   `None` clears a previous sealed vector, for `SqliteStore` and `InMemoryStore`.
//!
//! Trait methods (copied from the current defaulted `MemoryStore` methods): `record_if_absent`,
//! `set_description_if_absent`, `has_encrypted_rows`, `sealed_vector`,
//! `vector_migration_candidates`, `move_vector_to_sealed`, plus `record_sealed`. Not testable
//! without trybuild: that a store without the trait is rejected by the compiler.

mod fts5_support;

use std::path::Path;

use async_trait::async_trait;
use fts5_support::{mem, q};
use hone_memory::{
    EncryptedStore, InMemoryStore, Memory, MemoryError, MemoryFilter, MemoryKind, MemoryQuery,
    MemoryStore, SealedVectorStore, SqliteStore,
};

/// Forwards every method of both traits.
struct Forward(SqliteStore);

#[async_trait]
impl MemoryStore for Forward {
    async fn record(&self, memory: Memory) -> Result<(), MemoryError> {
        self.0.record(memory).await
    }
    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        self.0.recall(query).await
    }
    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        self.0.recall_by_id(id).await
    }
    async fn forget(&self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        self.0.forget(filter).await
    }
    async fn decay(&self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        self.0.decay(factor, now).await
    }
    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        self.0.export(path).await
    }
    async fn import(_path: &Path) -> Result<Self, MemoryError> {
        Err(MemoryError::Io(std::io::Error::other("not supported")))
    }
}

#[async_trait]
impl SealedVectorStore for Forward {
    async fn record_if_absent(&self, memory: Memory) -> Result<bool, MemoryError> {
        self.0.record_if_absent(memory).await
    }
    async fn set_description_if_absent(&self, id: &str, description: &str) -> Result<bool, MemoryError> {
        self.0.set_description_if_absent(id, description).await
    }
    async fn has_encrypted_rows(&self) -> Result<bool, MemoryError> {
        self.0.has_encrypted_rows().await
    }
    async fn sealed_vector(&self, id: &str) -> Result<Option<Vec<u8>>, MemoryError> {
        self.0.sealed_vector(id).await
    }
    async fn vector_migration_candidates(&self) -> Result<Vec<(String, Vec<u8>)>, MemoryError> {
        self.0.vector_migration_candidates().await
    }
    async fn move_vector_to_sealed(&self, id: &str, blob: &[u8]) -> Result<(), MemoryError> {
        self.0.move_vector_to_sealed(id, blob).await
    }
    async fn record_sealed(&self, memory: Memory, sealed: Option<Vec<u8>>) -> Result<(), MemoryError> {
        self.0.record_sealed(memory, sealed).await
    }
}

#[tokio::test]
async fn test_g36_a_forwarding_wrapper_should_work_under_the_encrypted_store() {
    let store = EncryptedStore::new(Forward(SqliteStore::in_memory().await.unwrap()), "pw").await.unwrap();
    let mut m = mem("x", "a note about quokkas", MemoryKind::Feedback, 1);
    m.embedding = Some(vec![0.5, 0.25, 0.125, 1.0]);

    store.record(m).await.expect("record never errors after writing");

    let got = store.recall(&q("quokkas", 3, None)).await.unwrap();
    assert_eq!(got[0].id, "x");
    assert_eq!(got[0].embedding.as_deref(), Some(&[0.5f32, 0.25, 0.125, 1.0][..]));
}

macro_rules! record_sealed_contract {
    ($name:ident, $store:expr) => {
        #[tokio::test]
        async fn $name() {
            let store = $store;
            let blob = vec![7u8; 40];

            store.record_sealed(mem("x", "enc:v1:AAAA", MemoryKind::Feedback, 1), Some(blob.clone())).await.unwrap();

            assert!(store.recall_by_id("x").await.unwrap().is_some(), "the row was written");
            assert_eq!(store.sealed_vector("x").await.unwrap(), Some(blob), "with its sealed vector");

            store.record_sealed(mem("x", "enc:v1:BBBB", MemoryKind::Feedback, 2), None).await.unwrap();

            assert_eq!(store.recall_by_id("x").await.unwrap().unwrap().content, "enc:v1:BBBB");
            assert_eq!(store.sealed_vector("x").await.unwrap(), None, "None clears the old sealed vector");
        }
    };
}

record_sealed_contract!(test_g36_record_sealed_should_write_row_and_vector_together_in_a_sqlite_store, SqliteStore::in_memory().await.unwrap());
record_sealed_contract!(test_g36_record_sealed_should_write_row_and_vector_together_in_an_in_memory_store, InMemoryStore::new());
