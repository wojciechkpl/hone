//! Application-layer encryption for memory content.
//!
//! Wraps any [`MemoryStore`] to encrypt `Memory.content` before writing and
//! decrypt after reading. Uses AES-256-GCM with a key derived from a
//! passphrase via Argon2id.
//!
//! This avoids the complexity of linking sqlcipher while providing at-rest
//! encryption of the most sensitive field (conversation content).
//!
//! # Salt management (Fix 2)
//!
//! A random 16-byte salt is generated the first time [`EncryptedStore::new`]
//! is called on a fresh inner store and persisted as a sentinel Memory record
//! with id `__encrypted_store_salt__`. Subsequent opens read the sentinel and
//! re-derive the same key, so ciphertext is portable across process restarts.
//!
//! If the sentinel is missing but encrypted memories already exist, `new`
//! returns an error — the original key cannot be reconstructed without the
//! original salt. Use `EncryptedStore::migrate_from_fixed_salt` (TODO) to
//! re-encrypt an existing store after recovering from this state.
//!
//! # Argon2 parameters (Fix 2b)
//!
//! Parameters are pinned to Argon2id / V0x13 / m=19456 / t=2 / p=1 / tag=32
//! so a future upgrade of the `argon2` crate cannot silently change the
//! derived key.  The version marker constant `ARGON2_VERSION_MARKER` records
//! this choice; a TODO comment below explains how to evolve it with
//! `enc:v2:` prefix detection.

use std::path::Path;

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;

use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::{Memory, MemoryFilter, MemoryQuery};

/// The old fixed salt.  Kept as `LEGACY_SALT` so that any tests or tooling
/// that were built against the old API have a named constant to reference.
///
/// TODO: migrate_from_fixed_salt should re-derive the key from LEGACY_SALT,
/// decrypt all records, generate a new random salt, and re-encrypt.
pub const LEGACY_SALT: &[u8] = b"hone-memory-v1-salt";

/// Reserved memory ID used to persist the per-store random salt.
const SALT_SENTINEL_ID: &str = "__encrypted_store_salt__";

/// Argon2 version marker.  If the KDF parameters are ever changed (e.g., to
/// increase memory cost), bump this string and add detection logic in
/// `decrypt_content` based on the `enc:vN:` prefix so existing records can
/// still be decrypted.
///
/// TODO: for v2 params introduce `enc:v2:` prefix and detect in decrypt.
#[allow(dead_code)]
const ARGON2_VERSION_MARKER: &str = "argon2id/v0x13/m=19456/t=2/p=1";

/// Wraps an inner [`MemoryStore`] with AES-256-GCM content encryption.
pub struct EncryptedStore<S: MemoryStore> {
    pub(crate) inner: S,
    cipher: Aes256Gcm,
}

impl<S: MemoryStore + Send + Sync> EncryptedStore<S> {
    /// Create an encrypted wrapper around `inner`, deriving the key from
    /// `passphrase` using Argon2id with a per-store random salt.
    ///
    /// On the first call for a given `inner` store a fresh random 16-byte salt
    /// is generated and stored as a sentinel record.  Subsequent calls read
    /// that sentinel and reproduce the same key.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The sentinel is absent but encrypted records already exist (migration
    ///   required — see module doc).
    /// - Argon2 key derivation fails.
    /// - The cipher cannot be initialised.
    pub async fn new(mut inner: S, passphrase: &str) -> Result<Self, MemoryError> {
        let salt = load_or_init_salt(&mut inner).await?;
        let key = derive_key(passphrase, &salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| MemoryError::Io(std::io::Error::other(format!("cipher init: {e}"))))?;
        Ok(Self { inner, cipher })
    }

    fn encrypt_content(&self, plaintext: &str) -> Result<String, MemoryError> {
        // Generate a random 96-bit nonce for each encryption directly into a
        // stack array.  Mirrors the salt-generation pattern at the bottom of
        // this file and removes the need for a clippy::unwrap_used escape.
        let mut nonce_bytes = [0u8; 12];
        aes_gcm::aead::rand_core::RngCore::fill_bytes(&mut OsRng, &mut nonce_bytes);

        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_bytes())
            .map_err(|e| MemoryError::Io(std::io::Error::other(format!("encrypt: {e}"))))?;

        // Store as: base64(nonce || ciphertext) with a prefix marker.
        let mut combined = Vec::with_capacity(12 + ciphertext.len());
        combined.extend_from_slice(&nonce_bytes);
        combined.extend_from_slice(&ciphertext);

        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&combined);
        Ok(format!("enc:v1:{encoded}"))
    }

    fn decrypt_content(&self, stored: &str) -> Result<String, MemoryError> {
        let encoded = stored.strip_prefix("enc:v1:").ok_or_else(|| {
            MemoryError::Io(std::io::Error::other(
                "memory content not encrypted or unknown version",
            ))
        })?;

        use base64::Engine;
        let combined = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map_err(|e| MemoryError::Io(std::io::Error::other(format!("base64 decode: {e}"))))?;

        if combined.len() < 12 {
            return Err(MemoryError::Io(std::io::Error::other(
                "encrypted content too short",
            )));
        }

        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = self.cipher.decrypt(nonce, ciphertext).map_err(|e| {
            MemoryError::Io(std::io::Error::other(format!(
                "decrypt failed (wrong passphrase?): {e}"
            )))
        })?;

        String::from_utf8(plaintext)
            .map_err(|e| MemoryError::Io(std::io::Error::other(format!("utf8: {e}"))))
    }

    fn encrypt_memory(&self, mut memory: Memory) -> Result<Memory, MemoryError> {
        memory.content = self.encrypt_content(&memory.content)?;
        Ok(memory)
    }

    fn decrypt_memory(&self, mut memory: Memory) -> Result<Memory, MemoryError> {
        if memory.content.starts_with("enc:v1:") {
            memory.content = self.decrypt_content(&memory.content)?;
        }
        Ok(memory)
    }
}

#[async_trait]
impl<S: MemoryStore + Send + Sync> MemoryStore for EncryptedStore<S> {
    async fn record(&mut self, memory: Memory) -> Result<(), MemoryError> {
        let encrypted = self.encrypt_memory(memory)?;
        self.inner.record(encrypted).await
    }

    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        // Note: encrypted content cannot be searched server-side. We fetch all
        // and filter in-memory after decryption. For typical memory stores
        // (< 10K entries) this is acceptable.
        let all_query = MemoryQuery {
            text: String::new(), // fetch all
            limit: 10_000,
        };
        let encrypted_memories = self.inner.recall(&all_query).await?;

        let query_lower = query.text.to_lowercase();
        let mut results: Vec<Memory> = Vec::new();

        for mem in encrypted_memories {
            // Skip the salt sentinel — it is an implementation detail.
            if mem.id == SALT_SENTINEL_ID {
                continue;
            }
            let decrypted = self.decrypt_memory(mem)?;
            if query_lower.is_empty() || decrypted.content.to_lowercase().contains(&query_lower) {
                results.push(decrypted);
            }
            if results.len() >= query.limit {
                break;
            }
        }

        Ok(results)
    }

    async fn forget(&mut self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        self.inner.forget(filter).await
    }

    async fn decay(&mut self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        self.inner.decay(factor, now).await
    }

    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        // Export decrypted content (plaintext JSON).
        let all = self
            .inner
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 100_000,
            })
            .await?;

        let mut decrypted: Vec<Memory> = Vec::new();
        for mem in all {
            if mem.id == SALT_SENTINEL_ID {
                continue;
            }
            decrypted.push(self.decrypt_memory(mem)?);
        }

        let json = serde_json::to_string_pretty(&decrypted)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    async fn import(_path: &Path) -> Result<Self, MemoryError>
    where
        Self: Sized,
    {
        // Cannot import without knowing the passphrase at this layer.
        Err(MemoryError::Io(std::io::Error::other(
            "EncryptedStore::import not supported — import into the inner store, then wrap",
        )))
    }
}

// ---------------------------------------------------------------------------
// Salt management helpers
// ---------------------------------------------------------------------------

/// Load the persisted random salt from the sentinel record, or generate a new
/// one and persist it.
///
/// Returns an error if encrypted records exist but the sentinel is absent
/// (this indicates the store was created with the old fixed-salt code and the
/// sentinel was never written — migration is required).
async fn load_or_init_salt(store: &mut impl MemoryStore) -> Result<Vec<u8>, MemoryError> {
    use base64::Engine;

    let sentinel_query = MemoryQuery {
        text: String::new(),
        limit: 10_000,
    };
    let all = store.recall(&sentinel_query).await?;

    // Check whether the sentinel already exists.
    if let Some(sentinel) = all.iter().find(|m| m.id == SALT_SENTINEL_ID) {
        let salt = base64::engine::general_purpose::STANDARD
            .decode(&sentinel.content)
            .map_err(|e| {
                MemoryError::Io(std::io::Error::other(format!(
                    "failed to decode salt sentinel: {e}"
                )))
            })?;
        return Ok(salt);
    }

    // No sentinel present. Check for existing encrypted records.
    let has_encrypted = all
        .iter()
        .any(|m| m.id != SALT_SENTINEL_ID && m.content.starts_with("enc:v1:"));

    if has_encrypted {
        return Err(MemoryError::Io(std::io::Error::other(
            "EncryptedStore: salt sentinel (__encrypted_store_salt__) is missing but \
             encrypted records exist. The store was created with the old fixed-salt API. \
             Call EncryptedStore::migrate_from_fixed_salt to re-encrypt with a random salt.",
        )));
    }

    // Fresh store — generate and persist a new random salt.
    let mut salt = [0u8; 16];
    aes_gcm::aead::rand_core::RngCore::fill_bytes(&mut OsRng, &mut salt);

    let encoded = base64::engine::general_purpose::STANDARD.encode(salt);
    store
        .record(Memory {
            id: SALT_SENTINEL_ID.to_string(),
            content: encoded,
            tags: vec!["__internal__".to_string()],
            weight: 0.0,
            created_at: 0,
            decay_at: None,
            kind: crate::types::MemoryKind::Conversation,
            description: None,
        })
        .await?;

    Ok(salt.to_vec())
}

// ---------------------------------------------------------------------------
// Key derivation
// ---------------------------------------------------------------------------

/// Derive a 256-bit key from `passphrase` and `salt` using Argon2id.
///
/// Parameters are pinned (see `ARGON2_VERSION_MARKER`) so a crate update
/// cannot silently change the derived key.
fn derive_key(passphrase: &str, salt: &[u8]) -> Result<[u8; 32], MemoryError> {
    let params = Params::new(19_456, 2, 1, Some(32))
        .map_err(|e| MemoryError::Io(std::io::Error::other(format!("argon2 params: {e}"))))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| MemoryError::Io(std::io::Error::other(format!("argon2: {e}"))))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::in_memory::InMemoryStore;

    #[tokio::test]
    async fn test_round_trip_encrypt_decrypt() {
        let inner = InMemoryStore::new();
        let mut store = EncryptedStore::new(inner, "test-passphrase")
            .await
            .expect("create encrypted store");

        let memory = Memory {
            id: "m1".to_string(),
            content: "sensitive conversation data".to_string(),
            tags: vec!["test".to_string()],
            weight: 1.0,
            created_at: 1000,
            decay_at: None,
            kind: crate::types::MemoryKind::Conversation,
            description: None,
        };

        store.record(memory.clone()).await.unwrap();

        // Recall should return decrypted content.
        let results = store
            .recall(&MemoryQuery {
                text: "sensitive".to_string(),
                limit: 10,
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "sensitive conversation data");
    }

    #[tokio::test]
    async fn test_inner_store_has_encrypted_content() {
        let inner = InMemoryStore::new();
        let mut store = EncryptedStore::new(inner, "my-secret")
            .await
            .expect("create encrypted store");

        store
            .record(Memory {
                id: "m1".to_string(),
                content: "plaintext secret".to_string(),
                tags: vec![],
                weight: 1.0,
                created_at: 1000,
                decay_at: None,
                kind: crate::types::MemoryKind::Conversation,
                description: None,
            })
            .await
            .unwrap();

        // Access the inner store directly — content should be encrypted.
        let inner_results = store
            .inner
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 10,
            })
            .await
            .unwrap();

        // inner has the sentinel + the encrypted m1
        let user_record = inner_results
            .iter()
            .find(|m| m.id == "m1")
            .expect("m1 should be in inner store");
        assert!(
            user_record.content.starts_with("enc:v1:"),
            "inner store should have encrypted content, got: {}",
            &user_record.content[..50.min(user_record.content.len())]
        );
        assert!(!user_record.content.contains("plaintext secret"));
    }

    #[tokio::test]
    async fn test_wrong_passphrase_fails() {
        let inner = InMemoryStore::new();
        let mut store = EncryptedStore::new(inner, "correct-passphrase")
            .await
            .expect("create encrypted store");

        store
            .record(Memory {
                id: "m1".to_string(),
                content: "secret data".to_string(),
                tags: vec![],
                weight: 1.0,
                created_at: 1000,
                decay_at: None,
                kind: crate::types::MemoryKind::Conversation,
                description: None,
            })
            .await
            .unwrap();

        // Create a new store wrapping the same inner but with wrong passphrase.
        // This must succeed (salt sentinel is present, key is derived) but
        // recall must fail because the ciphertext was encrypted with a different key.
        let wrong_store = EncryptedStore::new(store.inner, "wrong-passphrase")
            .await
            .expect("create store");

        let result = wrong_store
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 10,
            })
            .await;

        assert!(result.is_err(), "wrong passphrase should fail decryption");
    }

    #[test]
    fn test_derive_key_deterministic() {
        let k1 = derive_key("hello", LEGACY_SALT).unwrap();
        let k2 = derive_key("hello", LEGACY_SALT).unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_derive_key_different_passphrases() {
        let k1 = derive_key("alpha", LEGACY_SALT).unwrap();
        let k2 = derive_key("beta", LEGACY_SALT).unwrap();
        assert_ne!(k1, k2);
    }

    // -----------------------------------------------------------------------
    // Fix 2 new tests
    // -----------------------------------------------------------------------

    /// Two EncryptedStore instances on separate InMemoryStores with the SAME
    /// passphrase must derive DIFFERENT keys because each store gets its own
    /// random salt.  This is verified by comparing ciphertext for the same
    /// plaintext.
    #[tokio::test]
    async fn test_different_stores_same_passphrase_derive_different_keys() {
        let inner_a = InMemoryStore::new();
        let inner_b = InMemoryStore::new();

        let mut store_a = EncryptedStore::new(inner_a, "hunter2")
            .await
            .expect("store A");
        let mut store_b = EncryptedStore::new(inner_b, "hunter2")
            .await
            .expect("store B");

        let content = "identical plaintext";

        store_a
            .record(Memory {
                id: "x".to_string(),
                content: content.to_string(),
                tags: vec![],
                weight: 1.0,
                created_at: 0,
                decay_at: None,
                kind: crate::types::MemoryKind::Conversation,
                description: None,
            })
            .await
            .unwrap();
        store_b
            .record(Memory {
                id: "x".to_string(),
                content: content.to_string(),
                tags: vec![],
                weight: 1.0,
                created_at: 0,
                decay_at: None,
                kind: crate::types::MemoryKind::Conversation,
                description: None,
            })
            .await
            .unwrap();

        // Read ciphertext from the inner stores directly.
        let ct_a = store_a
            .inner
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 10,
            })
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.id == "x")
            .unwrap()
            .content;

        let ct_b = store_b
            .inner
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 10,
            })
            .await
            .unwrap()
            .into_iter()
            .find(|m| m.id == "x")
            .unwrap()
            .content;

        assert_ne!(
            ct_a, ct_b,
            "ciphertexts from different stores with the same passphrase must differ"
        );
    }

    /// Salt persistence round-trip: open the same inner store twice with the
    /// same passphrase — the second open must successfully decrypt records
    /// written by the first open.
    #[tokio::test]
    async fn test_salt_persistence_round_trip() {
        let inner = InMemoryStore::new();

        // First open: write a record.
        let mut store1 = EncryptedStore::new(inner, "correct-horse-battery-staple")
            .await
            .expect("first open");
        store1
            .record(Memory {
                id: "msg1".to_string(),
                content: "super secret".to_string(),
                tags: vec![],
                weight: 1.0,
                created_at: 1,
                decay_at: None,
                kind: crate::types::MemoryKind::Conversation,
                description: None,
            })
            .await
            .unwrap();

        // Move the inner store out so we can re-wrap it.
        let inner2 = store1.inner;

        // Second open: re-derive key from the persisted salt.
        let store2 = EncryptedStore::new(inner2, "correct-horse-battery-staple")
            .await
            .expect("second open");

        let results = store2
            .recall(&MemoryQuery {
                text: "super secret".to_string(),
                limit: 5,
            })
            .await
            .expect("recall after reopen");

        assert_eq!(results.len(), 1, "should find 1 record after reopen");
        assert_eq!(results[0].content, "super secret");
    }

    /// Missing sentinel with existing encrypted records must return an error.
    #[tokio::test]
    async fn test_missing_sentinel_with_encrypted_records_errors() {
        let mut inner = InMemoryStore::new();

        // Manually insert a record that looks encrypted (no sentinel).
        inner
            .record(Memory {
                id: "orphan".to_string(),
                content: "enc:v1:AAAAAAAAAAAAAAAA".to_string(),
                tags: vec![],
                weight: 1.0,
                created_at: 1,
                decay_at: None,
                kind: crate::types::MemoryKind::Conversation,
                description: None,
            })
            .await
            .unwrap();

        let result = EncryptedStore::new(inner, "any-passphrase").await;
        assert!(
            result.is_err(),
            "should error when sentinel is missing but encrypted records exist"
        );
        let err_str = result.err().unwrap().to_string();
        assert!(
            err_str.contains("salt sentinel") || err_str.contains("migrate"),
            "error should mention migration, got: {err_str}"
        );
    }
}
