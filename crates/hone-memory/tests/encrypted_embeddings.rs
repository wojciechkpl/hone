#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! G15 (second half) of `docs/plans/fts5-recall-spec.md`, review round 1: ranked recall over
//! an `EncryptedStore` decrypts and ranks the corpus WITHOUT loading its embeddings; only the
//! memories it returns carry them. Observed through a counting wrapper around the inner
//! store: the number of memories WITH an embedding that the inner store hands out through
//! listing calls (`recall`, `recall_excluding`) must not grow with the corpus. Returned
//! memories still have their embeddings (the wrapper's `recall_by_id` is not counted).
//! No new API.

mod fts5_support;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use fts5_support::{mem, q};
use hone_memory::{
    EncryptedStore, Memory, MemoryError, MemoryFilter, MemoryKind, MemoryQuery, MemoryStore,
    SqliteStore,
};

struct Counting {
    inner: SqliteStore,
    listed_with_embedding: Arc<AtomicUsize>,
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
    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        self.inner.recall_by_id(id).await
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

#[tokio::test]
async fn test_g15_ranked_encrypted_recall_should_not_load_embeddings_for_the_whole_corpus() {
    let counter = Arc::new(AtomicUsize::new(0));
    let inner = Counting {
        inner: SqliteStore::in_memory().await.unwrap(),
        listed_with_embedding: counter.clone(),
    };
    let store = EncryptedStore::new(inner, "correct horse").await.unwrap();
    for i in 0..60u64 {
        let mut m = mem(&format!("m{i}"), &format!("note {i} about quokkas and filler"), MemoryKind::Feedback, i);
        m.embedding = Some(vec![i as f32, 1.0, 0.5, 0.25]);
        store.record(m).await.unwrap();
    }
    counter.store(0, Ordering::SeqCst);

    let got = store.recall(&q("quokkas", 3, None)).await.unwrap();

    assert_eq!(got.len(), 3);
    let loaded = counter.load(Ordering::SeqCst);
    assert!(loaded <= 3, "the inner store listed {loaded} embeddings for a corpus of 60 and a limit of 3");
    assert!(got.iter().all(|m| m.embedding.is_some()), "the returned memories carry their embeddings");
}
