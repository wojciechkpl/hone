# Encrypted memory store: the salt can't be forgotten, descriptions are encrypted: spec

Branch `fix/encrypted-store-safety`, off `main`. Found while reviewing PR G (FTS5 recall); both bugs are on `main`.

## Problem

1. **`hone memory forget` can destroy an encrypted store.** `EncryptedStore` keeps its Argon2 salt in a sentinel memory (`__encrypted_store_salt__`, kind `conversation`, `created_at` 0). `hone memory …` opens the database as a plain `SqliteStore`, and `forget(Before(…))` or `forget(Kind(Conversation))` deletes the sentinel with everything else. With the salt gone, every encrypted memory is permanently unreadable. Reproduced in review: after a plain `forget(Before(2020))`, the next encrypted open failed with "salt sentinel is missing".
2. **Descriptions are stored in plaintext.** `EncryptedStore` encrypts `content` but writes `description` as given, and the trigger-phrase writer (`memory_worthy.rs`) puts the first 80 characters of the user's message there. Reproduced: content `enc:v1:…`, description `"feedback rule: my bank PIN is 4321"`.

## Rows

| Row | Behaviour |
|---|---|
| ES1 | **The salt sentinel survives every forget.** `SqliteStore::forget` never deletes the row with id `__encrypted_store_salt__`, whatever the filter (`Before`, `After`, `Kind`, `Tag`, `Id`, all). The decay pass's expiry never deletes it either. The returned count doesn't include it. A plain `hone memory forget --before <date>` / `--kind conversation` on an encrypted database leaves it openable with the right passphrase and every memory readable. |
| ES2 | **New descriptions are encrypted.** `EncryptedStore` encrypts a memory's `description` (as `enc:v1:`) when storing it and decrypts it when reading; a plaintext description (written before this change) is returned as is. Recall, listing, export and `find_similar` return decrypted descriptions. Existing plaintext descriptions are not rewritten by this change (re-encrypting them safely needs a verified passphrase at open, which PR G adds); `docs/configuration.md` says so. |
| ES3 | **Docs.** `docs/configuration.md` says which fields `--encrypted-memory` encrypts (content and, for new memories, description) and which it doesn't (tags, kind, timestamps, weight), and warns against `hone memory forget` on encrypted stores from older versions. CHANGELOG entry. |

## Testing notes

- **ES1** on a temp `SqliteStore` with a sentinel row plus ordinary rows, for each filter; and end to end: an `EncryptedStore` with memories, a plain-store `forget(Before(now))`, then reopen encrypted and read.
- **ES2** with `EncryptedStore` over a temp `SqliteStore`: the raw row's description starts with `enc:v1:`; reads return the plaintext; a hand-inserted plaintext description reads back as is.

## Out of scope

- Re-encrypting existing descriptions (PR G).
- Encrypting tags.

## Review round 1 (ES4–ES7)

Review found the same failure through another pre-existing path: `load_or_init_salt` finds the sentinel by listing the newest 10,000 memories, and the sentinel's `created_at` is 0. With 10,000 newer rows (plain runs on the same database, or a large encrypted store) it isn't listed, so a new salt is generated and written over the old one (`INSERT OR REPLACE`) — every encrypted memory permanently unreadable; with 10,001 encrypted memories the store refuses to open with a misleading error.

| Row | Behaviour |
|---|---|
| ES4 | **The salt is found by id and never overwritten.** `EncryptedStore` reads the sentinel with `recall_by_id(SALT_SENTINEL_ID)`; whether encrypted rows exist is decided by a direct query (any `content LIKE 'enc:v1:%'`), not a listing; a missing sentinel is created insert-if-absent (never replacing an existing one). A store with 10,000+ newer plaintext rows keeps its salt and reads its encrypted memories; a store with 10,001+ encrypted memories opens. |
| ES5 | **A description that fails to decrypt isn't shown as ciphertext.** It's returned as `None` and a warning is logged (content that fails to decrypt still errors, as today). |
| ES6 | **Every inner store protects the sentinel.** `EncryptedStore::forget` never passes a filter that would remove the sentinel to its inner store (works for `InMemoryStore` too). `hone memory ls` and `show` hide the sentinel row. |
| ES7 | **Docs.** `docs/configuration.md` says that running Hone without `--encrypted-memory` on an encrypted `memory.db` writes plaintext memories into it, and that `hone memory ls/show` don't decrypt (they show `enc:v1:…`). CHANGELOG: "the salt record is found by id, so stores with more than 10,000 memories open and a new salt is never written over an existing one." |
