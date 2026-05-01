# Memory and Pass-Through Caching

This tutorial is a task-oriented walkthrough of hone's memory subsystem: what gets remembered, where it lives, how to inspect it, and how to turn it off. Pair it with [`docs/architecture/memory.md`](../architecture/memory.md) for the full design reference.

## What hone actually remembers

There are five distinct layers, and most user confusion comes from conflating them:

| Layer | What | Where | When it persists |
|---|---|---|---|
| 1. Memory store | Q&A pairs from each turn, learning outcomes | `~/.hone/memory.db` (SQLite) | Across sessions |
| 2. `HONE.md` | Project conventions you write yourself | Files in your repo | Versioned in git |
| 3. Context injection | Repo skeleton + relevant files | In-process cache | First turn only |
| 4. Prompt-cache pass-through | The prompt prefix sent to Anthropic | Anthropic's servers | ~5 minutes |
| 5. Local LRU | Cached completions | In-process | Process lifetime |

The rest of this guide walks each layer from a user's standpoint.

## Layer 1: persistent memory store

Every memory has a `kind` (`Conversation`, `User`, `Feedback`, `Project`, `Reference`, `Outcome`) and an optional `description` (one-line summary). The system prompt carries a `## Memory index` section listing IDs + descriptions grouped by kind; the agent fetches bodies on demand via the `memory_read` tool. Recall ordering uses BM25 over content + description.

Not every turn produces a memory. A classifier in the agent loop only records turns that match specific patterns — corrections (`stop doing`, `don't do`), validated decisions, project deadlines, external links — and dedupes against the recent 5 000-memory slice via trigram-Jaccard similarity at threshold 0.8. Conversational chatter is dropped on the floor.

### Where the file lives

```bash
ls -la ~/.hone/memory.db
sqlite3 ~/.hone/memory.db "select count(*) from memories;"
```

If you never see the file, hone is running ephemerally — see below.

### Inspecting recalls in the TUI

The TUI exposes recalls in three places:

- **Status bar / session anatomy** (`Ctrl+A`): per-turn count of memory recalls.
- **Agent rail**: each recall is a sub-event under the turn — open the rail with `Ctrl+R`.
- **Prompt mirror** (`Ctrl+M`): preview the *exact* context sent to the model, including the `## Relevant context from memory:` block.

If a recall surprises you, `Ctrl+M` is the fastest way to see the snippet that the model saw.

### Disabling memory for a session

```bash
hone --ephemeral
```

Or set:

```bash
HONE_EPHEMERAL=1 hone
```

In ephemeral mode hone never opens `memory.db`. Q&A records are not written; nothing is recalled.

### Deleting all memories

There's no slash command for this yet. Drop the database:

```bash
rm ~/.hone/memory.db
```

Hone re-creates it on the next startup. Sessions, audit logs, and the index are kept (they live in different files).

### Inspecting and deleting via the CLI

```bash
hone memory ls                          # list recent memories with kind, weight, description
hone memory ls --kind feedback          # filter by kind
hone memory ls --limit 50               # default 20
hone memory show <id>                   # full body, tags, timestamps
hone memory forget --tag outcome        # bulk delete by tag
hone memory forget --kind conversation  # bulk delete by kind
hone memory forget --before 2026-01-01T00:00:00Z   # bulk delete by RFC3339 cutoff
```

`hone memory forget` requires exactly one filter and prints the affected count. SQL fallbacks are still available if you need surgical edits the CLI doesn't cover, but restart hone after editing — an open `SqliteStore` won't see your changes.

### Encryption (`--encrypted-memory`)

Wraps the on-disk store with AES-256-GCM + Argon2id (per-store random salt sentinel). Opt in at startup:

```bash
export HONE_MEMORY_PASSPHRASE='your-passphrase'
hone --encrypted-memory
```

If `--encrypted-memory` is set without a non-empty `HONE_MEMORY_PASSPHRASE`, the binary exits non-zero with a clear message. If the passphrase is wrong (sentinel decrypt fails), the binary exits non-zero rather than silently falling back to a plaintext or empty store — a typo can't quietly mask your memory.

Properties:

- Per-store random salt persisted as a sentinel record; two stores with the same passphrase but different salts produce different ciphertexts.
- Search is unavoidably client-side (you can't index ciphertext); recall decrypts every record and BM25-scores in memory. Bounded by the 5 000-memory recall cap.
- Rotating the passphrase requires re-encrypting the store (decrypt with old, write to a fresh DB with new). There is no in-place rotation tool yet.

## Layer 2: HONE.md conventions

A markdown file describing project conventions. Hone reads three levels at startup and merges them additively:

| Level | Path | Priority |
|---|---|---|
| User | `~/.config/hone/HONE.md` | lowest |
| Project | `<repo-root>/HONE.md` | middle |
| Working dir | `<cwd>/HONE.md` | highest |

The merged result is prepended to the system prompt verbatim.

Use it for things you want the agent to remember **deterministically** — rules, style notes, "always run X after Y", "never do Z". Don't put per-conversation state here; that's what Layer 1 is for.

Minimum useful `HONE.md`:

```markdown
# Project: hone

## Rules
- Always run `cargo clippy -- -D warnings` after Rust changes.
- Use TDD for non-trivial work.
- Conventional commits.

## Architecture notes
- The TUI is in `crates/hone-tui/`. Theme tokens are in `theme.rs`.
- Sessions persist to `~/.hone/sessions/<id>/`.
```

Missing files are silently ignored — there's no error if you don't have one.

## Layer 3: context injection (turn 1 only)

When the agent runs with `inject_context: true` (the default for interactive mode), the **first turn** is augmented with two tiers:

1. **Skeleton** — a compact projection of the repo (file tree + symbols).
2. **Relevant files** — top-K files retrieved from `hone-index` for your question.

This is not memory — it's a deterministic projection of the current repo. It does not survive across sessions. The cache invalidates when source files change (mtime + file count check).

Tune via:

```bash
hone --skeleton-strategy compact      # paths only
hone --skeleton-strategy symbols      # paths + top-level symbols
hone --skeleton-strategy full         # paths + every public signature
hone --skeleton-strategy adaptive     # auto-select based on token budget
```

Or `HONE_SKELETON_STRATEGY=symbols hone`.

To skip injection entirely (one-shot pass through with no context):

```bash
hone --no-inject-context -i "..."
```

## Layer 4: Anthropic prompt-cache pass-through

When you use the Anthropic adapter, hone marks the prompt prefix with `cache_control: { type: "ephemeral" }`. Anthropic caches that prefix server-side for ~5 minutes; subsequent requests with the same prefix skip re-tokenisation, cutting cost (~10×) and latency (~3×).

You don't configure this — it's automatic for Anthropic, and silently absent for other providers.

What's marked as cacheable:

1. **System prompt** — emitted as a text-block array, `cache_control` on the last block.
2. **Tool schemas** — `cache_control` on the last tool entry; the entire tool array is one cached unit.
3. **Last user message** — promoted to a content-block array if it was a bare string.

Confirm cache hits in the per-turn anatomy (`Ctrl+A`) — `cached_tokens` shows how many of the input tokens hit the upstream cache.

### Why "pass-through"?

Hone does not store anything locally for this layer. It only marks the request structure so the upstream service caches. Restart hone, drop the network, switch terminals — the cache lives on Anthropic's side and persists for ~5 minutes regardless.

### What invalidates the upstream cache

- Any change to the system prompt (HONE.md edit, new memory recall, different model).
- Any change to the tool list.
- Any change in the assistant/user message history except appending.
- 5 minutes of idleness.

If you're cost-optimising, keep the system prompt stable across turns (HONE.md edits cost a full cache miss next turn).

## Layer 5: local response LRU

`CachingProvider` wraps any provider with an in-process LRU keyed on a hash of the completion request:

```rust
let cached = CachingProvider::new(inner, max_entries: 1024);
```

Off by default. Useful in deterministic test runs or replay scenarios. Not useful in normal sessions where every turn carries unique context.

## Putting it together: a typical turn

```mermaid
sequenceDiagram
  participant U as User
  participant H as Hone
  participant L1 as Memory store
  participant L4 as Anthropic API
  U->>H: "fix the login bug"
  Note over H,L1: at session start
  H->>L1: render_index(budget=default)
  L1-->>H: markdown index (IDs + descriptions, grouped by kind)
  H->>H: system prompt = HONE.md + ## Memory index + skeleton
  H->>L4: messages with cache_control on prefix
  L4-->>H: stream tokens; agent calls memory_read(<id>) for relevant entries
  L1-->>H: full body of fetched memory
  H-->>U: response
  Note over H,L1: classifier decides if turn is memory-worthy
  H->>L1: record(kind=Feedback|Project|..., body, dedup at 0.8)
```

Every layer is independent and can be turned off in isolation:

| Disable layer | How |
|---|---|
| 1 | `--ephemeral` |
| 2 | Don't write `HONE.md` files |
| 3 | `--no-inject-context` |
| 4 | Switch to a non-Anthropic provider |
| 5 | Don't wrap with `CachingProvider` (default) |

## Common questions

### "Hone keeps recalling something irrelevant — why?"

Recall is BM25 (lexical, not semantic). A query containing the token "test" will favour memories that *also* contain "test", weighted by IDF and document length. If a memory is dominating recall, options:

- Delete it: `hone memory forget --tag <t>` or `hone memory forget --before <iso>` (see "Inspecting and deleting via the CLI").
- Rephrase your prompt.
- Run with `--ephemeral` if the noise is overwhelming.

The agent additionally sees the `## Memory index` and reads bodies on demand via `memory_read`, so even a high BM25 hit only enters the prompt body when the model decides to fetch it. Semantic (embedding) recall is still on the roadmap.

### "Why is my first turn slow?"

Layer 3 builds the skeleton on the first turn (~10 ms for medium repos, longer for large monorepos). Subsequent turns reuse the cache. If turn 1 is consistently slow:

```bash
HONE_SKELETON_STRATEGY=compact hone   # cheapest
```

Or skip injection entirely with `--no-inject-context`.

### "I edited HONE.md mid-session — does the agent see it?"

No. HONE.md is read once at startup. Restart hone for changes to take effect.

### "Does memory work with ACP providers?"

Yes — Layer 1 is provider-agnostic; Q&A is recorded after every turn regardless of provider. Layer 4 (cache_control) is Anthropic-specific, so ACP `claude --acp` sessions don't get the upstream cache directly — that's controlled by the upstream agent process.

### "I want to migrate memories to a new machine"

```bash
# On old machine:
sqlite3 ~/.hone/memory.db ".dump" > memory-export.sql

# On new machine:
sqlite3 ~/.hone/memory.db < memory-export.sql
```

`MemoryStore::export` / `import` exist programmatically but aren't yet wired to the CLI. Encrypted stores need decrypted-export-then-encrypted-reimport (no direct ciphertext copy across passphrases).

### "How big can the memory store get?"

SQLite handles millions of rows fine. Recall is bounded — `SqliteStore::recall` caps the BM25 universe at the 5 000 most-recent memories on non-empty queries. The empty-query path used by `render_index` does a full table scan, so at corpus sizes north of ~50 K you'll start to feel session-start latency. Decay runs automatically every 24 hours on first open of the day (`decay(0.95, now)`); run `hone memory forget --before <iso>` for hard cleanup.

## Debugging recalls

When a recall surprises you:

1. `Ctrl+M` — open the prompt mirror; see the exact `## Memory index` block sent to the model and any `memory_read` results materialised into the conversation.
2. `RUST_LOG=hone_core::agent=debug hone` — every memory write and `memory_read` call is logged.
3. `hone memory ls --limit 50` — the same view the agent gets, minus bodies. `hone memory show <id>` for any single body.
4. `sqlite3 ~/.hone/memory.db "select id, kind, weight, substr(content, 1, 60) from memories order by created_at desc limit 20;"` — raw store inspection. The schema is versioned via the `schema_version` table.

## Related

- [`docs/architecture/memory.md`](../architecture/memory.md) — design reference (data model, traits, cache invalidation rules, source map).
- [`docs/architecture/multi-agent-and-memory.md`](../architecture/multi-agent-and-memory.md) — how multi-agent coordination uses the memory store.
- [`docs/tutorials/security.md`](security.md) — ephemeral mode, audit logging, sandbox.
- [`docs/tutorials/providers.md`](providers.md) — which providers honor `cache_control`, fallback chains.
- [`docs/tutorials/tui-keybindings-and-themes.md`](tui-keybindings-and-themes.md) — `Ctrl+A`, `Ctrl+M`, `Ctrl+R` overlays.
