use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

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
pub fn create_mock_agent_script(
    config: &MockAgentConfig,
) -> Result<(PathBuf, PathBuf), io::Error> {
    // Create a unique directory under the system temp dir.
    let unique = format!(
        "hone_mock_acp_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    );
    let dir = std::env::temp_dir().join(unique);
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

/// Escape a string for safe embedding in a shell single-quoted context.
/// Single quotes cannot appear inside a single-quoted string in POSIX sh,
/// but for our purposes agent names are simple ASCII so no escaping is needed.
fn shell_single_quote_escape(s: &str) -> String {
    s.replace('\'', "'\\''")
}
