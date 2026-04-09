//! Application-layer encryption for memory content.
//!
//! Wraps any [`MemoryStore`] to encrypt `Memory.content` before writing and
//! decrypt after reading. Uses AES-256-GCM with a key derived from a
//! passphrase via Argon2id.
//!
//! This avoids the complexity of linking sqlcipher while providing at-rest
//! encryption of the most sensitive field (conversation content).

use std::path::Path;

use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::Argon2;
use async_trait::async_trait;

use crate::error::MemoryError;
use crate::store::MemoryStore;
use crate::types::{Memory, MemoryFilter, MemoryQuery};

/// Fixed salt for key derivation. In production this should be per-user,
/// but for a local CLI tool a fixed salt is acceptable — the passphrase
/// provides the entropy.
const SALT: &[u8] = b"hone-memory-v1-salt";

/// Wraps an inner [`MemoryStore`] with AES-256-GCM content encryption.
pub struct EncryptedStore<S: MemoryStore> {
    inner: S,
    cipher: Aes256Gcm,
}

impl<S: MemoryStore> EncryptedStore<S> {
    /// Create an encrypted wrapper around `inner`, deriving the key from
    /// `passphrase` using Argon2id.
    pub fn new(inner: S, passphrase: &str) -> Result<Self, MemoryError> {
        let key = derive_key(passphrase)?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| {
            MemoryError::Io(std::io::Error::other(format!("cipher init: {e}")))
        })?;
        Ok(Self { inner, cipher })
    }

    fn encrypt_content(&self, plaintext: &str) -> Result<String, MemoryError> {
        // Generate a random 96-bit nonce for each encryption.
        let nonce_bytes: [u8; 12] = aes_gcm::aead::rand_core::RngCore::next_u64(&mut OsRng)
            .to_le_bytes()
            .iter()
            .chain(
                aes_gcm::aead::rand_core::RngCore::next_u64(&mut OsRng)
                    .to_le_bytes()
                    .iter(),
            )
            .take(12)
            .copied()
            .collect::<Vec<u8>>()
            .try_into()
            .unwrap();

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
        let encoded = stored
            .strip_prefix("enc:v1:")
            .ok_or_else(|| {
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

        let plaintext = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| {
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

/// Derive a 256-bit key from a passphrase using Argon2id.
fn derive_key(passphrase: &str) -> Result<[u8; 32], MemoryError> {
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), SALT, &mut key)
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
        let mut store =
            EncryptedStore::new(inner, "test-passphrase").expect("create encrypted store");

        let memory = Memory {
            id: "m1".to_string(),
            content: "sensitive conversation data".to_string(),
            tags: vec!["test".to_string()],
            weight: 1.0,
            created_at: 1000,
            decay_at: None,
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
        let mut store =
            EncryptedStore::new(inner, "my-secret").expect("create encrypted store");

        store
            .record(Memory {
                id: "m1".to_string(),
                content: "plaintext secret".to_string(),
                tags: vec![],
                weight: 1.0,
                created_at: 1000,
                decay_at: None,
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

        assert_eq!(inner_results.len(), 1);
        assert!(
            inner_results[0].content.starts_with("enc:v1:"),
            "inner store should have encrypted content, got: {}",
            &inner_results[0].content[..50.min(inner_results[0].content.len())]
        );
        assert!(!inner_results[0].content.contains("plaintext secret"));
    }

    #[tokio::test]
    async fn test_wrong_passphrase_fails() {
        let inner = InMemoryStore::new();
        let mut store =
            EncryptedStore::new(inner, "correct-passphrase").expect("create encrypted store");

        store
            .record(Memory {
                id: "m1".to_string(),
                content: "secret data".to_string(),
                tags: vec![],
                weight: 1.0,
                created_at: 1000,
                decay_at: None,
            })
            .await
            .unwrap();

        // Create a new store wrapping the same inner but with wrong passphrase.
        let wrong_store =
            EncryptedStore::new(store.inner, "wrong-passphrase").expect("create store");

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
        let k1 = derive_key("hello").unwrap();
        let k2 = derive_key("hello").unwrap();
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_derive_key_different_passphrases() {
        let k1 = derive_key("alpha").unwrap();
        let k2 = derive_key("beta").unwrap();
        assert_ne!(k1, k2);
    }
}
