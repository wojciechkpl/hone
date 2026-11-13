pub mod bm25;
pub mod embedding;
pub mod encrypted;
mod error;
pub mod fts_query;
mod in_memory;
pub mod index;
pub mod sqlite_store;
mod store;
mod types;

pub use embedding::{EmbedError, Embedder, Embedding, ScoredMemory, cosine_similarity, rrf_fuse};
pub use encrypted::{EncryptedStore, OpenReport, SALT_SENTINEL_ID};
pub use error::MemoryError;
pub use in_memory::InMemoryStore;
pub use index::read_memory;
pub use sqlite_store::SqliteStore;
pub use store::{MemoryStore, SealedVectorStore, trigram_jaccard, visible_in_project};
pub use types::{Memory, MemoryFilter, MemoryKind, MemoryQuery};
