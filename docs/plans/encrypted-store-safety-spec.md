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
