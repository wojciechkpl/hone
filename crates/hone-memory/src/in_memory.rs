use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::{Memory, MemoryFilter, MemoryQuery};

pub struct InMemoryStore {
    memories: HashMap<String, Memory>,
}

impl InMemoryStore {
    pub fn new() -> Self {
        Self {
            memories: HashMap::new(),
        }
    }
}

impl Default for InMemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Score a memory against a query text.
///
/// Returns a non-negative float. The score is the number of (overlapping)
/// case-insensitive occurrences of the query string inside the memory content.
/// An empty query matches everything with a score of 1.
fn relevance_score(memory: &Memory, query_lower: &str) -> f64 {
    if query_lower.is_empty() {
        return 1.0;
    }
    let content_lower = memory.content.to_lowercase();
    let mut count = 0usize;
    let mut start = 0;
    while let Some(pos) = content_lower[start..].find(query_lower) {
        count += 1;
        start += pos + 1;
    }
    count as f64
}

#[async_trait]
impl MemoryStore for InMemoryStore {
    async fn record(&mut self, memory: Memory) -> Result<(), MemoryError> {
        self.memories.insert(memory.id.clone(), memory);
        Ok(())
    }

    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        let query_lower = query.text.to_lowercase();

        let mut scored: Vec<(f64, &Memory)> = self
            .memories
            .values()
            .filter_map(|m| {
                let score = relevance_score(m, &query_lower);
                if score > 0.0 {
                    Some((score * m.weight, m))
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

    async fn forget(&mut self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        let before = self.memories.len();
        match filter {
            MemoryFilter::Tag(tag) => {
                self.memories.retain(|_, m| !m.tags.contains(tag));
            }
            MemoryFilter::Before(ts) => {
                self.memories.retain(|_, m| m.created_at >= *ts);
            }
            MemoryFilter::After(ts) => {
                self.memories.retain(|_, m| m.created_at <= *ts);
            }
        }
        let removed = before.saturating_sub(self.memories.len()) as u32;
        Ok(removed)
    }

    async fn decay(&mut self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        let before = self.memories.len();

        // Apply weight decay and remove expired memories in one pass.
        self.memories.retain(|_, m| {
            m.weight *= factor;
            match m.decay_at {
                Some(t) if t <= now => false,
                _ => true,
            }
        });

        let removed = before.saturating_sub(self.memories.len()) as u32;
        Ok(removed)
    }

    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        let memories: Vec<&Memory> = self.memories.values().collect();
        let json = serde_json::to_string_pretty(&memories)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    async fn import(path: &Path) -> Result<Self, MemoryError> {
        let bytes = std::fs::read(path)?;
        let memories: Vec<Memory> = serde_json::from_slice(&bytes)?;
        let map = memories.into_iter().map(|m| (m.id.clone(), m)).collect();
        Ok(Self { memories: map })
    }
}
