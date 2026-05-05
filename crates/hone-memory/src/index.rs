//! Memory index helpers shared across crates.
//!
//! Provides `read_memory`, a low-level helper that fetches the full body of a
//! single memory by its id.  The companion `render_index` function lives in
//! `hone-core::memory_index` because it depends on context-window budgeting
//! that belongs to the core orchestration layer.

use crate::{Memory, MemoryError, MemoryQuery, MemoryStore};

/// Fetch the full body of a single memory by its id.
///
/// Returns `Ok(Some(memory))` if the id is found, `Ok(None)` if the memory
/// does not exist or has been decayed/forgotten.
pub async fn read_memory(store: &dyn MemoryStore, id: &str) -> Result<Option<Memory>, MemoryError> {
    let memories = store
        .recall(&MemoryQuery {
            text: String::new(),
            limit: usize::MAX,
            kind: None,
        })
        .await?;

    Ok(memories.into_iter().find(|m| m.id == id))
}
