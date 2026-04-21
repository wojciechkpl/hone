# Goose vs Hone — Claude CLI Integration & Conversational Context

A code-grounded comparison of how [block/goose](https://github.com/block/goose)
and this repository (`hone`) each handle Claude/Anthropic integration,
multi-turn conversational state, tool-use loops, and session persistence.

Research date: 2026-04-20. Goose code inspected via raw.githubusercontent.com
on `main` branch. Hone code inspected from working tree at branch `main`.

---

## 1. TL;DR

- **Provider integration.** Goose talks to Anthropic *only* via the HTTP API
  (`POST /v1/messages` with `stream: true`, SSE parsed on the client). Hone has
  both an HTTP adapter (`anthropic.rs`, the primary path) *and* a CLI
  pass-through adapter (`claude_cli.rs`) that spawns the `claude` binary and
  parses its `--output-format stream-json` NDJSON. Hone is unusual in shipping
  the CLI subprocess as a first-class provider.
- **Conversation state.** Both tools are *stateless at the provider layer*:
  the full `Vec<Message>` conversation is re-sent on every turn. Neither
  relies on any server-side session id held by Anthropic. The difference is
  in local persistence: Goose uses SQLite (one row per message) via a
  `SessionManager`; hone also uses SQLite but stores the whole message history
  as a JSON blob in one column.
- **Tool-use loop.** Both implement the classic "stream until stop → dispatch
  tool calls → append tool results → call provider again" loop inside a
  single `loop { ... }` driven by the agent, not the provider. Hone's CLI
  pass-through path is the one real divergence: when the `claude` binary runs
  its own bash/read/edit tools, hone only *observes* (`ObservedToolStart` /
  `ObservedToolEnd`) for display and never executes the tool itself.

---

## 2. Goose architecture

Goose is a Rust agent with a `crates/`-based workspace. The relevant crates
for this comparison are `goose` (library: providers, agents, session) and
`goose-cli` (TUI/CLI wrapping it).

### 2.1 Provider — `crates/goose/src/providers/anthropic.rs`

Goose hits the Anthropic HTTP API directly with `reqwest`. There is **no code
path that spawns the `claude` binary** anywhere in `providers/`.

```rust
// crates/goose/src/providers/anthropic.rs (excerpted)
async fn stream(
    &self,
    model_config: &ModelConfig,
    session_id: &str,
    system: &str,
    messages: &[Message],
    tools: &[Tool],
) -> Result<MessageStream, ProviderError> {
    let mut payload = create_request(model_config, system, messages, tools)?;
    payload
        .as_object_mut()
        .unwrap()
        .insert("stream".to_string(), Value::Bool(true));

    let response = self
        .api_client
        .request(Some(session_id), "v1/messages")
        .response_post(&payload)
        .await?;
    let stream = response.bytes_stream().map_err(io::Error::other);
    Ok(response_to_streaming_message(framed))
}
```

Key points:

- Endpoint: `v1/messages`.
- Every call receives the full `messages: &[Message]` slice; the provider
  itself keeps no history. `session_id` is only passed for logging/rate-limit
  bucketing, not to make the server stateful.
- Streaming is SSE: `response.bytes_stream()` is framed line-by-line and
  `response_to_streaming_message` decodes the Anthropic event types
  (`content_block_delta`, `message_stop`, …) into a `MessageStream`.

### 2.2 Agent loop — `crates/goose/src/agents/agent.rs`

The main reply loop lives in `reply_internal()`. It is the conventional
"stream → dispatch tools → re-call provider" pattern, and the full
conversation is re-built on every iteration.

```rust
// Essence of agent::reply_internal (reconstructed from source)
loop {
    let mut stream = Self::stream_response_from_provider(
        self.provider().await?,
        &session_config.id,
        &system_prompt,
        conversation_with_moim.messages(),   // full history, re-sent
        &tools,
        &toolshim_tools,
    ).await?;

    while let Some(next) = stream.next().await {
        match next {
            Ok((response, usage)) => {
                let ToolCategorizeResult {
                    frontend_requests,
                    remaining_requests,
                    filtered_response,
                } = self.categorize_tools(&response, &tools, ...).await;
                // dispatch via self.dispatch_tool_call(...)
                // populate request_to_response_map -> messages_to_add
            }
            Err(ProviderError::ContextLengthExceeded(_)) => {
                // trigger compact_messages()
            }
        }
    }

    for msg in &messages_to_add {
        session_manager.add_message(&session_config.id, msg).await?;
    }
    conversation.extend(messages_to_add);
}
```

So follow-ups carry prior context by **re-sending the entire
`conversation.messages()` vector** on every iteration. When the context
window is exceeded, `compact_messages()` is called (summarisation /
drop-oldest strategy — not inspected in depth here).

### 2.3 Session persistence — `crates/goose/src/session/session_manager.rs`

Goose stores sessions in a SQLite database `sessions.db` via `sqlx`. The
`Session` struct is:

```rust
pub struct Session {
    pub id: String,
    pub working_dir: PathBuf,
    pub name: String,
    pub user_set_name: bool,
    pub session_type: SessionType,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub extension_data: ExtensionData,
    pub total_tokens: Option<i32>,
    pub conversation: Option<Conversation>,
    pub message_count: usize,
    pub provider_name: Option<String>,
    pub model_config: Option<ModelConfig>,
    pub goose_mode: GooseMode,
    pub thread_id: Option<String>,
}
```

- Storage: `SessionStorage { pool: Pool<Sqlite>, initialized: OnceCell<()>,
  session_dir: PathBuf }` with dedicated `sessions` and `messages` tables and
  explicit migrations (schema v9 at time of writing).
- `add_message` inserts one row per message within a transaction that also
  bumps `updated_at`.
- `replace_conversation` deletes then re-inserts all messages atomically — the
  hook Goose uses after compaction.
- `get_session(id, include_conversation: bool)` returns just metadata or the
  full history. Resume = "load messages from DB → attach to the in-memory
  `Conversation` → re-enter `reply_internal`".
- Before SQLite, Goose used per-session JSONL files; from v1.10.0 it auto-
  imports legacy `.jsonl` on first run.

---

## 3. Hone architecture

### 3.1 Two provider paths — HTTP and CLI subprocess

Hone has a conventional HTTP adapter and, unusually, a **subprocess adapter**
that shells out to the installed `claude` binary.

#### HTTP adapter — `crates/hone-providers/src/adapters/anthropic.rs`

Same shape as Goose: `reqwest`, POST to `{base_url}/v1/messages`, `stream:
true`, SSE parsing of `content_block_delta` etc. The request body is built
from the full message list per call:

```rust
// crates/hone-providers/src/adapters/anthropic.rs:172
let mut body = json!({
    "model": model_name,
    "max_tokens": max_tokens,
    "messages": messages,
    "stream": true,
});

// ... system prompt lifted out separately with cache_control:
body["system"] = json!([{
    "type": "text",
    "text": system,
    "cache_control": { "type": "ephemeral" },
}]);
```

Notable detail: hone *explicitly* attaches an `ephemeral` `cache_control`
breakpoint to the system prompt (and to the last tool) to enable Anthropic
prompt caching for the stable prefix. Goose's request-building code at
`super::formats::anthropic::create_request` was not inspected here, so a
direct comparison on caching is "not determined from available sources".

#### CLI adapter — `crates/hone-providers/src/adapters/claude_cli.rs`

This is the piece that has no Goose counterpart. Hone can use the installed
`claude` binary as its model backend, flattening the whole message history
into a single stdin prompt:

```rust
// crates/hone-providers/src/adapters/claude_cli.rs:103-112
let mut cmd = Command::new(&self.binary);
cmd.arg("-p")
    .arg("--output-format").arg("stream-json")
    .arg("--verbose")
    .arg("--model").arg(&model_name);

Ok(spawn_claude_stream_with_stdin(cmd, prompt))
```

The `prompt` is `messages_to_prompt(&request.messages)` — a plain-text
flattening of every message's role + content, including tool-call and
tool-result markers:

```rust
// crates/hone-providers/src/adapters/cli_common.rs:45
pub fn messages_to_prompt(messages: &[Message]) -> String {
    let mut parts = Vec::with_capacity(messages.len());
    for msg in messages {
        let role_label = match msg.role { /* SYSTEM / USER / ASSISTANT / TOOL */ };
        let mut text = msg.content.as_text().to_owned();
        for tc in &msg.tool_calls {
            let args = serde_json::to_string(&tc.arguments).unwrap_or_default();
            text.push_str(&format!("\n[tool_use id={} name={} input={}]", tc.id, tc.name, args));
        }
        if let Some(ref id) = msg.tool_call_id {
            text = format!("[tool_result id={id}]\n{text}");
        }
        parts.push(format!("{role_label}:\n{text}"));
    }
    parts.join("\n\n")
}
```

The subprocess is invoked fresh for every turn (it exits on
`{"type":"result",…}`). The CLI's stream-json output is parsed into
`ProviderEvent`s:

- `assistant` blocks with `type: "text"` → emit `ProviderEvent::Token` for the
  *new* substring (the CLI sends cumulative text, so hone diffs against
  `last_text`).
- `assistant` blocks with `type: "tool_use"` → emit
  `ProviderEvent::ObservedToolStart` — the CLI is about to run one of *its
  own* built-in tools; hone does **not** execute it.
- `user` blocks with `type: "tool_result"` → emit
  `ProviderEvent::ObservedToolEnd`.
- `type: "result"` → emit `ProviderEvent::Usage` (input/output/cache_read) and
  break.

Because hone is not executing those tools, it has to re-send the full
text-flattened history on the *next* turn so the CLI sees the full
conversation. There is no way to hand the CLI a live session id.

### 3.2 Agent loop — `crates/hone-core/src/agent.rs`

`Agent::run_turn` is the classic stream-and-dispatch loop:

```rust
// crates/hone-core/src/agent.rs:360 (excerpted)
loop {
    let mut messages = self.context.build_messages(&self.config.system_prompt);
    // … optional Relevant-Memories system message inserted here …

    let mut request = CompletionRequest::new(self.config.model.clone(), messages);
    request.tools = self.config.tools.schemas();
    let mut stream = retry(&retry_cfg, || prov.complete(req.clone()),
                           is_transient_error).await?;

    let mut pending_tool_calls: Vec<ToolCallRequest> = Vec::new();
    while let Some(event) = stream.next().await {
        match event {
            ProviderEvent::Token { text } => { assistant_text.push_str(&text); … }
            ProviderEvent::ToolCall(call)  => pending_tool_calls.push(call),
            ProviderEvent::Usage(u)        => { turn_usage = turn_usage + u; … }
            ProviderEvent::ObservedToolStart { id, name, input } => {
                let _ = events.send(AgentEvent::ToolCallStart { id, name, args: input }).await;
            }
            ProviderEvent::ObservedToolEnd { id, result, is_error } => { … }
            ProviderEvent::Done => break,
            ProviderEvent::Error { message } => { return Err(…); }
            _ => {}
        }
    }

    if pending_tool_calls.is_empty() {
        self.context.add_message(Message::assistant(assistant_text));
        break;
    }
    // append assistant msg with tool_calls, dispatch tools (parallel for
    // read-only, sequential otherwise), append tool-result messages, loop.
}
```

Context is a local `ContextManager` (see `crates/hone-core/src/context.rs`)
that stores `messages: Vec<Message>` and exposes `build_messages(system)` to
rebuild the full prompt prefix each turn. Like Goose, nothing survives in the
provider between calls — hone is entirely responsible for carrying context.

One behavioural wrinkle: for tool calls hone distinguishes *executed*
(`ProviderEvent::ToolCall`) from *observed* (`ObservedToolStart/End`). The
observed path exists specifically to surface the claude-CLI's internal tool
activity in the TUI without trying to re-execute it.

### 3.3 Session persistence — `bins/hone/src/session.rs`

**Scope note**: Hone has 6+ SQLite schemas across different domains — memory
(`hone-memory`), tool-result cache (`hone-cache`), code index (`hone-index`),
orchestration checkpoints (`hone-core/checkpoint.rs`), server session registry
(`hone-server/sqlite_session.rs`, just id+created_at), and the CLI session
store covered below. The comparison here is only about **CLI conversation
persistence**, the direct analogue to Goose's `session_storage`.

For CLI sessions specifically, hone's schema is *much* simpler than Goose's:
one row per session, messages as a JSON blob.

```sql
-- bins/hone/src/session.rs:6-14
CREATE TABLE cli_sessions (
    id         TEXT PRIMARY KEY,
    name       TEXT UNIQUE NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    messages   TEXT NOT NULL        -- JSON-encoded Vec<Message>
);
```

Resume is done by loading the JSON blob, deserialising to `Vec<Message>`, and
replaying into a freshly spawned agent via
`AgentHandle::load_history(messages)` — see `bins/hone/src/main.rs:1869`:

```rust
if is_resume {
    // … fetch messages from SessionStore …
    agent.load_history(messages).await;
}
```

There is also a server-side `SqliteSessionStore`
(`crates/hone-server/src/sqlite_session.rs`) used by `honed` for the HTTP API
path; the CLI and the daemon do not share the same DB.

---

## 4. Side-by-side comparison

| Aspect | Goose | Hone |
|---|---|---|
| Claude integration | HTTP only (`POST /v1/messages`, SSE). `reqwest` + `async_stream`. | **HTTP** (`anthropic.rs`) *and* **CLI subprocess** (`claude_cli.rs`, NDJSON `stream-json`). |
| Request shape | `create_request(model, system, messages, tools)` → JSON payload. | `build_request_body(...)` → JSON payload; system lifted out with `cache_control: ephemeral`. |
| Context carried across turns | Full `Conversation::messages()` re-sent every call. Provider stateless. | Full `ContextManager::messages` re-sent every call (or flattened to a text prompt for the CLI path). Provider stateless. |
| Agent loop shape | `loop { stream → categorise tools → dispatch → extend conversation → repeat }` in `agents::agent::reply_internal`. | `loop { build_messages → stream → collect pending_tool_calls → dispatch (parallel for read-only) → append tool results → repeat }` in `hone-core::agent::run_turn`. |
| Tool execution ownership | Agent dispatches every tool; results appended to conversation. | Agent dispatches hone-native tools *and* observes (without executing) the claude-CLI's built-in tools via `ObservedToolStart/End`. |
| Compaction / context length | `ProviderError::ContextLengthExceeded` → `compact_messages()` (summarise + prune). Details not inspected. | `ContextManager` with relevance-based reordering of middle-zone tool messages (BM25) + summary markers; compaction logic present but not detailed in this report. |
| Session persistence | SQLite `sessions.db` with normalised `sessions` and `messages` tables (schema v9), migrated from legacy JSONL. | SQLite `sessions.db`, **single-table** `cli_sessions` with `messages` stored as a JSON blob; server path uses a separate `SqliteSessionStore`. |
| Resume semantics | `SessionManager::get_session(id, true)` loads all rows → attaches `Conversation` → re-enters `reply_internal`. | Load JSON blob → `agent.load_history(messages)` → next `run_turn` uses it. |
| Extra session metadata | working_dir, provider_name, model_config, extension_data, token totals, thread_id, goose_mode. | id / name / created_at / updated_at / messages. Cost, metrics, and provider are not persisted in the CLI session row. |
| Prompt caching | Not determined from available sources. | Explicit `cache_control: ephemeral` breakpoints on system prompt and last tool schema. |

---

## 5. Concrete differences

### What Goose does that hone doesn't

1. **Normalised per-message SQLite schema with schema versioning.** Goose has
   a `messages` table with per-row inserts inside transactions, schema v9,
   plus search indexes (`chat_history_search.rs`). Hone stores the whole
   history as one JSON blob — cheaper to write, but makes history search and
   partial-row updates impossible.
2. **Richer session metadata.** Goose persists working_dir, provider_name,
   model_config, extension state, token totals, and `thread_id` directly on
   the session row. Hone persists name + messages + timestamps only.
3. **Explicit `replace_conversation` for compaction.** Goose has an atomic
   "delete + re-insert all messages" operation, which is the clean hook for
   after compaction. Hone's blob model gets this for free but at the cost of
   rewriting the whole history every time.
4. **Session fork / chat-history search / JSON import-export** as first-class
   operations.
5. **`session_id` threaded through provider calls** (even though Anthropic is
   stateless) for logging/rate-limit bucketing. Hone's provider trait does
   not carry a session identifier.

### What hone does that Goose doesn't

1. **`ClaudeCliProvider`.** Hone can drive the actual `claude` binary as a
   backend. No equivalent exists in Goose — Goose always talks HTTP. This is
   both a feature (bring-your-own-CLI, inherits CLI's built-in tools, uses
   CLI's auth) and a constraint (no `supports_tools`, flattened text prompt,
   can't hand the CLI a live session).
2. **Observed-tool events.** `ProviderEvent::ObservedToolStart` /
   `ObservedToolEnd` exist specifically so the agent can forward
   provider-internal tool activity to the TUI without dispatching it. Goose
   assumes tools always flow through its dispatcher.
3. **Explicit Anthropic prompt caching markers** (`cache_control: ephemeral`)
   baked into `build_request_body`. Goose may do this too — not determined
   from available sources.
4. **Relevance-aware context rebuild.** `ContextManager::build_messages`
   partitions messages into start/middle/recent zones and reorders middle-
   zone `Role::Tool` messages by BM25 relevance to the latest user query.
   Goose compacts, but BM25 mid-zone reordering was not observed in
   `agent.rs`.
5. **Parallel dispatch of consecutive read-only tool calls** inside a single
   turn via `JoinSet` in `Agent::run_turn`. Goose categorises tools
   (`categorize_tools`) but the parallel read-only fast path was not
   observed.
6. **Injection detector** on every tool result (and on observed-tool results
   from the claude CLI) that prepends a `[SECURITY: …]` banner when the
   content looks like an injection attempt.

---

## 6. Recommendations for hone

Scoped conservatively — only changes where Goose's approach is clearly
superior, not cosmetic.

1. **Split the `messages` blob into a normalised `messages` table.**
   The current blob-in-a-column design makes it impossible to:
   - Search past conversations (Goose has `chat_history_search.rs` for a
     reason).
   - Stream-append one message without rewriting the whole row.
   - Share messages across forked sessions without duplicating the blob.

   Schema suggestion (close to what Goose uses):

   ```sql
   CREATE TABLE cli_sessions (
       id TEXT PRIMARY KEY,
       name TEXT UNIQUE NOT NULL,
       working_dir TEXT,
       provider_name TEXT,
       model_id TEXT,
       total_input_tokens INTEGER,
       total_output_tokens INTEGER,
       total_cost_usd REAL,
       created_at INTEGER NOT NULL,
       updated_at INTEGER NOT NULL
   );
   CREATE TABLE cli_messages (
       session_id TEXT NOT NULL REFERENCES cli_sessions(id),
       seq INTEGER NOT NULL,
       role TEXT NOT NULL,
       content TEXT NOT NULL,       -- JSON for rich content
       tool_calls TEXT,             -- JSON
       tool_call_id TEXT,
       created_at INTEGER NOT NULL,
       PRIMARY KEY (session_id, seq)
   );
   CREATE INDEX idx_cli_messages_session ON cli_messages(session_id, seq);
   ```

   Needs a migration (schema version column) — worth doing once, then treated
   as the default going forward.

2. **Persist the session's provider/model and cost totals on the session row.**
   Currently a resumed session has no record of which model produced those
   tokens or how much has been spent. Goose persists `provider_name`,
   `model_config`, and token totals; hone has `CostAccumulator` in memory
   only.

3. **Add `fork_session`.** Goose supports duplicating a session to try a
   divergent branch. Given hone's interactive TUI, this is a cheap quality-
   of-life win once messages are in their own table.

4. **Add chat-history search over persisted messages.** Trivial once messages
   are normalised (`FTS5` virtual table, or just `LIKE` queries). Useful
   because hone already has `hone-index` for code — extending to chat
   transcripts keeps the UX consistent.

5. **Thread a `session_id` through `CompletionRequest`.** Even though the
   Anthropic API doesn't care, having the id inside the provider lets the
   telemetry/egress-log/retry layers correlate requests to their session
   without reaching back up through `events`. Goose already does this.

6. **Decide whether `ClaudeCliProvider` remains a first-class path.** The
   flatten-to-text prompt is lossy (it drops the tool_calls structure into a
   string marker) and the CLI runs its own tool sandbox on top of hone's. If
   the answer is "yes, it's a feature", keep a round-trip test that
   guarantees `messages_to_prompt` is a faithful reversible rendering of the
   history on resume. If "no", consider gating it behind a feature flag — it
   is currently enumerated in `supported_models` as five cli: variants and
   easy to pick by accident.

Explicitly *not* recommended:

- Adopting Goose's `reply_internal`-style single-monolithic loop. Hone's
  `run_turn` is cleaner (per-turn guards, parallel read-only dispatch,
  injection scan, proactive context injection on first turn) and has no
  feature parity problem with Goose here.
- Dropping the CLI adapter. It's a real differentiator for users who already
  authenticate `claude` locally.

---

## Sources

- Goose agent loop: `crates/goose/src/agents/agent.rs` (`reply_internal`).
- Goose Anthropic provider: `crates/goose/src/providers/anthropic.rs`.
- Goose session manager: `crates/goose/src/session/session_manager.rs`.
- Goose session management overview: <https://deepwiki.com/block/goose/4.3-session-management>.
- Hone Claude CLI adapter: `crates/hone-providers/src/adapters/claude_cli.rs`.
- Hone HTTP adapter: `crates/hone-providers/src/adapters/anthropic.rs`.
- Hone agent loop: `crates/hone-core/src/agent.rs::Agent::run_turn`.
- Hone context rebuild: `crates/hone-core/src/context.rs::ContextManager::build_messages`.
- Hone CLI session store: `bins/hone/src/session.rs`.
