//! The document types and traits of workspace retrieval: a `Document` is cut
//! into `Chunk`s by a `Chunker`, and a `Retriever` turns a query into `Hit`s.

use async_trait::async_trait;
use hone_proto::{SourceRef, StrategyId};

use crate::error::IndexError;

/// A file's text with its hash.
#[derive(Debug, Clone, PartialEq)]
pub struct Document {
    /// Path relative to the working directory, with `/` separators.
    pub uri: String,
    pub text: String,
    pub sha256: String,
}

/// A contiguous piece of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// `<doc_uri>#<start_line>`, stable while the lines before it don't move.
    pub id: String,
    pub doc_uri: String,
    /// 1-based, inclusive.
    pub start_line: u32,
    /// 1-based, inclusive.
    pub end_line: u32,
    /// Heading path (`Setup > Linux`), when the chunker knows one.
    pub heading: Option<String>,
    /// The chunk's lines joined with `\n`.
    pub text: String,
    /// SHA-256 (hex) of `text`.
    pub sha256: String,
}

/// One retrieved chunk.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub source: SourceRef,
    pub text: String,
    /// Higher is better.
    pub score: f32,
}

/// Cuts a document into chunks.
pub trait Chunker: Send + Sync {
    fn id(&self) -> &StrategyId;
    fn chunk(&self, doc: &Document) -> Vec<Chunk>;
}

/// Finds the chunks relevant to a query.
#[async_trait]
pub trait Retriever: Send + Sync {
    fn id(&self) -> &StrategyId;
    async fn retrieve(&self, query: &str, k: usize) -> Result<Vec<Hit>, IndexError>;
}
