use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::bm25::{Bm25Index, Bm25Params};
use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::{Memory, MemoryFilter, MemoryQuery};

pub struct InMemoryStore {
    memories: Arc<RwLock<HashMap<String, Memory>>>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            memories: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn record(&self, memory: Memory) -> Result<(), MemoryError> {
        self.memories.write().await.insert(memory.id.clone(), memory);
        Ok(())
    }

    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        // Collect memories into a stable-ordered Vec so that BM25 doc indices
        // correspond to positions in this slice.  Sort by id to make doc-index
        // assignment deterministic across runs (HashMap iteration order is not).
        let guard = self.memories.read().await;
        let mut all: Vec<&Memory> = guard.values().collect();
        all.sort_by(|a, b| a.id.cmp(&b.id));

        // Apply kind filter BEFORE limit (and before BM25 scoring).
        if let Some(kind) = query.kind {
            all.retain(|m| m.kind == kind);
        }

        // Empty-query short-circuit: return all memories unfiltered (by kind already applied).
        if query.text.is_empty() {
            let mut scored: Vec<(f64, &Memory)> =
                all.iter().map(|m| (m.weight, *m)).collect();
            scored.sort_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
            let results = scored
                .into_iter()
                .take(query.limit)
                .map(|(_, m)| m.clone())
                .collect();
            return Ok(results);
        }

        // Build a BM25 index over the searchable text of every (kind-filtered) memory.
        // Searchable text = description (if present) + "\n" + content.
        let texts: Vec<String> = all
            .iter()
            .map(|m| {
                let desc = m.description.as_deref().unwrap_or_default();
                format!("{desc}\n{}", m.content)
            })
            .collect();
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let index = Bm25Index::build(&refs, Bm25Params::default());

        let mut scored: Vec<(f64, &Memory)> = all
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let bm25 = index.score(i, &query.text);
                if bm25 > 0.0 {
                    Some((bm25 * m.weight, *m))
                } else {
                    None
                }
            })
            .collect();

        // Descending by combined score; stable sort for determinism on ties.
        scored.sort_by(|(a, _), (b, _)| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

        let results = scored
            .into_iter()
            .take(query.limit)
            .map(|(_, m)| m.clone())
            .collect();

        Ok(results)
    }

    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        Ok(self.memories.read().await.get(id).cloned())
    }

    async fn forget(&self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        let mut guard = self.memories.write().await;
        let before = guard.len();
        match filter {
            MemoryFilter::Tag(tag) => {
                guard.retain(|_, m| !m.tags.contains(tag));
            }
            MemoryFilter::Before(ts) => {
                guard.retain(|_, m| m.created_at >= *ts);
            }
            MemoryFilter::After(ts) => {
                guard.retain(|_, m| m.created_at <= *ts);
            }
            MemoryFilter::Kind(kind) => {
                guard.retain(|_, m| m.kind != *kind);
            }
        }
        let removed = before.saturating_sub(guard.len()) as u32;
        Ok(removed)
    }

    async fn decay(&self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        let mut guard = self.memories.write().await;
        let before = guard.len();

        // Apply weight decay and remove expired memories in one pass.
        guard.retain(|_, m| {
            m.weight *= factor;
            !matches!(m.decay_at, Some(t) if t <= now)
        });

        let removed = before.saturating_sub(guard.len()) as u32;
        Ok(removed)
    }

    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        let guard = self.memories.read().await;
        let memories: Vec<&Memory> = guard.values().collect();
        let json = serde_json::to_string_pretty(&memories)?;
        drop(guard);
        tokio::fs::write(path, json).await?;
        Ok(())
    }

    async fn import(path: &Path) -> Result<Self, MemoryError> {
        let bytes = tokio::fs::read(path).await?;
        let memories: Vec<Memory> = serde_json::from_slice(&bytes)?;
        let map = memories.into_iter().map(|m| (m.id.clone(), m)).collect();
        Ok(Self {
            memories: Arc::new(RwLock::new(map)),
        })
    }
}
