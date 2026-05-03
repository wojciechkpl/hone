//! Tamper-evident hash chain over agent run events.
//!
//! Each entry's `prev_hash` field carries the SHA-256 hex digest of the
//! previous entry's serialised body. Verifying the chain (start to end)
//! detects any insertion, deletion, or modification.

mod chain;
mod event;

pub use chain::{ProvenanceLog, ProvenanceLogError, VerifyError};
pub use event::{ProvenanceEntry, ProvenanceEvent};
