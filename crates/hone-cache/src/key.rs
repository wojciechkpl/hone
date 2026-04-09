use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Hash-based cache key identifying a cacheable operation.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CacheKey {
    /// Logical namespace, e.g. "tool:read_file", "tool:grep", "llm:replay".
    /// Used for targeted invalidation (drop all entries under a namespace).
    pub namespace: String,
    /// 32-byte content hash (blake3). Deterministic and portable across
    /// Rust versions and platforms.
    pub hash: [u8; 32],
    /// Invalidation policy attached to this key.
    pub invalidator: InvalidationTag,
}

impl CacheKey {
    /// Build a key from a namespace and raw bytes using blake3 for a
    /// deterministic, portable 32-byte hash.
    pub fn from_bytes(
        namespace: impl Into<String>,
        bytes: &[u8],
        invalidator: InvalidationTag,
    ) -> Self {
        let hash: [u8; 32] = *blake3::hash(bytes).as_bytes();
        Self {
            namespace: namespace.into(),
            hash,
            invalidator,
        }
    }

    /// Lowercase hex string of the hash. Useful for SQLite storage.
    pub fn hash_hex(&self) -> String {
        self.hash.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// Invalidation policy for a cached entry.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InvalidationTag {
    /// Never invalidate (caller controls).
    None,
    /// Expire at a specific unix timestamp (seconds).
    Ttl(i64),
    /// Invalidate when the file at `path` changes (content hash mismatch).
    FileHash { path: PathBuf, hash: [u8; 32] },
    /// Invalidate when the git HEAD SHA changes or the worktree becomes dirty.
    GitSha {
        repo: PathBuf,
        sha: String,
        dirty: bool,
    },
    /// Invalidate when the index epoch counter changes.
    IndexEpoch(u64),
}

impl InvalidationTag {
    /// Check whether this tag is currently valid (i.e., the cache entry should
    /// still be considered live).
    pub fn is_valid(&self, now: i64) -> bool {
        match self {
            Self::None => true,
            Self::Ttl(expiry) => now < *expiry,
            Self::FileHash { path, hash } => match std::fs::read(path) {
                Ok(content) => {
                    let current = hash_bytes(&content);
                    &current == hash
                }
                Err(_) => false, // file gone = invalid
            },
            Self::GitSha { repo, sha, dirty } => {
                // For v1, check the git HEAD SHA. Entries stored while the
                // worktree was dirty are treated as immediately invalid on
                // next lookup (they should never have been cached).
                if *dirty {
                    return false;
                }
                match current_git_sha(repo) {
                    Some(current) => &current == sha,
                    None => false,
                }
            }
            Self::IndexEpoch(_) => true, // caller must compare externally
        }
    }
}

fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    *blake3::hash(bytes).as_bytes()
}

/// Read the current HEAD SHA using libgit2 instead of spawning a `git`
/// subprocess. This avoids blocking the Tokio runtime when called from
/// async contexts (libgit2 is pure in-process I/O).
fn current_git_sha(repo_path: &std::path::Path) -> Option<String> {
    let repo = git2::Repository::discover(repo_path).ok()?;
    let head = repo.head().ok()?;
    let oid = head.target()?;
    Some(oid.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_bytes_produces_32_byte_hash() {
        let key = CacheKey::from_bytes("tool:read_file", b"hello world", InvalidationTag::None);
        assert_eq!(key.hash.len(), 32);
    }

    #[test]
    fn test_hash_hex_is_64_chars() {
        let key = CacheKey::from_bytes("tool:read_file", b"hello world", InvalidationTag::None);
        assert_eq!(key.hash_hex().len(), 64);
    }

    #[test]
    fn test_same_input_produces_same_key() {
        let a = CacheKey::from_bytes("ns", b"data", InvalidationTag::None);
        let b = CacheKey::from_bytes("ns", b"data", InvalidationTag::None);
        assert_eq!(a.hash, b.hash);
    }

    #[test]
    fn test_different_input_produces_different_key() {
        let a = CacheKey::from_bytes("ns", b"data1", InvalidationTag::None);
        let b = CacheKey::from_bytes("ns", b"data2", InvalidationTag::None);
        assert_ne!(a.hash, b.hash);
    }

    #[test]
    fn test_ttl_valid_before_expiry() {
        let tag = InvalidationTag::Ttl(i64::MAX);
        assert!(tag.is_valid(0));
    }

    #[test]
    fn test_ttl_invalid_after_expiry() {
        let tag = InvalidationTag::Ttl(1000);
        assert!(!tag.is_valid(2000));
    }

    #[test]
    fn test_none_is_always_valid() {
        assert!(InvalidationTag::None.is_valid(i64::MAX));
    }

    #[test]
    fn test_index_epoch_is_always_valid_inline() {
        assert!(InvalidationTag::IndexEpoch(42).is_valid(0));
    }

    #[test]
    fn test_dirty_git_sha_is_invalid() {
        let tag = InvalidationTag::GitSha {
            repo: PathBuf::from("/nonexistent"),
            sha: "abc".to_string(),
            dirty: true,
        };
        assert!(!tag.is_valid(0));
    }

    #[test]
    fn test_file_hash_invalid_when_file_missing() {
        let tag = InvalidationTag::FileHash {
            path: PathBuf::from("/nonexistent/path/file.txt"),
            hash: [0u8; 32],
        };
        assert!(!tag.is_valid(0));
    }
}
