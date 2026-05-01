use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::Stdio;
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, Role, TokenUsage};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, OwnedMutexGuard, mpsc};
use tokio_util::sync::CancellationToken;

use crate::adapters::cli_common::{check_binary_available, estimate_tokens_heuristic};
use crate::error::ProviderError;
use crate::traits::{CompletionRequest, ModelProvider};

// ---------------------------------------------------------------------------
// Stderr ring buffer
// ---------------------------------------------------------------------------

/// Maximum number of stderr lines retained per process instance.
///
/// Capped at 16 lines to bound memory usage and prevent sensitive content
/// from accumulating in long-lived error buffers.  Lines beyond this limit
/// push the oldest entry out (ring-buffer semantics via `VecDeque`).
const STDERR_RING_CAPACITY: usize = 16;

/// A shared, cheaply-cloneable ring buffer for capturing the most recent
/// stderr lines emitted by a single `claude` child process.
///
/// Ownership contract:
/// - Created fresh with every `spawn_process_now` call so each process
///   instance has its own independent buffer.
/// - Cloned into the background stderr-drain task (push side).
/// - Owned by `CliProcess` (snapshot side) so `complete()` can read it
///   immediately before tombstoning a dying process.
///
/// The inner `Mutex` is a **`std::sync::Mutex`** (not `tokio::sync::Mutex`)
/// because we never hold the lock across an `.await` point — only push/pop
/// under lock and release immediately.
///
/// **Sensitive-data caveat.** The captured stderr is embedded verbatim in
/// `ProviderError::Http { body }` returned to callers and may flow into
/// downstream logs/traces. If the `claude` CLI ever writes secrets
/// (e.g. an `Authorization: Bearer ...` line under verbose tracing) to
/// stderr, those secrets reach the caller. This is no worse than the
/// existing `tracing::debug!` mirror but worth a redaction pass before
/// surfacing to untrusted UI.
type StderrBuf = Arc<StdMutex<VecDeque<String>>>;

/// Create a fresh, empty stderr ring buffer.
fn new_stderr_buf() -> StderrBuf {
    Arc::new(StdMutex::new(VecDeque::with_capacity(STDERR_RING_CAPACITY)))
}

/// Snapshot the current contents of a stderr buffer as a single joined string.
///
/// Returns `None` when the buffer is empty or when the mutex is poisoned
/// (the latter is treated as "no data available" to avoid propagating the
/// poison across call sites — mutex poisoning here only means a previous
/// stderr-drain task panicked, which is already visible via the panic log).
fn snapshot_stderr(buf: &StderrBuf) -> Option<String> {
    // Lock, copy out, release immediately — no await inside.
    let Ok(guard) = buf.lock() else {
        return None; // mutex poisoned — treat as empty
    };
    if guard.is_empty() {
        None
    } else {
        Some(guard.iter().cloned().collect::<Vec<_>>().join("\n"))
    }
}

// ---------------------------------------------------------------------------
// Persistent child-process state
// ---------------------------------------------------------------------------

/// Owns the open stdin/stdout handles of a single long-running `claude` process.
/// All turns on the same `ClaudeCliProvider` instance share this struct under a `Mutex`.
struct CliProcess {
    stdin: ChildStdin,
    stdout_lines: Lines<BufReader<ChildStdout>>,
    /// Stable session UUID generated at spawn time and sent with every NDJSON
    /// turn on this process.  Lives here (not on the provider) so a fresh UUID
    /// is generated automatically whenever the process is re-spawned.
    session_id: String,
    /// Ring buffer capturing the most recent [`STDERR_RING_CAPACITY`] lines of
    /// stderr for this process instance.  Populated by the background drain
    /// task; read by `complete()` when the process dies to surface diagnostics
    /// in the returned `Err`.
    stderr_buf: StderrBuf,
}

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// A `ModelProvider` that delegates to the `claude` CLI tool.
///
/// The provider keeps **one persistent child process** alive for its entire
/// lifetime and multiplexes turns over it via NDJSON on stdin/stdout.
///
/// Requires the `claude` binary to be installed and available on `PATH`.
pub struct ClaudeCliProvider {
    binary: String,
    /// The permission mode passed to `--permission-mode` at spawn time.
    /// Stored here so re-spawn after process death can reuse the same value
    /// without re-reading the environment (which would be racey under parallel
    /// test execution).
    permission_mode: String,
    /// Optional path to an MCP server configuration JSON file.
    ///
    /// When `Some`, the spawned process receives `--mcp-config <path>` so it
    /// loads MCP servers from this file.  When `None`, the flag is omitted.
    /// This is a per-provider, immutable value set once at construction via
    /// [`with_mcp_config`]; it survives process death and is reused on every
    /// re-spawn within this provider's lifetime.
    mcp_config_path: Option<PathBuf>,
    /// When `true`, the spawned process receives `--strict-mcp-config` as a
    /// standalone argv token, restricting MCP server discovery to the file
    /// passed via `--mcp-config`.  Defaults to `false` (opt-in).
    ///
    /// This is a per-provider, immutable-after-construction toggle set via
    /// [`with_strict_mcp_config`]; it survives re-spawn after process death
    /// and is reused on every re-spawn within this provider's lifetime.
    strict_mcp_config: bool,
    /// The persistent child process, shared across all `complete()` calls.
    ///
    /// Spawn is **lazy**: the slot starts as `None` and the process is spawned
    /// on the first `complete()` call, after `request.messages` has been read
    /// so the system prompt can be extracted and passed via `--system-prompt`.
    /// `None` means either "not yet spawned" or "tombstoned (dead process)";
    /// `complete()` will spawn or re-spawn when it finds `None`.
    process: Arc<Mutex<Option<CliProcess>>>,
    /// The system prompt captured from the FIRST `complete()` call's messages.
    ///
    /// This is a per-session, once-written value: set from the first turn's
    /// `Role::System` messages (if any) and never updated thereafter.
    /// Re-spawns after process death reuse this same value so the system prompt
    /// remains consistent for the entire provider lifetime.
    ///
    /// Uses `std::sync::Mutex` (not `tokio::sync::Mutex`) because we never hold
    /// the lock across an `.await` — only read/write under lock and release
    /// immediately.
    system_prompt: Arc<StdMutex<Option<String>>>,
}

impl ClaudeCliProvider {
    /// Create a provider using the `claude` binary found on `PATH`.
    pub fn new() -> Self {
        Self::with_binary("claude")
    }

    /// Create a provider using a specific binary path (useful for tests).
    ///
    /// The child process is **not spawned here**.  Spawning is deferred to the
    /// first `complete()` call so that `request.messages` can be read to extract
    /// any `Role::System` messages and pass them via `--system-prompt` at spawn
    /// time.  The system prompt is a session-level flag; once captured from the
    /// first turn it is reused for all re-spawns within this provider lifetime.
    pub fn with_binary(binary: impl Into<String>) -> Self {
        let binary = binary.into();

        let permission_mode = std::env::var("HONE_CLAUDE_PERMISSION_MODE")
            .unwrap_or_else(|_| "acceptEdits".to_owned());

        Self {
            binary,
            permission_mode,
            mcp_config_path: None,
            strict_mcp_config: false,
            process: Arc::new(Mutex::new(None)),
            system_prompt: Arc::new(StdMutex::new(None)),
        }
    }

    /// Configure the MCP config file path passed to `claude --mcp-config`.
    ///
    /// When set, the spawned process receives `--mcp-config <path>` so it loads
    /// MCP servers from this JSON file.  When unset, the flag is omitted.
    /// The path is per-provider (not per-turn) and survives re-spawn after
    /// process death.
    pub fn with_mcp_config(mut self, path: impl Into<PathBuf>) -> Self {
        self.mcp_config_path = Some(path.into());
        self
    }

    /// When called, the spawned process receives `--strict-mcp-config`,
    /// restricting MCP server discovery to the file passed via `--mcp-config`.
    ///
    /// This is a boolean toggle (no value follows the flag in argv).  The
    /// setting is per-provider and survives re-spawn after process death.
    /// Calling this method multiple times is idempotent.
    pub fn with_strict_mcp_config(mut self) -> Self {
        self.strict_mcp_config = true;
        self
    }
}

/// Spawn the `claude` process and return a `CliProcess` with async handles,
/// or `None` if spawning fails.
///
/// `tokio::process::Command::spawn()` is a synchronous call but requires an
/// active tokio runtime.  If called outside a runtime (e.g. from a plain
/// `#[test]` that only calls `supported_models()`), the spawn will fail and
/// we return `None`; `complete()` will surface a clear error at call time
/// instead of panicking here.
///
/// When `system_prompt` is `Some(text)`, `--system-prompt <text>` is appended
/// to the spawn argv.  When `None`, no `--system-prompt` flag is emitted.
fn spawn_process_now(
    binary: &str,
    permission_mode: &str,
    system_prompt: Option<&str>,
    mcp_config_path: Option<&Path>,
    strict_mcp_config: bool,
) -> Option<CliProcess> {
    // Check whether a tokio runtime is active before attempting the spawn.
    // `Handle::try_current()` is cheap and infallible — it returns `Err` when
    // there is no runtime on the current thread.
    if tokio::runtime::Handle::try_current().is_err() {
        return None;
    }

    let mut cmd = Command::new(binary);
    cmd.arg("--input-format")
        .arg("stream-json")
        .arg("--output-format")
        .arg("stream-json")
        .arg("--verbose")
        .arg("--permission-mode")
        .arg(permission_mode);

    if let Some(prompt) = system_prompt {
        cmd.arg("--system-prompt").arg(prompt);
    }

    if let Some(path) = mcp_config_path {
        cmd.arg("--mcp-config").arg(path);
    }

    if strict_mcp_config {
        cmd.arg("--strict-mcp-config");
    }

    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .ok()?;

    let stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;

    // Create a fresh stderr ring buffer for this process instance.
    // The Arc is cloned into the drain task (push side) and kept on the
    // CliProcess (snapshot side).  A brand-new buffer per spawn ensures
    // per-attempt scoping: the second attempt's buffer never sees the first
    // attempt's lines.
    let stderr_buf = new_stderr_buf();
    let stderr_buf_task = Arc::clone(&stderr_buf);

    // Drain stderr line-by-line in a separate task so it never blocks stdout
    // and auth failures / CLI panics are visible in logs.  The task holds the
    // stderr handle and a clone of the ring buffer — no other CliProcess state.
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let trimmed = line.trim().to_owned();
                if !trimmed.is_empty() {
                    // Both log AND capture — exclusive of neither.
                    tracing::debug!(source = "claude-cli-stderr", "{trimmed}");

                    // Push under lock; release immediately (no await inside).
                    // Ignore mutex poison — a poisoned stderr buffer means a
                    // previous panic in this same task, which cannot happen here
                    // since this is the only task that writes to the buffer.
                    if let Ok(mut buf) = stderr_buf_task.lock() {
                        if buf.len() == STDERR_RING_CAPACITY {
                            buf.pop_front(); // evict oldest to stay within cap
                        }
                        buf.push_back(trimmed);
                    }
                }
            }
        });
    }

    // Keep the child alive in the background; it exits when stdin is dropped.
    tokio::spawn(async move {
        let _ = child.wait().await;
    });

    // Generate a fresh session UUID for this new process instance.
    // A new UUID is intentional: the new child has never seen a prior session_id,
    // so reusing one from a dead process could confuse it.
    let session_id = uuid::Uuid::now_v7().to_string();

    Some(CliProcess {
        stdin,
        stdout_lines: BufReader::new(stdout).lines(),
        session_id,
        stderr_buf,
    })
}

impl Default for ClaudeCliProvider {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// NDJSON helpers
// ---------------------------------------------------------------------------

/// Build the `last_err` message used when a process dies.
///
/// When `stderr` is `Some`, the lines are appended so operators can diagnose
/// auth failures and similar crashes that write to stderr before exiting.
/// When `stderr` is `None` (no output captured), only the base message is used.
fn build_process_died_msg(base: &str, stderr: Option<&str>) -> String {
    match stderr {
        Some(lines) if !lines.is_empty() => {
            format!("{base}; stderr: {lines}")
        }
        _ => base.to_owned(),
    }
}

/// Extract the text of the **last** user-role message from the message list.
///
/// Returns `None` when no user message is present; the caller must treat that
/// as a protocol error rather than forwarding an empty string.
fn last_user_text(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .find(|m| matches!(m.role, Role::User))
        .map(|m| m.content.as_text())
}

/// Collect all `Role::System` messages and join their text with `"\n\n"`.
///
/// Returns `None` when no System messages are present so callers can
/// distinguish "no system prompt" from "empty system prompt".  An empty
/// system prompt text is also treated as absent (filtered out) to avoid
/// passing a blank `--system-prompt` flag.
///
/// Multiple system messages are joined with a blank line (`\n\n`) which is
/// the conventional markdown separator between distinct instruction blocks.
fn extract_system_prompt(messages: &[Message]) -> Option<String> {
    let parts: Vec<String> = messages
        .iter()
        .filter(|m| matches!(m.role, Role::System))
        .map(|m| m.content.as_text())
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n\n"))
    }
}

/// Serialise a single user turn into the NDJSON envelope expected by
/// `claude --input-format stream-json`.
fn build_ndjson_line(session_id: &str, user_text: &str) -> String {
    let payload = serde_json::json!({
        "type": "user",
        "session_id": session_id,
        "message": {
            "role": "user",
            "content": [
                {
                    "type": "text",
                    "text": user_text
                }
            ]
        }
    });
    payload.to_string()
}

// ---------------------------------------------------------------------------
// Stream-json output parser (stateless, per-line)
// ---------------------------------------------------------------------------

/// Parse one line of stream-json output and push zero or more `ProviderEvent`s
/// onto `out`.
///
/// Returns `true` when the line is a `result` with `subtype == "success"`,
/// signalling end-of-turn.
fn parse_line(line: &str, out: &mut Vec<ProviderEvent>, last_text: &mut String) -> bool {
    let Ok(json) = serde_json::from_str::<serde_json::Value>(line) else {
        return false;
    };

    match json.get("type").and_then(|t| t.as_str()) {
        Some("assistant") => {
            let content = json
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());

            if let Some(blocks) = content {
                for block in blocks {
                    match block.get("type").and_then(|t| t.as_str()) {
                        Some("text") => {
                            if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                // The CLI may emit cumulative text; emit only the new delta.
                                if text.len() > last_text.len()
                                    && text.starts_with(last_text.as_str())
                                {
                                    let new_part = &text[last_text.len()..];
                                    if !new_part.is_empty() {
                                        out.push(ProviderEvent::Token {
                                            text: new_part.to_owned(),
                                        });
                                    }
                                } else if text != last_text {
                                    out.push(ProviderEvent::Token {
                                        text: text.to_owned(),
                                    });
                                }
                                *last_text = text.to_owned();
                            }
                        }
                        Some("tool_use") => {
                            let id = block
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_owned();
                            let name = block
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool")
                                .to_owned();
                            let input = block
                                .get("input")
                                .cloned()
                                .unwrap_or(serde_json::Value::Null);
                            out.push(ProviderEvent::ObservedToolStart { id, name, input });
                        }
                        _ => {}
                    }
                }
            }
            false
        }

        Some("user") => {
            // Tool results fed back by the CLI; emit as ObservedToolEnd.
            let content = json
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array());

            if let Some(blocks) = content {
                for block in blocks {
                    if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                        let id = block
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_owned();
                        let is_error = block
                            .get("is_error")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let result = match block.get("content") {
                            Some(serde_json::Value::String(s)) => s.clone(),
                            Some(serde_json::Value::Array(arr)) => arr
                                .iter()
                                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join("\n"),
                            _ => String::new(),
                        };
                        out.push(ProviderEvent::ObservedToolEnd {
                            id,
                            result,
                            is_error,
                        });
                    }
                }
            }
            false
        }

        Some("result") => {
            // Emit token-usage accounting regardless of subtype.
            if let Some(usage) = json.get("usage") {
                let input = usage
                    .get("input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let output = usage
                    .get("output_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                let cache_read = usage
                    .get("cache_read_input_tokens")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32;
                out.push(ProviderEvent::Usage(TokenUsage {
                    input,
                    output,
                    cache_read,
                    cache_creation: 0,
                }));
            }

            let subtype = json.get("subtype").and_then(|v| v.as_str());
            if subtype == Some("error") {
                let msg = json
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                out.push(ProviderEvent::Error {
                    message: msg.to_owned(),
                });
            }

            // Log unexpected subtypes so auth failures, max-turns events, etc.
            // are visible in traces rather than silently ignored.
            if subtype != Some("success") {
                let is_error = json
                    .get("is_error")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                tracing::warn!(
                    subtype = subtype.unwrap_or("<missing>"),
                    is_error,
                    "claude-cli result with non-success subtype; turn continues"
                );
            }

            // Only `subtype == "success"` signals end-of-turn.  Other subtypes
            // (e.g. `"error_max_turns"`) mean the CLI will continue processing;
            // do NOT terminate the stream.
            subtype == Some("success")
        }

        _ => false,
    }
}

// ---------------------------------------------------------------------------
// ModelProvider impl
// ---------------------------------------------------------------------------

#[async_trait]
impl ModelProvider for ClaudeCliProvider {
    fn id(&self) -> &str {
        "claude-cli"
    }

    fn display_name(&self) -> &str {
        "Claude CLI"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![
            ModelInfo {
                id: ModelId::new("claude-cli:claude-sonnet-4-6"),
                display_name: "Claude Sonnet 4.6 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("claude-cli:claude-opus-4-7"),
                display_name: "Claude Opus 4.7 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("claude-cli:claude-sonnet-4-5"),
                display_name: "Claude Sonnet 4.5 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("claude-cli:claude-opus-4-5"),
                display_name: "Claude Opus 4.5 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
            ModelInfo {
                id: ModelId::new("claude-cli:claude-haiku-3-5"),
                display_name: "Claude Haiku 3.5 (CLI)".to_owned(),
                context_length: 200_000,
                supports_tools: false,
                supports_streaming: true,
            },
        ]
    }

    async fn complete(
        &self,
        mut request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let user_text = last_user_text(&request.messages).ok_or_else(|| ProviderError::Http {
            provider: "claude-cli".to_owned(),
            status: 0,
            body: "no user message found in request; cannot send empty turn to the CLI".to_owned(),
        })?;

        // Acquire an OWNED mutex guard (OwnedMutexGuard has 'static lifetime so
        // it can be moved into the spawned background task that will live for the
        // duration of the stream).
        let mut guard: OwnedMutexGuard<Option<CliProcess>> =
            Arc::clone(&self.process).lock_owned().await;

        // Capture the system prompt on the FIRST complete() call (first-turn-wins).
        //
        // The read-and-set-if-none + clone happen inside a single std::sync::Mutex
        // critical section AFTER the async process mutex is held.  Holding the
        // process mutex serialises concurrent complete() callers, so two first-turn
        // calls cannot both observe an empty slot — the second one sees the value
        // the first wrote.  The std::sync::Mutex guard is released before any
        // .await below.
        let captured_system_prompt: Option<String> = {
            let mut sp_guard = self.system_prompt.lock().unwrap_or_else(|e| e.into_inner());
            if sp_guard.is_none() {
                *sp_guard = extract_system_prompt(&request.messages);
            }
            sp_guard.clone()
        }; // std::sync::Mutex guard released here, before any .await

        // Snapshot the MCP config path (read-only after construction — no mutex needed).
        let mcp_config_path: Option<PathBuf> = self.mcp_config_path.clone();
        // Copy the strict-mcp-config bool (Copy type — no clone needed).
        let strict_mcp_config: bool = self.strict_mcp_config;
        // Clone the binary path for re-spawn inside the retry loop.
        let binary = self.binary.clone();
        let permission_mode = self.permission_mode.clone();

        // ---------------------------------------------------------------------------
        // Write + first-line phase: attempt the turn with up to 2 spawn attempts.
        //
        // For each attempt we:
        //   1. Write the NDJSON user turn to stdin.
        //   2. Eagerly read the FIRST stdout line.
        //
        // Reading the first line serves two purposes:
        //   a. It confirms the process is alive and responding.
        //   b. When the process was alive at write time but already exiting (e.g.
        //      one-shot stub), the first read gets EOF, letting us tombstone and
        //      retry transparently — exactly as the original eager implementation
        //      did.
        //
        // The first line is buffered into `pre_buffered_events` and emitted as the
        // first item(s) of the returned stream.  All subsequent lines are read
        // lazily by a background task.
        //
        // If both attempts fail (write or first-read error), return Err immediately.
        // ---------------------------------------------------------------------------
        const MAX_SPAWN_ATTEMPTS: usize = 2;
        let mut last_err: Option<String> = None;
        // Events extracted from the first stdout line (pre-buffered for the stream).
        let mut pre_buffered_events: Vec<ProviderEvent> = Vec::new();
        let mut last_text = String::new();
        // True when the first line was itself a done signal (single-line turns).
        let mut first_line_done = false;
        let mut write_and_first_read_succeeded = false;

        for attempt in 0..MAX_SPAWN_ATTEMPTS {
            pre_buffered_events.clear();
            last_text.clear();
            first_line_done = false;

            // Spawn (or re-spawn after death) if the slot is empty.
            if guard.is_none() {
                match spawn_process_now(
                    &binary,
                    &permission_mode,
                    captured_system_prompt.as_deref(),
                    mcp_config_path.as_deref(),
                    strict_mcp_config,
                ) {
                    Some(new_proc) => {
                        *guard = Some(new_proc);
                    }
                    None => {
                        return Err(ProviderError::Http {
                            provider: "claude-cli".to_owned(),
                            status: 0,
                            body: format!(
                                "failed to re-spawn claude process after death — binary '{binary}'"
                            ),
                        });
                    }
                }
            }

            // Safety: the re-spawn block above guarantees the slot is Some here.
            let mut process_died = false;

            if let Some(proc) = guard.as_mut() {
                let ndjson_line = build_ndjson_line(&proc.session_id, &user_text);
                let line_bytes = format!("{ndjson_line}\n");

                match proc.stdin.write_all(line_bytes.as_bytes()).await {
                    Err(e) => {
                        // Write failed — stdin pipe is broken (process exited).
                        tracing::debug!(
                            attempt,
                            "claude-cli write failed ({}); tombstoning and re-spawning",
                            e
                        );
                        let stderr_snap = snapshot_stderr(&proc.stderr_buf);
                        last_err = Some(build_process_died_msg(
                            &format!("write to claude stdin failed: {e}"),
                            stderr_snap.as_deref(),
                        ));
                        process_died = true;
                    }
                    Ok(()) => {
                        // Write succeeded — eagerly read the first stdout line to
                        // verify the process is responding before returning the
                        // stream.  If the process died after the write (e.g.
                        // one-shot stub that exited after the previous turn), we
                        // detect it here and retry with a fresh spawn.
                        'first_line: loop {
                            match proc.stdout_lines.next_line().await {
                                Ok(Some(line)) => {
                                    let trimmed = line.trim().to_owned();
                                    if trimmed.is_empty() {
                                        continue 'first_line;
                                    }
                                    // Parse and buffer events from this first line.
                                    let done = parse_line(
                                        &trimmed,
                                        &mut pre_buffered_events,
                                        &mut last_text,
                                    );
                                    if done {
                                        // The entire turn fit in one line (unusual but
                                        // valid).  Mark done so we skip the lazy task.
                                        first_line_done = true;
                                    }
                                    write_and_first_read_succeeded = true;
                                    break 'first_line;
                                }
                                Ok(None) => {
                                    // Immediate EOF — the process exited after the
                                    // write but before responding.  Yield briefly for
                                    // stderr drain, then tombstone and retry.
                                    for _ in 0..8 {
                                        tokio::task::yield_now().await;
                                    }
                                    let stderr_snap = snapshot_stderr(&proc.stderr_buf);
                                    last_err = Some(build_process_died_msg(
                                        "claude process exited unexpectedly",
                                        stderr_snap.as_deref(),
                                    ));
                                    process_died = true;
                                    break 'first_line;
                                }
                                Err(e) => {
                                    let stderr_snap = snapshot_stderr(&proc.stderr_buf);
                                    last_err = Some(build_process_died_msg(
                                        &format!("error reading claude output: {e}"),
                                        stderr_snap.as_deref(),
                                    ));
                                    process_died = true;
                                    break 'first_line;
                                }
                            }
                        }
                    }
                }
            }

            if process_died {
                *guard = None;
                continue;
            }

            // Either the write+first-read succeeded or we are past MAX_SPAWN_ATTEMPTS.
            break;
        }

        if !write_and_first_read_succeeded {
            let msg = last_err.unwrap_or_else(|| "unknown failure".to_owned());
            return Err(ProviderError::Http {
                provider: "claude-cli".to_owned(),
                status: 0,
                body: format!(
                    "claude-cli persistent process died and re-spawn could not \
                     produce a successful turn: {msg}"
                ),
            });
        }

        // ---------------------------------------------------------------------------
        // Build the lazy stream.
        //
        // If the first line already contained the success marker (`first_line_done`),
        // the stream is fully pre-buffered and no background task is needed.
        //
        // Otherwise, a background task is spawned that:
        //   - Owns the OwnedMutexGuard (holds the process lock for the stream
        //     lifetime, preventing interleaved writes from concurrent calls).
        //   - Reads stdout lines lazily and forwards events through an mpsc channel
        //     with capacity 1 (back-pressure ensures the task cannot complete the
        //     entire turn before the consumer has a chance to drop mid-turn).
        //   - On natural completion (Done emitted): releases the guard without
        //     tombstoning, leaving the process alive for the next turn.
        //   - On consumer drop (receiver closed, detected via Err from sender.send):
        //     tombstones the process so the next complete() re-spawns into a clean
        //     state (the child still has unread stdout bytes from the abandoned turn).
        // ---------------------------------------------------------------------------
        if first_line_done {
            // Entire turn fit in the first line — no background task needed.
            pre_buffered_events.push(ProviderEvent::Done);
            // Guard drops here, releasing the lock cleanly.
            return Ok(Box::pin(stream::iter(pre_buffered_events)));
        }

        // Take the cancel token (if any) out of the request before moving things
        // into the task.  When None, the no-cancel code path in the task runs
        // with zero overhead.
        let cancel: Option<CancellationToken> = request.cancel.take();

        let (tx, rx) = mpsc::channel::<ProviderEvent>(1);

        // Pre-seed the channel with events from the first line, but only if there
        // are any (e.g. an assistant text token).  We skip this if the first line
        // produced no events (shouldn't happen for valid NDJSON, but be defensive).
        //
        // We do NOT send these through the channel here because the task has not
        // started yet.  Instead we pass `pre_buffered_events` into the task so it
        // sends them first, then continues reading.  This avoids a synchronous send
        // before the task is live and keeps all event ordering inside the task.

        tokio::spawn(async move {
            let mut guard = guard;

            // Send pre-buffered events from the first line (already read eagerly).
            for event in pre_buffered_events {
                if tx.send(event).await.is_err() {
                    // Consumer dropped even before receiving the first event.
                    // Tombstone so the next complete() re-spawns cleanly.
                    *guard = None;
                    return;
                }
            }

            // Helper enum used only when a cancel token is present.
            // Defined inside the async block to keep it local.
            enum ReadOutcome {
                Cancelled,
                Line(std::io::Result<Option<String>>),
            }

            // Lazily read remaining stdout lines.
            // When cancel is Some, each read races against the token.
            // When cancel is None, the original single-future await is used
            // with zero select!/waker overhead.
            #[allow(clippy::while_let_loop)]
            loop {
                // Build the read future.  We must re-borrow `guard` as mut each
                // iteration; the borrow is released before we branch on the result.
                let outcome: ReadOutcome = match &cancel {
                    None => {
                        // No cancellation token — zero-overhead path, identical to
                        // the original implementation.
                        let line_result = if let Some(proc) = guard.as_mut() {
                            proc.stdout_lines.next_line().await
                        } else {
                            break;
                        };
                        ReadOutcome::Line(line_result)
                    }
                    Some(token) => {
                        // Cancellation token present — race the read against it.
                        // No `biased`: tokio's default random arm selection means
                        // when both are ready simultaneously, a buffered line may
                        // still be delivered.  The next loop iteration sees the
                        // token cancelled and exits cleanly.
                        if guard.is_none() {
                            break;
                        }
                        tokio::select! {
                            _ = token.cancelled() => ReadOutcome::Cancelled,
                            line = async {
                                // Re-borrow inside the select! arm.
                                if let Some(proc) = guard.as_mut() {
                                    proc.stdout_lines.next_line().await
                                } else {
                                    Ok(None)
                                }
                            } => ReadOutcome::Line(line),
                        }
                    }
                };

                match outcome {
                    ReadOutcome::Cancelled => {
                        // Token was cancelled: snapshot stderr, emit Error + Done,
                        // tombstone the process, and return.
                        let stderr_snap =
                            guard.as_ref().and_then(|p| snapshot_stderr(&p.stderr_buf));
                        let stderr_suffix = match stderr_snap.as_deref() {
                            Some(s) if !s.is_empty() => format!("; stderr: {s}"),
                            _ => String::new(),
                        };
                        let msg = format!("turn cancelled via CancellationToken{stderr_suffix}");
                        tracing::debug!(
                            "claude-cli turn cancelled via CancellationToken; \
                             emitting Error and tombstoning"
                        );
                        let _ = tx.send(ProviderEvent::Error { message: msg }).await;
                        let _ = tx.send(ProviderEvent::Done).await;
                        *guard = None;
                        return;
                    }

                    ReadOutcome::Line(Ok(Some(line))) => {
                        let trimmed = line.trim().to_owned();
                        if trimmed.is_empty() {
                            continue;
                        }

                        let mut events: Vec<ProviderEvent> = Vec::new();
                        let done = parse_line(&trimmed, &mut events, &mut last_text);

                        for event in events {
                            if tx.send(event).await.is_err() {
                                tracing::debug!(
                                    "claude-cli stream consumer dropped before Done; \
                                     tombstoning process"
                                );
                                *guard = None;
                                return;
                            }
                        }

                        if done {
                            // Natural completion: emit Done and release guard cleanly.
                            let _ = tx.send(ProviderEvent::Done).await;
                            return;
                        }
                    }

                    ReadOutcome::Line(Ok(None)) => {
                        // EOF during the lazy read phase — process died unexpectedly.
                        // Yield briefly so the stderr-drain task can flush any
                        // final lines before we snapshot the buffer.
                        for _ in 0..8 {
                            tokio::task::yield_now().await;
                        }
                        let stderr_snap =
                            guard.as_ref().and_then(|p| snapshot_stderr(&p.stderr_buf));
                        let msg = build_process_died_msg(
                            "claude process exited unexpectedly (mid-stream)",
                            stderr_snap.as_deref(),
                        );
                        tracing::debug!(
                            "claude-cli process exited unexpectedly (EOF on stdout during \
                             lazy read); emitting Error and tombstoning"
                        );
                        // Surface the death to the consumer so the agent loop
                        // doesn't mistake a truncated turn for natural completion.
                        // If the consumer already dropped the receiver, both sends
                        // fail silently — that's fine, they abandoned the stream.
                        let _ = tx.send(ProviderEvent::Error { message: msg }).await;
                        let _ = tx.send(ProviderEvent::Done).await;
                        *guard = None;
                        return;
                    }

                    ReadOutcome::Line(Err(e)) => {
                        let stderr_snap =
                            guard.as_ref().and_then(|p| snapshot_stderr(&p.stderr_buf));
                        let msg = build_process_died_msg(
                            &format!("claude stdout read error mid-stream: {e}"),
                            stderr_snap.as_deref(),
                        );
                        tracing::debug!(
                            "claude-cli stdout read error during lazy read: {}; emitting \
                             Error and tombstoning",
                            e
                        );
                        let _ = tx.send(ProviderEvent::Error { message: msg }).await;
                        let _ = tx.send(ProviderEvent::Done).await;
                        *guard = None;
                        return;
                    }
                }
            }
        });

        let event_stream =
            stream::unfold(
                rx,
                |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
            );

        Ok(Box::pin(event_stream))
    }

    fn estimate_tokens(&self, messages: &[Message]) -> u32 {
        estimate_tokens_heuristic(messages)
    }

    async fn health(&self) -> ProviderHealth {
        check_binary_available(&self.binary).await
    }
}
