use std::path::Path;

use async_trait::async_trait;

use crate::error::MemoryError;
use crate::types::{Memory, MemoryFilter, MemoryQuery};

#[async_trait]
pub trait MemoryStore: Send + Sync {
    async fn record(&mut self, memory: Memory) -> Result<(), MemoryError>;

    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError>;

    async fn forget(&mut self, filter: &MemoryFilter) -> Result<u32, MemoryError>;

    /// Multiply all weights by `factor`. Remove memories whose `decay_at` is
    /// `Some(t)` with `t <= now`. Returns the count of memories removed.
    async fn decay(&mut self, factor: f64, now: u64) -> Result<u32, MemoryError>;

    async fn export(&self, path: &Path) -> Result<(), MemoryError>;

    async fn import(path: &Path) -> Result<Self, MemoryError>
    where
        Self: Sized;
}
