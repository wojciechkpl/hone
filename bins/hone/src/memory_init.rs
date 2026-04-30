//! Helper for constructing the memory store from CLI/env configuration.
//!
//! Extracted into its own module so that integration tests can import it
//! directly via `#[path = "../src/memory_init.rs"] mod memory_init;` without
//! requiring a `[lib]` target in the `hone` binary crate.

use std::sync::Arc;

use anyhow::Result;
use hone_memory::{EncryptedStore, MemoryStore, SqliteStore};
use tokio::sync::Mutex;

/// Build a [`MemoryStore`] trait object, optionally wrapped with encryption.
///
/// - `passphrase = None`  — returns the bare `SqliteStore` as a trait object.
/// - `passphrase = Some(s)` — wraps with `EncryptedStore::new(sqlite, s)` and
///   returns the encrypted store as a trait object.
///
/// The "env var missing" check (`--encrypted-memory` set but
/// `HONE_MEMORY_PASSPHRASE` absent) is the **caller's** responsibility;
/// this function does not perform any extra passphrase validation and accepts
/// an empty string as a valid (if low-entropy) passphrase.
pub async fn build_memory_store(
    sqlite: SqliteStore,
    passphrase: Option<&str>,
) -> Result<Arc<Mutex<dyn MemoryStore>>> {
    match passphrase {
        None => Ok(Arc::new(Mutex::new(sqlite))),
        Some(pp) => {
            let enc = EncryptedStore::new(sqlite, pp).await?;
            Ok(Arc::new(Mutex::new(enc)))
        }
    }
}
