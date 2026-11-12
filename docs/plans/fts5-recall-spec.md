# FTS5 memory recall, and ranked recall for encrypted stores: spec

Branch `feat/fts5-recall`, stacked on `feat/eval-profiles` (PR F). Plan: `docs/plans/pluggable-context-implementation.md`, PR G (first part; the workspace document index is the second). Design: `docs/research/pluggable-context-memory-2026-11.md` §4.7.

## Problem

1. **Recall ranks in Rust over a capped corpus.** Since #101, `SqliteStore` loads the newest 5,000 memories (after kind filters) and ranks them with an in-Rust BM25. A relevant memory older than the newest 5,000 is never found, and every turn reads up to 5,000 rows.
2. **Encrypted stores get no "Relevant Memories".** `EncryptedStore::recall` (`crates/hone-memory/src/encrypted.rs`) decrypts every memory and keeps those whose content contains the *whole query* as a substring, so a prompt almost never matches. This predates #101.

SQLite's FTS5 is compiled into the bundled SQLite (`libsqlite3-sys` sets `SQLITE_ENABLE_FTS5`), so an indexed, uncapped ranking needs no new dependency.

```mermaid
%%{init: {"themeVariables": {"fontSize": "13px"}, "flowchart": {"nodeSpacing": 25, "rankSpacing": 30, "padding": 8}, "sequence": {"actorMargin": 30, "messageMargin": 25, "boxMargin": 6}, "gantt": {"barHeight": 16, "barGap": 3}}}%%
flowchart LR
  W["record / update / forget"] --> M["memories"]
  M -->|"triggers"| F["memories_fts (FTS5)"]
  Q["prompt"] --> T["tokens → FTS5 OR query"]
  T --> F --> R["bm25() rank in SQL,<br/>kind + exclusions, top limit"]
  E["EncryptedStore"] --> D["decrypt corpus"] --> B["in-Rust BM25 (as SqliteStore)"]
```

## Rows

| Row | Behaviour |
|---|---|
| G1 | **An FTS5 index mirrors the memories.** `SqliteStore` keeps an FTS5 table over memory content (external content on `memories`, kept in sync by triggers on insert, update and delete). Opening a store creates it if missing and rebuilds it when its row count differs from `memories`. This is not a schema version change (v4 stays reserved for the ledger). Existing databases are indexed on first open. |
| G2 | **`fts5` recaller.** A built-in recaller `fts5` (PR D's `[memory] recaller`) ranks with FTS5's `bm25()` in SQL, applies the kind filter and excluded kinds in the same query, and returns the top `top_k` with no corpus cap. A relevant memory older than 6,000 newer unrelated ones is found. The prompt is turned into an FTS5 query from its word tokens (letters, digits, `_`; at least 3 characters; English stop words dropped; at most 32), each quoted as a string and OR-ed, so FTS5 syntax in a prompt (`"`, `*`, `NEAR`, `-`, `:`) is never interpreted. A prompt with no usable tokens returns nothing. Scores are `-bm25()` (higher is better). |
| G3 | **Ranking quality matches today's BM25 on the same corpus.** On a fixed corpus of memories and queries, `fts5`'s top result equals the in-Rust `bm25` recaller's top result for every query where both return something, and each returns only memories sharing at least one token with the query. |
| G4 | **Receipts and strategies.** Receipts name the strategy `fts5` with params `{"exclude":[…],"limit":N}`. `hone strategies` lists `fts5` as a built-in recaller. The default stays `bm25` until an eval report (PR F) says otherwise. |
| G5 | **Encrypted stores rank too.** `EncryptedStore::recall` with a non-empty query decrypts the newest memories (up to the same 5,000 cap, after the kind filter and exclusions) and ranks them with the same BM25 as `SqliteStore`, returning the top `limit`. So a prompt that shares words with an encrypted memory recalls it. An empty query lists as today. `recall_excluding` applies exclusions before the cap. |
| G6 | **`fts5` with an encrypted store falls back.** FTS5 can't index ciphertext, so choosing `recaller = "fts5"` with `--encrypted-memory` uses `bm25` instead, with one warning on stderr and in `hone strategies`; receipts name `bm25`. |
| G7 | **`hone memory reindex`.** Rebuilds the FTS5 index and prints the number of memories indexed. A store without the FTS5 table gains it. |

## Testing notes

- **G1/G2/G7** on temp SQLite stores: inserts, updates (`record` is `INSERT OR REPLACE`), `forget`, reopening an old database without the FTS table, FTS5 syntax in prompts, 6,000-row corpora.
- **G3** with a fixed corpus of 30–50 memories and 10 queries in a test fixture.
- **G4/G6** through the registry and an agent with the scripted provider.
- **G5** on an `EncryptedStore` over a temp store.

## Out of scope

- The workspace document index (markdown and fixed chunkers, `[retrieval]`), PR G's second part.
- Making `fts5` the default.
