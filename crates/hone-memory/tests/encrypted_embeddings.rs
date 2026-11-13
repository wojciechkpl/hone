#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G15 (second half) of `docs/plans/fts5-recall-spec.md`, review round 1: ranked recall over
//! an `EncryptedStore` decrypts and ranks the corpus WITHOUT loading its embeddings; only the
//! memories it returns carry them. Observed through a counting wrapper around the inner
//! store: the number of memories WITH an embedding that the inner store hands out through
//! listing calls (`recall`, `recall_excluding`) must not grow with the corpus. Returned
//! memories still have their embeddings (the wrapper's `recall_by_id` is not counted).
//!
//! Since G29 (review round 4) sealed vectors live in a side table, so listings never carry an
//! embedding and the listing counter alone would be vacuous. The wrapper therefore also counts
//! `sealed_vector` fetches (the per-memory read of a sealed vector): ranked lexical recall of 3
//! memories out of 60 must fetch at most 3, not the whole corpus. The wrapper forwards every
//! defaulted `MemoryStore` method to the inner store, including the sealed-vector ones.

mod fts5_support;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use fts5_support::{mem, q};
use hone_memory::{
    EncryptedStore, Memory, MemoryError, MemoryFilter, MemoryKind, MemoryQuery, MemoryStore,
    SealedVectorStore, SqliteStore,
};

struct Counting {
    inner: SqliteStore,
    listed_with_embedding: Arc<AtomicUsize>,
    sealed_fetches: Arc<AtomicUsize>,
}

impl Counting {
    fn count(&self, v: &[Memory]) {
        self.listed_with_embedding
            .fetch_add(v.iter().filter(|m| m.embedding.is_some()).count(), Ordering::SeqCst);
    }
}

#[async_trait]
impl MemoryStore for Counting {
    async fn record(&self, memory: Memory) -> Result<(), MemoryError> {
        self.inner.record(memory).await
    }
    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        let v = self.inner.recall(query).await?;
        self.count(&v);
        Ok(v)
    }
    async fn recall_excluding(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
    ) -> Result<Vec<Memory>, MemoryError> {
        let v = self.inner.recall_excluding(query, exclude).await?;
        self.count(&v);
        Ok(v)
    }
    async fn recall_indexed(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
    ) -> Result<Option<Vec<hone_memory::ScoredMemory>>, MemoryError> {
        self.inner.recall_indexed(query, exclude).await
    }
    async fn reindex(&self) -> Result<Option<usize>, MemoryError> {
        self.inner.reindex().await
    }
    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        self.inner.recall_by_id(id).await
    }
    async fn recall_semantic(&self, query_embedding: &[f32], limit: usize) -> Result<Vec<hone_memory::ScoredMemory>, MemoryError> {
        self.inner.recall_semantic(query_embedding, limit).await
    }
    async fn find_similar(&self, query: &str, threshold: f64) -> Result<Option<Memory>, MemoryError> {
        self.inner.find_similar(query, threshold).await
    }
    async fn forget(&self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        self.inner.forget(filter).await
    }
    async fn decay(&self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        self.inner.decay(factor, now).await
    }
    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        self.inner.export(path).await
    }
    async fn import(_path: &Path) -> Result<Self, MemoryError> {
        Err(MemoryError::Io(std::io::Error::other("not supported")))
    }
}

#[async_trait]
impl SealedVectorStore for Counting {
    async fn record_if_absent(&self, memory: Memory) -> Result<bool, MemoryError> {
        self.inner.record_if_absent(memory).await
    }
    async fn set_description_if_absent(&self, id: &str, description: &str) -> Result<bool, MemoryError> {
        self.inner.set_description_if_absent(id, description).await
    }
    async fn sealed_vector(&self, id: &str) -> Result<Option<Vec<u8>>, MemoryError> {
        self.sealed_fetches.fetch_add(1, Ordering::SeqCst);
        self.inner.sealed_vector(id).await
    }
    async fn vector_migration_candidates(&self) -> Result<Vec<(String, Vec<u8>)>, MemoryError> {
        self.inner.vector_migration_candidates().await
    }
    async fn move_vector_to_sealed(&self, id: &str, blob: &[u8]) -> Result<(), MemoryError> {
        self.inner.move_vector_to_sealed(id, blob).await
    }
    async fn has_encrypted_rows(&self) -> Result<bool, MemoryError> {
        self.inner.has_encrypted_rows().await
    }
    async fn record_sealed(&self, memory: Memory, sealed: Option<Vec<u8>>) -> Result<(), MemoryError> {
        self.inner.record_sealed(memory, sealed).await
    }
}

#[tokio::test]
async fn test_g15_ranked_encrypted_recall_should_not_load_embeddings_for_the_whole_corpus() {
    let counter = Arc::new(AtomicUsize::new(0));
    let fetches = Arc::new(AtomicUsize::new(0));
    let inner = Counting {
        inner: SqliteStore::in_memory().await.unwrap(),
        listed_with_embedding: counter.clone(),
        sealed_fetches: fetches.clone(),
    };
    let store = EncryptedStore::new(inner, "correct horse").await.unwrap();
    for i in 0..60u64 {
        let mut m = mem(&format!("m{i}"), &format!("note {i} about quokkas and filler"), MemoryKind::Feedback, i);
        m.embedding = Some(vec![i as f32, 1.0, 0.5, 0.25]);
        store.record(m).await.unwrap();
    }
    counter.store(0, Ordering::SeqCst);
    fetches.store(0, Ordering::SeqCst);

    let got = store.recall(&q("quokkas", 3, None)).await.unwrap();

    assert_eq!(got.len(), 3);
    let loaded = counter.load(Ordering::SeqCst);
    assert!(loaded <= 3, "the inner store listed {loaded} embeddings for a corpus of 60 and a limit of 3");
    let fetched = fetches.load(Ordering::SeqCst);
    assert!(fetched <= 3, "{fetched} sealed vectors were fetched for a corpus of 60 and a limit of 3");
    assert!(got.iter().all(|m| m.embedding.is_some()), "the returned memories carry their embeddings");
}
