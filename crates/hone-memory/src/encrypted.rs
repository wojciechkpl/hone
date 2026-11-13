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

use aes_gcm::aead::{Aead, OsRng, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use async_trait::async_trait;

use crate::error::MemoryError;
use crate::store::{MemoryStore, SealedVectorStore};
use crate::types::{Memory, MemoryFilter, MemoryKind, MemoryQuery};

/// The old fixed salt.  Kept as `LEGACY_SALT` so that any tests or tooling
/// that were built against the old API have a named constant to reference.
///
/// TODO: migrate_from_fixed_salt should re-derive the key from LEGACY_SALT,
/// decrypt all records, generate a new random salt, and re-encrypt.
pub const LEGACY_SALT: &[u8] = b"hone-memory-v1-salt";

/// The 8-byte magic of sealed vectors in the `embedding` column, as earlier
/// builds of this branch wrote them (now only read, to migrate them).
const OLD_EMBEDDING_MAGIC: [u8; 8] = [0xE1, 0x48, 0x4F, 0x4E, 0x45, 0x56, 0x31, 0x00];
/// The 1-byte marker of still earlier builds (also only read).
const OLD_EMBEDDING_MARKER: u8 = 0xE1;
/// What the key-check value (kept in the salt sentinel's description) holds.
const KEY_CHECK_PLAIN: &str = "hone-key-check";
const KEY_CHECK_PREFIX: &str = "keycheck:";
/// How many encrypted rows are tried to verify a passphrase when the store has
/// no key-check value yet.
const TRIAL_ROWS: usize = 5;

/// Reserved memory ID used to persist the per-store random salt. `SqliteStore`
/// never forgets or expires it: without it every encrypted memory is unreadable.
pub const SALT_SENTINEL_ID: &str = "__encrypted_store_salt__";

/// Argon2 version marker.  If the KDF parameters are ever changed (e.g., to
/// increase memory cost), bump this string and add detection logic in
/// `decrypt_content` based on the `enc:vN:` prefix so existing records can
/// still be decrypted.
///
/// TODO: for v2 params introduce `enc:v2:` prefix and detect in decrypt.
#[allow(dead_code)]
const ARGON2_VERSION_MARKER: &str = "argon2id/v0x13/m=19456/t=2/p=1";

/// What [`EncryptedStore::new`] did beyond opening: for the caller to tell the
/// user, whatever the log level.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpenReport {
    /// There was no encrypted memory to check the passphrase against, so the
    /// passphrase used now was pinned as this database's.
    pub passphrase_pinned: bool,
    /// Vectors an older Hone left in the `embedding` column, now sealed in the
    /// side table.
    pub vectors_migrated: usize,
    /// Vectors left in place because they are neither a plain vector nor a blob
    /// of this key; they read as no embedding.
    pub vectors_skipped: usize,
}

impl OpenReport {
    /// Whether there is anything to tell the user.
    #[must_use]
    pub fn has_news(&self) -> bool {
        self.passphrase_pinned || self.vectors_migrated > 0 || self.vectors_skipped > 0
    }
}

impl std::fmt::Display for OpenReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.passphrase_pinned {
            parts.push(
                "there were no encrypted memories to check the passphrase against, so this \
                 passphrase is now the one for this database"
                    .to_owned(),
            );
        }
        if self.vectors_migrated > 0 {
            parts.push(format!(
                "{} embedding(s) moved into the encrypted side table",
                self.vectors_migrated
            ));
        }
        if self.vectors_skipped > 0 {
            parts.push(format!(
                "{} embedding(s) skipped (not readable with this passphrase)",
                self.vectors_skipped
            ));
        }
        write!(f, "{}", parts.join("; "))
    }
}

/// Wraps an inner [`MemoryStore`] with AES-256-GCM content encryption. The inner
/// store must also be a [`SealedVectorStore`], which keeps the sealed vectors.
pub struct EncryptedStore<S: MemoryStore + SealedVectorStore> {
    pub(crate) inner: S,
    cipher: Aes256Gcm,
    report: OpenReport,
}

/// How many of the newest memories a ranked recall decrypts.
const RANKED_CORPUS: usize = 5_000;

impl<S: MemoryStore + SealedVectorStore> EncryptedStore<S> {
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
    pub async fn new(inner: S, passphrase: &str) -> Result<Self, MemoryError> {
        // The salt is created insert-if-absent and read back before any key is
        // derived, so two first opens of one database agree on it.
        let sentinel = ensure_sentinel(&inner).await?;
        let salt = decode_salt(&sentinel)?;
        let key = derive_key(passphrase, &salt)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| MemoryError::Io(std::io::Error::other(format!("cipher init: {e}"))))?;
        let mut store = Self {
            inner,
            cipher,
            report: OpenReport::default(),
        };
        // No memory, vector or key check is changed before the passphrase is
        // known to be right: a wrong key would otherwise "migrate" plaintext
        // vectors into unreadable blobs.
        store.report.passphrase_pinned = store.verify_passphrase(sentinel).await?;
        let (migrated, skipped) = store.migrate_vectors().await?;
        store.report.vectors_migrated = migrated;
        store.report.vectors_skipped = skipped;
        Ok(store)
    }

    /// What opening this store did (a pinned passphrase, migrated or skipped
    /// vectors), for the caller to show.
    #[must_use]
    pub fn open_report(&self) -> &OpenReport {
        &self.report
    }

    /// The error for a passphrase that does not match the store.
    fn wrong_passphrase() -> MemoryError {
        MemoryError::Io(std::io::Error::other(
            "EncryptedStore: wrong passphrase (the key check failed); no memory, vector or key check was changed",
        ))
    }

    /// Whether `sentinel`'s key-check value (if it has one) opens with this key.
    /// `None` when it has no key check.
    fn key_check_ok(&self, sentinel: &Memory) -> Option<bool> {
        let check = sentinel
            .description
            .as_deref()?
            .strip_prefix(KEY_CHECK_PREFIX)?;
        Some(
            self.decrypt_content(check)
                .is_ok_and(|plain| plain == KEY_CHECK_PLAIN),
        )
    }

    /// Check the passphrase against the key-check value kept with the salt
    /// sentinel. A store without one (made by `main`) is checked by
    /// trial-decrypting up to [`TRIAL_ROWS`] encrypted rows, chosen by a direct
    /// query whatever their age (any success accepts); a store with no
    /// encrypted rows pins the first passphrase used (the result is `true`).
    /// The key check is then written insert-if-absent and read back, so a
    /// concurrent open with another passphrase can't both win.
    async fn verify_passphrase(&self, sentinel: Memory) -> Result<bool, MemoryError> {
        if let Some(ok) = self.key_check_ok(&sentinel) {
            return if ok {
                Ok(false)
            } else {
                Err(Self::wrong_passphrase())
            };
        }
        let sample = self.inner.encrypted_rows_sample(TRIAL_ROWS).await?;
        let pinned = sample.is_empty();
        if pinned {
            tracing::warn!(
                "encrypted memory has no memories to check the passphrase against: \
                 the passphrase used now is pinned for this database"
            );
        } else if !sample
            .iter()
            .any(|m| self.decrypt_content(&m.content).is_ok())
        {
            return Err(Self::wrong_passphrase());
        }
        let check = self.encrypt_content(KEY_CHECK_PLAIN)?;
        self.inner
            .set_description_if_absent(SALT_SENTINEL_ID, &format!("{KEY_CHECK_PREFIX}{check}"))
            .await?;
        // Whoever wrote the check first decides: it must be ours.
        let stored = self
            .inner
            .recall_by_id(SALT_SENTINEL_ID)
            .await?
            .ok_or_else(Self::wrong_passphrase)?;
        match self.key_check_ok(&stored) {
            Some(true) => Ok(pinned),
            _ => Err(Self::wrong_passphrase()),
        }
    }

    /// The associated data a sealed vector is bound to: the memory id and the
    /// row's content ciphertext, so a vector can't be read for another id or for
    /// a row that has since been replaced outside this store.
    fn vector_aad(id: &str, content_ciphertext: &str) -> Vec<u8> {
        let mut aad = Vec::with_capacity(id.len() + 1 + content_ciphertext.len());
        aad.extend_from_slice(id.as_bytes());
        aad.push(0);
        aad.extend_from_slice(content_ciphertext.as_bytes());
        aad
    }

    /// An embedding as the blob kept in the side table: the 12-byte nonce and
    /// the AES-GCM ciphertext of the little-endian f32s, bound to the row it
    /// belongs to (see [`vector_aad`](Self::vector_aad)).
    fn seal_embedding(
        &self,
        embedding: &[f32],
        id: &str,
        content_ciphertext: &str,
    ) -> Result<Vec<u8>, MemoryError> {
        let mut nonce_bytes = [0u8; 12];
        aes_gcm::aead::rand_core::RngCore::fill_bytes(&mut OsRng, &mut nonce_bytes);
        let plain: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
        let aad = Self::vector_aad(id, content_ciphertext);
        let ct = self
            .cipher
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: plain.as_slice(),
                    aad: &aad,
                },
            )
            .map_err(|e| MemoryError::Io(std::io::Error::other(format!("encrypt: {e}"))))?;
        let mut blob = Vec::with_capacity(12 + ct.len());
        blob.extend_from_slice(&nonce_bytes);
        blob.extend_from_slice(&ct);
        Ok(blob)
    }

    /// The vector in a side-table blob, or `None` when it does not open with
    /// this store's key for this row.
    fn open_sealed(&self, blob: &[u8], id: &str, content_ciphertext: &str) -> Option<Vec<f32>> {
        if blob.len() < 12 + 16 {
            return None;
        }
        let aad = Self::vector_aad(id, content_ciphertext);
        let plain = self
            .cipher
            .decrypt(
                Nonce::from_slice(&blob[..12]),
                Payload {
                    msg: &blob[12..],
                    aad: &aad,
                },
            )
            .ok()?;
        Some(le_f32s(&plain))
    }

    /// What an `embedding` column value of an `enc:v1` row is.
    fn classify_column(&self, bytes: &[u8]) -> ColumnVector {
        if bytes.len() >= 8 + 12 + 16 && bytes[..8] == OLD_EMBEDDING_MAGIC {
            // An earlier build's sealed blob: decrypted, or left alone. Never
            // taken for a plaintext vector.
            return match self
                .cipher
                .decrypt(Nonce::from_slice(&bytes[8..20]), &bytes[20..])
            {
                Ok(plain) => ColumnVector::Vector(le_f32s(&plain)),
                Err(_) => ColumnVector::Undecryptable,
            };
        }
        if bytes.first() == Some(&OLD_EMBEDDING_MARKER) && bytes.len() >= 32 && bytes.len() % 4 == 0
        {
            // Earlier build: marker, nonce, ciphertext, 3 zero bytes of padding.
            let end = bytes.len() - 3;
            let (nonce, ct) = bytes[1..end].split_at(12);
            if let Ok(plain) = self.cipher.decrypt(Nonce::from_slice(nonce), ct) {
                return ColumnVector::Vector(le_f32s(&plain));
            }
            // Not a sealed blob of this key: a plaintext vector that happens to
            // start with 0xE1.
        }
        if bytes.len() % 4 != 0 {
            return ColumnVector::Undecryptable;
        }
        ColumnVector::Vector(le_f32s(bytes))
    }

    /// Rows an older binary wrote (`enc:v1` content, a vector in the `embedding`
    /// column: plaintext, or a blob of an earlier build) are moved into the side
    /// table, once: sealed (bound to the row's content ciphertext), the column
    /// set to NULL. Only those rows are selected (in SQL), a batch of about 500 at
    /// a time and one transaction per batch, so an open with nothing to migrate
    /// reads no embeddings and a large one never holds every candidate. A blob
    /// that does not decrypt is left in place and skipped. Returns how many were
    /// moved and how many skipped.
    async fn migrate_vectors(&self) -> Result<(usize, usize), MemoryError> {
        let mut migrated = 0usize;
        let mut skipped: std::collections::HashSet<String> = std::collections::HashSet::new();
        loop {
            let batch = self.inner.vector_migration_candidates().await?;
            if batch.is_empty() {
                break;
            }
            let mut moves: Vec<(String, Vec<u8>)> = Vec::with_capacity(batch.len());
            for (id, bytes) in batch {
                let sealed = match self.classify_column(&bytes) {
                    ColumnVector::Vector(v) => match self.inner.recall_by_id(&id).await? {
                        Some(row) => Some(self.seal_embedding(&v, &id, &row.content)?),
                        None => None,
                    },
                    ColumnVector::Undecryptable => None,
                };
                match sealed {
                    Some(blob) => moves.push((id, blob)),
                    None => {
                        if skipped.insert(id.clone()) {
                            tracing::warn!(
                                memory = %id,
                                "skipping an embedding that is neither a plain vector nor a blob of this key"
                            );
                        }
                    }
                }
            }
            if moves.is_empty() {
                break;
            }
            migrated += moves.len();
            self.inner.move_vectors_to_sealed(&moves).await?;
        }
        Ok((migrated, skipped.len()))
    }

    /// Give each of `memories` its decrypted embedding, read from the side table
    /// (the column is NULL for encrypted rows). `ciphertexts` maps a memory's id
    /// to the content ciphertext of its row, which the vector is bound to.
    async fn attach_embeddings(
        &self,
        memories: &mut [Memory],
        ciphertexts: &std::collections::HashMap<String, String>,
    ) -> Result<(), MemoryError> {
        for m in memories.iter_mut().filter(|m| m.embedding.is_none()) {
            let Some(ct) = ciphertexts.get(&m.id) else {
                continue;
            };
            if let Some(blob) = self.inner.sealed_vector(&m.id).await? {
                m.embedding = self.open_sealed(&blob, &m.id, ct);
            }
        }
        Ok(())
    }

    fn encrypt_content(&self, plaintext: &str) -> Result<String, MemoryError> {
        self.encrypt_with("enc:v1:", plaintext)
    }

    fn encrypt_with(&self, prefix: &str, plaintext: &str) -> Result<String, MemoryError> {
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
        Ok(format!("{prefix}{encoded}"))
    }

    fn decrypt_content(&self, stored: &str) -> Result<String, MemoryError> {
        let encoded = stored
            .strip_prefix("enc:v1:")
            .or_else(|| stored.strip_prefix("enc:v2:"))
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

        let plaintext = self.cipher.decrypt(nonce, ciphertext).map_err(|e| {
            MemoryError::Io(std::io::Error::other(format!(
                "decrypt failed (wrong passphrase?): {e}"
            )))
        })?;

        String::from_utf8(plaintext)
            .map_err(|e| MemoryError::Io(std::io::Error::other(format!("utf8: {e}"))))
    }

    /// Decrypt content and description; a plaintext description (written before
    /// descriptions were encrypted) is returned as it is. An `enc:v2:` row (written
    /// by earlier builds) carries its embedding inside the ciphertext.
    /// Also returns the row's content ciphertext (what its sealed vector is bound to).
    fn decrypt_row(&self, mut memory: Memory) -> Result<(Memory, String), MemoryError> {
        let ciphertext = memory.content.clone();
        if memory.content.starts_with("enc:v2:") {
            let plain = self.decrypt_content(&memory.content)?;
            let payload: serde_json::Value = serde_json::from_str(&plain)?;
            memory.content = payload["content"].as_str().unwrap_or_default().to_owned();
            memory.embedding = serde_json::from_value(payload["embedding"].clone()).ok();
        } else if memory.content.starts_with("enc:v1:") {
            memory.content = self.decrypt_content(&memory.content)?;
            // The vector is in the side table (attached by id); whatever the
            // column holds is not a vector this store can vouch for.
            memory.embedding = None;
        }
        if let Some(d) = memory
            .description
            .as_deref()
            .filter(|d| d.starts_with("enc:v1:"))
        {
            // A description that doesn't decrypt is never shown as ciphertext.
            match self.decrypt_content(d) {
                Ok(plain) => memory.description = Some(plain),
                Err(e) => {
                    tracing::warn!(id = %memory.id, "description could not be decrypted: {e}");
                    memory.description = None;
                }
            }
        }
        Ok((memory, ciphertext))
    }

    /// `recall` / `recall_excluding`: fetch every memory of the wanted kinds
    /// (the inner store applies the kind filter and the exclusions), decrypt,
    /// then filter on the text.
    async fn recall_filtered(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
        project: Option<&str>,
    ) -> Result<Vec<Memory>, MemoryError> {
        // Encrypted content cannot be searched server-side: the inner store
        // applies the kind filter and the exclusions, then the newest
        // `RANKED_CORPUS` memories are decrypted and ranked in Rust.
        if query.text.is_empty() {
            let all_query = MemoryQuery {
                text: String::new(),
                limit: 10_000,
                kind: query.kind,
            };
            let encrypted_memories = self
                .inner
                .recall_visible(&all_query, exclude, project)
                .await?;
            let mut results: Vec<Memory> = Vec::new();
            let mut ciphertexts = std::collections::HashMap::new();
            for mem in encrypted_memories {
                // Skip the salt sentinel — it is an implementation detail.
                if mem.id == SALT_SENTINEL_ID {
                    continue;
                }
                let (plain, raw) = self.decrypt_row(mem)?;
                ciphertexts.insert(plain.id.clone(), raw);
                results.push(plain);
                if results.len() >= query.limit {
                    break;
                }
            }
            self.attach_embeddings(&mut results, &ciphertexts).await?;
            return Ok(results);
        }

        // One extra row: the salt sentinel may be among the newest.
        let corpus_query = MemoryQuery {
            text: String::new(),
            limit: RANKED_CORPUS + 1,
            kind: query.kind,
        };
        let encrypted_memories = self
            .inner
            .recall_visible(&corpus_query, exclude, project)
            .await?;
        let mut corpus: Vec<Memory> = Vec::with_capacity(encrypted_memories.len());
        let mut ciphertexts = std::collections::HashMap::new();
        for mem in encrypted_memories {
            if mem.id == SALT_SENTINEL_ID {
                continue;
            }
            if corpus.len() >= RANKED_CORPUS {
                break;
            }
            let (plain, raw) = self.decrypt_row(mem)?;
            ciphertexts.insert(plain.id.clone(), raw);
            corpus.push(plain);
        }
        let mut results = crate::sqlite_store::bm25_top(corpus, &query.text, query.limit);
        // Vectors are decrypted only for the memories returned.
        self.attach_embeddings(&mut results, &ciphertexts).await?;
        Ok(results)
    }
}

#[async_trait]
impl<S: MemoryStore + SealedVectorStore> MemoryStore for EncryptedStore<S> {
    /// Content is `enc:v1:` (readable by older binaries); the embedding goes,
    /// sealed and bound to the row, into the side table, and the `embedding`
    /// column stays NULL, so an older binary sees no embedding. The row and its
    /// sealed vector are written in one transaction, so nothing fails after the
    /// row was written.
    async fn record(&self, mut memory: Memory) -> Result<(), MemoryError> {
        let embedding = memory.embedding.take();
        memory.content = self.encrypt_content(&memory.content)?;
        // The description (the start of the user's message, for some memories)
        // is encrypted too.
        memory.description = memory
            .description
            .take()
            .map(|d| self.encrypt_content(&d))
            .transpose()?;
        let sealed = embedding
            .as_deref()
            .map(|e| self.seal_embedding(e, &memory.id, &memory.content))
            .transpose()?;
        // Replacing a memory replaces (or removes) its sealed vector too.
        self.inner.record_sealed(memory, sealed).await
    }

    async fn recall_by_id(&self, id: &str) -> Result<Option<Memory>, MemoryError> {
        let Some(found) = self.inner.recall_by_id(id).await? else {
            return Ok(None);
        };
        let (mut memory, raw) = self.decrypt_row(found)?;
        if memory.embedding.is_none() {
            let ciphertexts = std::collections::HashMap::from([(memory.id.clone(), raw)]);
            self.attach_embeddings(std::slice::from_mut(&mut memory), &ciphertexts)
                .await?;
        }
        Ok(Some(memory))
    }

    async fn recall(&self, query: &MemoryQuery) -> Result<Vec<Memory>, MemoryError> {
        self.recall_filtered(query, &[], None).await
    }

    async fn recall_excluding(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
    ) -> Result<Vec<Memory>, MemoryError> {
        self.recall_filtered(query, exclude, None).await
    }

    async fn recall_visible(
        &self,
        query: &MemoryQuery,
        exclude: &[MemoryKind],
        project: Option<&str>,
    ) -> Result<Vec<Memory>, MemoryError> {
        self.recall_filtered(query, exclude, project).await
    }

    async fn recall_tool_memories(
        &self,
        kind: MemoryKind,
        project: &str,
        limit: usize,
    ) -> Result<Vec<Memory>, MemoryError> {
        let found = self
            .inner
            .recall_tool_memories(kind, project, limit)
            .await?;
        let mut memories: Vec<Memory> = Vec::with_capacity(found.len());
        let mut ciphertexts = std::collections::HashMap::new();
        for m in found {
            if m.id == SALT_SENTINEL_ID {
                continue;
            }
            let (plain, raw) = self.decrypt_row(m)?;
            ciphertexts.insert(plain.id.clone(), raw);
            memories.push(plain);
        }
        self.attach_embeddings(&mut memories, &ciphertexts).await?;
        Ok(memories)
    }

    async fn recall_semantic(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<crate::embedding::ScoredMemory>, MemoryError> {
        // Candidates: the newest memories; their vectors are decrypted here.
        let mut candidates = self
            .recall_filtered(
                &MemoryQuery {
                    text: String::new(),
                    limit: 1024,
                    kind: None,
                },
                &[],
                None,
            )
            .await?;
        candidates.retain(|m| m.embedding.is_some());
        let mut scored: Vec<crate::embedding::ScoredMemory> = candidates
            .into_iter()
            .filter_map(|m| {
                let score =
                    crate::embedding::cosine_similarity(query_embedding, m.embedding.as_deref()?);
                Some(crate::embedding::ScoredMemory { memory: m, score })
            })
            .collect();
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);
        Ok(scored)
    }

    async fn forget(&self, filter: &MemoryFilter) -> Result<u32, MemoryError> {
        self.inner.forget(filter).await
    }

    async fn decay(&self, factor: f64, now: u64) -> Result<u32, MemoryError> {
        self.inner.decay(factor, now).await
    }

    async fn export(&self, path: &Path) -> Result<(), MemoryError> {
        // Export decrypted content (plaintext JSON).
        let all = self
            .inner
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 100_000,
                kind: None,
            })
            .await?;

        let mut decrypted: Vec<Memory> = Vec::new();
        let mut ciphertexts = std::collections::HashMap::new();
        for mem in all {
            if mem.id == SALT_SENTINEL_ID {
                continue;
            }
            let (plain, raw) = self.decrypt_row(mem)?;
            ciphertexts.insert(plain.id.clone(), raw);
            decrypted.push(plain);
        }
        self.attach_embeddings(&mut decrypted, &ciphertexts).await?;

        let json = serde_json::to_string_pretty(&decrypted)?;
        tokio::fs::write(path, json).await?;
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

/// The salt sentinel: read it, or create it insert-if-absent and read it back
/// (so concurrent first opens all end up with the one stored salt). Errors if
/// encrypted records exist but the sentinel is absent (the store was created
/// with the old fixed-salt code: migration is required).
async fn ensure_sentinel(
    store: &(impl MemoryStore + SealedVectorStore),
) -> Result<Memory, MemoryError> {
    use base64::Engine;

    // The sentinel is found by id (a listing of the newest memories can miss it).
    if let Some(sentinel) = store.recall_by_id(SALT_SENTINEL_ID).await? {
        return Ok(sentinel);
    }

    // No sentinel present. Check for existing encrypted records (a direct query).
    if store.has_encrypted_rows().await? {
        return Err(MemoryError::Io(std::io::Error::other(
            "EncryptedStore: salt sentinel (__encrypted_store_salt__) is missing but \
             encrypted records exist. The store was created with the old fixed-salt API. \
             Call EncryptedStore::migrate_from_fixed_salt to re-encrypt with a random salt.",
        )));
    }

    let mut salt = [0u8; 16];
    aes_gcm::aead::rand_core::RngCore::fill_bytes(&mut OsRng, &mut salt);
    store
        .record_if_absent(Memory {
            id: SALT_SENTINEL_ID.to_string(),
            content: base64::engine::general_purpose::STANDARD.encode(salt),
            tags: vec!["__internal__".to_string()],
            weight: 0.0,
            created_at: 0,
            decay_at: None,
            kind: crate::types::MemoryKind::Conversation,
            description: None,
            embedding: None,
        })
        .await?;
    store
        .recall_by_id(SALT_SENTINEL_ID)
        .await?
        .ok_or_else(|| MemoryError::Io(std::io::Error::other("salt sentinel was not stored")))
}

fn decode_salt(sentinel: &Memory) -> Result<Vec<u8>, MemoryError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(&sentinel.content)
        .map_err(|e| {
            MemoryError::Io(std::io::Error::other(format!(
                "failed to decode salt sentinel: {e}"
            )))
        })
}

/// What an `embedding` column value of an `enc:v1` row turned out to be.
enum ColumnVector {
    /// A vector (plaintext, or decrypted from an earlier build's blob).
    Vector(Vec<f32>),
    /// Not usable: a sealed-looking blob that does not decrypt, or bytes that
    /// are not whole f32s.
    Undecryptable,
}

fn le_f32s(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
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
        let store = EncryptedStore::new(inner, "test-passphrase")
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
            embedding: None,
        };

        store.record(memory.clone()).await.unwrap();

        // Recall should return decrypted content.
        let results = store
            .recall(&MemoryQuery {
                text: "sensitive".to_string(),
                limit: 10,
                kind: None,
            })
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].content, "sensitive conversation data");
    }

    #[tokio::test]
    async fn test_inner_store_has_encrypted_content() {
        let inner = InMemoryStore::new();
        let store = EncryptedStore::new(inner, "my-secret")
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
                embedding: None,
            })
            .await
            .unwrap();

        // Access the inner store directly — content should be encrypted.
        let inner_results = store
            .inner
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 10,
                kind: None,
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
        // G25: a wrong passphrase fails at open and changes nothing; the right one still
        // recalls the data.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.db");
        {
            let inner = crate::SqliteStore::open_without_fts(&path).await.unwrap();
            let store = EncryptedStore::new(inner, "correct-passphrase")
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
                    embedding: None,
                })
                .await
                .unwrap();
        }

        let inner = crate::SqliteStore::open_without_fts(&path).await.unwrap();
        let err = EncryptedStore::new(inner, "wrong-passphrase")
            .await
            .err()
            .expect("a wrong passphrase must fail at open");
        assert!(
            err.to_string().to_lowercase().contains("passphrase"),
            "the error says why: {err}"
        );

        let inner = crate::SqliteStore::open_without_fts(&path).await.unwrap();
        let right = EncryptedStore::new(inner, "correct-passphrase")
            .await
            .expect("the right passphrase still opens it");
        let got = right
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 10,
                kind: None,
            })
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].content, "secret data");
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

        let store_a = EncryptedStore::new(inner_a, "hunter2")
            .await
            .expect("store A");
        let store_b = EncryptedStore::new(inner_b, "hunter2")
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
                embedding: None,
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
                embedding: None,
            })
            .await
            .unwrap();

        // Read ciphertext from the inner stores directly.
        let ct_a = store_a
            .inner
            .recall(&MemoryQuery {
                text: String::new(),
                limit: 10,
                kind: None,
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
                kind: None,
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
        let store1 = EncryptedStore::new(inner, "correct-horse-battery-staple")
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
                embedding: None,
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
                kind: None,
            })
            .await
            .expect("recall after reopen");

        assert_eq!(results.len(), 1, "should find 1 record after reopen");
        assert_eq!(results[0].content, "super secret");
    }

    /// Missing sentinel with existing encrypted records must return an error.
    #[tokio::test]
    async fn test_missing_sentinel_with_encrypted_records_errors() {
        let inner = InMemoryStore::new();

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
                embedding: None,
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
