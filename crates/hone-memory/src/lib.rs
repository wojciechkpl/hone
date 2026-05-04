pub mod bm25;
pub mod embedding;
pub mod encrypted;
mod error;
mod in_memory;
pub mod sqlite_store;
mod store;
mod types;

pub use embedding::{EmbedError, Embedder, Embedding, ScoredMemory, cosine_similarity, rrf_fuse};
pub use encrypted::EncryptedStore;
pub use error::MemoryError;
pub use in_memory::InMemoryStore;
pub use sqlite_store::SqliteStore;
pub use store::MemoryStore;
pub use types::{Memory, MemoryFilter, MemoryKind, MemoryQuery};
