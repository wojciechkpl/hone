use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Per-process monotonic counter used to disambiguate concurrent mock-agent
/// directory creation.  Combined with `subsec_nanos()` and the process id
/// it makes the unique-dir suffix robust against parallel-test races where
/// two `create_*` calls would otherwise hit the same nanosecond value.
static MOCK_AGENT_SEQ: AtomicU64 = AtomicU64::new(0);

fn unique_suffix() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let seq = MOCK_AGENT_SEQ.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    format!("{pid}_{nanos}_{seq}")
}

/// Configuration for the mock ACP agent script.
pub struct MockAgentConfig {
    /// The `agentInfo.name` returned in the `initialize` response.
    pub agent_name: String,
    /// Text chunks emitted as `agent_message_chunk` notifications in response
    /// to a `session/prompt` request.
    pub message_chunks: Vec<String>,
}

/// Write a bash mock-ACP-agent script to a unique temporary directory.
///
/// Returns the temporary directory path (caller must keep it alive for the
/// duration of the test — drop it when done) and the path to the script.
///
/// Uses `std::env::temp_dir()` so no extra crates are needed.
pub fn create_mock_agent_script(config: &MockAgentConfig) -> Result<(PathBuf, PathBuf), io::Error> {
    // Create a unique directory under the system temp dir.
    let dir = std::env::temp_dir().join(format!("hone_mock_acp_{}", unique_suffix()));
    std::fs::create_dir_all(&dir)?;

    let script_path = dir.join("mock_acp_agent.sh");

    // Build the session/update lines for each message chunk.
    let chunk_lines: String = config
        .message_chunks
        .iter()
        .map(|chunk| {
            let json_text =
                serde_json::to_string(chunk).unwrap_or_else(|_| format!("\"{}\"", chunk));
            format!(
                "            printf '{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"$SESSION_ID\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":{json_text}}}}}}}}}\\n'"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let agent_name = shell_single_quote_escape(&config.agent_name);

    let script = format!(
        "#!/bin/sh\n\
         SESSION_ID=\"sess_mock_$$\"\n\
         \n\
         while IFS= read -r line; do\n\
             method=$(printf '%s' \"$line\" | sed 's/.*\"method\":\"\\([^\"]*\\)\".*/\\1/')\n\
             id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\).*/\\1/p')\n\
         \n\
             case \"$method\" in\n\
                 initialize)\n\
                     printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"protocolVersion\":1,\"agentInfo\":{{\"name\":\"{agent_name}\",\"version\":\"0.1.0\"}}}}}}\\n' \"$id\"\n\
                     ;;\n\
                 session/new)\n\
                     printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"sessionId\":\"%s\"}}}}\\n' \"$id\" \"$SESSION_ID\"\n\
                     ;;\n\
                 session/prompt)\n\
{chunk_lines}\n\
                     printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"stopReason\":\"end_turn\"}}}}\\n' \"$id\"\n\
                     ;;\n\
                 session/cancel)\n\
                     ;;\n\
                 *)\n\
                     if [ -n \"$id\" ]; then\n\
                         printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{{\"code\":-32601,\"message\":\"Method not found\"}}}}\\n' \"$id\"\n\
                     fi\n\
                     ;;\n\
             esac\n\
         done\n",
    );

    std::fs::write(&script_path, &script)?;

    let mut perms = std::fs::metadata(&script_path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms)?;

    Ok((dir, script_path))
}

/// Write a bash mock-ACP-agent script that additionally appends every incoming
/// JSON-RPC line to `requests_log_path`.
///
/// This is the test-infrastructure variant used by Cycle 1 tool-schema tests.
/// The response behaviour is identical to [`create_mock_agent_script`]; only
/// the receive-side logging is added.  `requests_log_path` **must not** exist
/// when the script starts — the script appends to it, so the caller should
/// point it at a file inside a `tempfile::TempDir`.
pub fn create_mock_agent_script_with_log(
    config: &MockAgentConfig,
    requests_log_path: &std::path::Path,
) -> Result<(PathBuf, PathBuf), io::Error> {
    let dir = std::env::temp_dir().join(format!("hone_mock_acp_log_{}", unique_suffix()));
    std::fs::create_dir_all(&dir)?;

    let script_path = dir.join("mock_acp_agent_log.sh");

    let chunk_lines: String = config
        .message_chunks
        .iter()
        .map(|chunk| {
            let json_text =
                serde_json::to_string(chunk).unwrap_or_else(|_| format!("\"{}\"", chunk));
            format!(
                "            printf '{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"$SESSION_ID\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":{json_text}}}}}}}}}\\n'"
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let agent_name = shell_single_quote_escape(&config.agent_name);
    let log_path_escaped = shell_single_quote_escape(&requests_log_path.to_string_lossy());

    let script = format!(
        "#!/bin/sh\n\
         SESSION_ID=\"sess_mock_$$\"\n\
         \n\
         while IFS= read -r line; do\n\
             printf '%s\\n' \"$line\" >> '{log_path_escaped}'\n\
             method=$(printf '%s' \"$line\" | sed 's/.*\"method\":\"\\([^\"]*\\)\".*/\\1/')\n\
             id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\).*/\\1/p')\n\
         \n\
             case \"$method\" in\n\
                 initialize)\n\
                     printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"protocolVersion\":1,\"agentInfo\":{{\"name\":\"{agent_name}\",\"version\":\"0.1.0\"}}}}}}\\n' \"$id\"\n\
                     ;;\n\
                 session/new)\n\
                     printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"sessionId\":\"%s\"}}}}\\n' \"$id\" \"$SESSION_ID\"\n\
                     ;;\n\
                 session/prompt)\n\
{chunk_lines}\n\
                     printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"stopReason\":\"end_turn\"}}}}\\n' \"$id\"\n\
                     ;;\n\
                 session/cancel)\n\
                     ;;\n\
                 *)\n\
                     if [ -n \"$id\" ]; then\n\
                         printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{{\"code\":-32601,\"message\":\"Method not found\"}}}}\\n' \"$id\"\n\
                     fi\n\
                     ;;\n\
             esac\n\
         done\n",
    );

    std::fs::write(&script_path, &script)?;

    let mut perms = std::fs::metadata(&script_path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms)?;

    Ok((dir, script_path))
}

/// Configuration for a mock ACP agent that emits a single tool call mid-prompt,
/// waits for the provider to send a `session/toolResult` request back, then
/// completes the turn.
///
/// This is the test-infra variant used by Cycle 2 tool-roundtrip tests.
#[derive(Clone)]
pub struct MockToolCallConfig {
    /// The `agentInfo.name` returned in the `initialize` response.
    pub agent_name: String,
    /// Text chunk emitted *before* the tool call notification.
    pub pre_tool_chunk: String,
    /// Text chunk emitted *after* the tool result is received.
    pub post_tool_chunk: String,
    /// The `toolCallId` embedded in the `tool_call` notification.
    pub tool_call_id: String,
    /// The `title` embedded in the `tool_call` notification.
    pub tool_call_title: String,
    /// Optional `rawInput` JSON to embed in the `tool_call` notification.
    /// When `None`, the field is omitted entirely from the wire payload —
    /// modelling a legacy / minimal agent that does not advertise arguments.
    pub raw_input: Option<serde_json::Value>,
    /// Optional terminal `status` string emitted as a `tool_call_update`
    /// notification *after* the provider has sent its `session/toolResult`
    /// and *before* the prompt response.  When `None`, no
    /// `tool_call_update` notification is sent (modelling a legacy agent).
    pub tool_call_update_status: Option<String>,
}

/// Write a bash mock-ACP-agent script that performs a bidirectional tool-call
/// round-trip:
///
/// 1. Responds to `session/prompt` by emitting a `session/update` chunk.
/// 2. Emits a `session/update` with `sessionUpdate: "tool_call"`.
/// 3. Reads stdin in a loop until a line containing `session/toolResult` is
///    received from the provider.  Logs every received line to `requests_log_path`.
/// 4. Emits a second `session/update` chunk.
/// 5. Sends the `session/prompt` response with `stopReason: "end_turn"`.
///
/// This is fully additive — it does not modify any existing function.
pub fn create_mock_acp_agent_with_tool_call(
    config: &MockToolCallConfig,
    requests_log_path: &std::path::Path,
) -> Result<(PathBuf, PathBuf), io::Error> {
    let dir = std::env::temp_dir().join(format!("hone_mock_acp_toolcall_{}", unique_suffix()));
    std::fs::create_dir_all(&dir)?;

    let script_path = dir.join("mock_acp_tool_call_agent.sh");

    let agent_name = shell_single_quote_escape(&config.agent_name);
    let log_path_escaped = shell_single_quote_escape(&requests_log_path.to_string_lossy());

    // Pre-encode all JSON-string values once, then inline them directly into
    // the printf format string.  Inlining (not passing as `%s` arguments)
    // matters because shell argument parsing strips the outer JSON quotes:
    // `printf '"x":%s' "\"foo\""` outputs `"x":foo` (invalid JSON), whereas
    // inlining the literal `\"foo\"` into the format string preserves the
    // quotes and yields `"x":"foo"`.
    let pre_chunk_json = serde_json::to_string(&config.pre_tool_chunk)
        .unwrap_or_else(|_| "\"thinking...\"".to_owned());
    let post_chunk_json =
        serde_json::to_string(&config.post_tool_chunk).unwrap_or_else(|_| "\"done\"".to_owned());
    let tool_call_id_json =
        serde_json::to_string(&config.tool_call_id).unwrap_or_else(|_| "\"t_1\"".to_owned());
    let tool_title_json =
        serde_json::to_string(&config.tool_call_title).unwrap_or_else(|_| "\"tool\"".to_owned());
    // Optional rawInput field — when set, splice in `,"rawInput":<json>` so the
    // resulting object has the field; when None, leave the splice empty so the
    // field is absent (modelling legacy agents).  This does NOT inject a key
    // when the value is null — the caller must use `Some(Value::Null)` to
    // explicitly send a null.
    let raw_input_splice = match &config.raw_input {
        Some(value) => {
            let encoded = serde_json::to_string(value).unwrap_or_else(|_| "{}".to_owned());
            format!(",\"rawInput\":{encoded}")
        }
        None => String::new(),
    };

    // When configured, build a complete `tool_call_update` notification line —
    // including the trailing `\n` and the explicit \"$SESSION_ID\" arg — that
    // gets spliced into the script verbatim.  We pre-encode the status as a
    // JSON string (`\"completed\"`) so the printf format string contains a
    // literal quoted token.
    let tool_call_update_line = match &config.tool_call_update_status {
        Some(status) => {
            let status_json =
                serde_json::to_string(status).unwrap_or_else(|_| "\"completed\"".to_owned());
            format!(
                "             # (d.5) tool_call_update notification — terminal status\n\
                 \x20            printf '{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"%s\",\"update\":{{\"sessionUpdate\":\"tool_call_update\",\"toolCallId\":{tool_call_id_json},\"status\":{status_json}}}}}}}\\n' \"$SESSION_ID\"\n"
            )
        }
        None => String::new(),
    };

    // The script handles all four ACP methods.  During session/prompt it:
    //   a) emits the pre-tool chunk notification
    //   b) emits the tool_call notification
    //   c) enters a read loop waiting for a line that contains "session/toolResult"
    //      (all lines received while waiting are logged to the requests log file)
    //   d) emits the post-tool chunk notification
    //   e) sends the session/prompt response
    //
    // All incoming JSON-RPC lines (initialize, session/new, session/prompt, and
    // the interleaved session/toolResult) are appended to requests_log_path so
    // tests can assert on what the provider sent.
    let script = format!(
        "#!/bin/sh\n\
         SESSION_ID=\"sess_mock_tool_$$\"\n\
         PROMPT_ID=\"\"\n\
         \n\
         handle_prompt() {{\n\
             # (a) pre-tool chunk — JSON values are inlined into the format string\n\
             printf '{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"%s\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":{pre_chunk_json}}}}}}}}}\\n' \"$SESSION_ID\"\n\
             # (b) tool_call notification — id, title, and optional rawInput inlined as JSON\n\
             printf '{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"%s\",\"update\":{{\"sessionUpdate\":\"tool_call\",\"toolCallId\":{tool_call_id_json},\"title\":{tool_title_json},\"kind\":\"read\",\"status\":\"in_progress\"{raw_input_splice}}}}}}}\\n' \"$SESSION_ID\"\n\
             # (c) wait for session/toolResult from the provider\n\
             while IFS= read -r incoming; do\n\
                 printf '%s\\n' \"$incoming\" >> '{log_path_escaped}'\n\
                 case \"$incoming\" in\n\
                     *session/toolResult*)\n\
                         break\n\
                         ;;\n\
                 esac\n\
             done\n\
             # (d) post-tool chunk\n\
             printf '{{\"jsonrpc\":\"2.0\",\"method\":\"session/update\",\"params\":{{\"sessionId\":\"%s\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":{post_chunk_json}}}}}}}}}\\n' \"$SESSION_ID\"\n\
{tool_call_update_line}\
             # (e) prompt response\n\
             printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"stopReason\":\"end_turn\"}}}}\\n' \"$PROMPT_ID\"\n\
         }}\n\
         \n\
         while IFS= read -r line; do\n\
             printf '%s\\n' \"$line\" >> '{log_path_escaped}'\n\
             method=$(printf '%s' \"$line\" | sed 's/.*\"method\":\"\\([^\"]*\\)\".*/\\1/')\n\
             id=$(printf '%s' \"$line\" | sed -n 's/.*\"id\":\\([0-9][0-9]*\\).*/\\1/p')\n\
         \n\
             case \"$method\" in\n\
                 initialize)\n\
                     printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"protocolVersion\":1,\"agentInfo\":{{\"name\":\"{agent_name}\",\"version\":\"0.1.0\"}}}}}}\\n' \"$id\"\n\
                     ;;\n\
                 session/new)\n\
                     printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"result\":{{\"sessionId\":\"%s\"}}}}\\n' \"$id\" \"$SESSION_ID\"\n\
                     ;;\n\
                 session/prompt)\n\
                     PROMPT_ID=\"$id\"\n\
                     handle_prompt\n\
                     ;;\n\
                 session/cancel)\n\
                     ;;\n\
                 *)\n\
                     if [ -n \"$id\" ]; then\n\
                         printf '{{\"jsonrpc\":\"2.0\",\"id\":%s,\"error\":{{\"code\":-32601,\"message\":\"Method not found\"}}}}\\n' \"$id\"\n\
                     fi\n\
                     ;;\n\
             esac\n\
         done\n",
    );

    std::fs::write(&script_path, &script)?;

    let mut perms = std::fs::metadata(&script_path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script_path, perms)?;

    Ok((dir, script_path))
}

/// Escape a string for safe embedding in a shell single-quoted context.
/// Single quotes cannot appear inside a single-quoted string in POSIX sh,
/// but for our purposes agent names are simple ASCII so no escaping is needed.
fn shell_single_quote_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}
