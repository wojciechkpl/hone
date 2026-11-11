use std::collections::HashMap;
use std::time::Duration;

use hone_config::mcp::{McpConfig, McpServerConfig};
use hone_tools::Tool;

use crate::error::McpError;
use crate::stdio_client::StdioMcpClient;
use crate::traits::McpClient;

/// Manages the lifecycle of all configured MCP server connections.
///
/// One `McpManager` is constructed at startup. It owns all live
/// [`StdioMcpClient`] instances and hands out tool wrappers that are then
/// inserted into `ToolRegistry`.
pub struct McpManager {
    /// Live clients, keyed by server name.
    clients: HashMap<String, StdioMcpClient>,
}

impl McpManager {
    /// Spawn and handshake all enabled servers from `config`, concurrently.
    ///
    /// Servers that fail to start are logged as warnings and skipped — one
    /// broken (or hung) MCP server must not prevent the agent from starting, and
    /// startup takes as long as the slowest server, not the sum. The connections
    /// are driven in the caller's task, not detached: dropping this future drops,
    /// and so kills, every client still connecting.
    pub async fn from_config(config: &McpConfig) -> Self {
        let connecting = config.mcp_servers.iter().filter_map(|(name, server_cfg)| {
            if server_cfg.disabled {
                tracing::info!(server = %name, "MCP server disabled, skipping");
                return None;
            }
            Some(async move { (name, Self::spawn_and_connect(name, server_cfg).await) })
        });

        let mut clients = HashMap::new();
        for (name, result) in futures::future::join_all(connecting).await {
            match result {
                Ok(client) => {
                    tracing::info!(
                        server = %name,
                        tools = client.server_info_tool_count(),
                        "MCP server connected"
                    );
                    clients.insert(name.clone(), client);
                }
                Err(e) => {
                    tracing::warn!(
                        server = %name,
                        error = %e,
                        "MCP server failed to start, skipping"
                    );
                }
            }
        }

        Self { clients }
    }

    /// Return all MCP tools across all live servers as native `Tool` objects.
    ///
    /// The returned vec is ready to be registered directly into
    /// `ToolRegistryBuilder`. Tools from different servers are interleaved in
    /// an unspecified order.
    pub fn tools_as_native(&self) -> Vec<Box<dyn Tool>> {
        self.clients
            .values()
            .flat_map(|client| client.tools_as_native())
            .collect()
    }

    /// Return the number of live (successfully connected) servers.
    pub fn server_count(&self) -> usize {
        self.clients.len()
    }

    /// Gracefully shut down all MCP server subprocesses, concurrently.
    ///
    /// Each server receives a `notifications/cancelled` notification and is
    /// given 5 seconds to exit before a hard kill (the budgets run side by side,
    /// so the whole shutdown takes at most about 5 seconds).  Shutdown errors
    /// are logged but never propagated — callers can drop `McpManager`
    /// unconditionally.
    pub async fn shutdown(mut self) {
        let stopping = self.clients.drain().map(|(name, mut client)| async move {
            if let Err(e) = client.shutdown().await {
                tracing::warn!(server = %name, error = %e, "MCP server shutdown error");
            }
        });
        futures::future::join_all(stopping).await;
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    async fn spawn_and_connect(
        name: &str,
        cfg: &McpServerConfig,
    ) -> Result<StdioMcpClient, McpError> {
        // Resolve ${VAR} references in env values before passing to the subprocess.
        let resolved_env: Vec<(String, String)> = cfg
            .env
            .iter()
            .map(|(k, v)| (k.clone(), resolve_env_var(v)))
            .collect();

        let args: Vec<&str> = cfg.args.iter().map(String::as_str).collect();

        // `timeout_ms` bounds every request, the handshake included
        // (0 = no timeout).
        let mut client = StdioMcpClient::spawn_with_name(name, &cfg.command, &args, &resolved_env)
            .await?
            .with_request_timeout(Duration::from_millis(cfg.timeout_ms));

        if let Some(ms) = cfg.handshake_timeout_ms {
            client = client.with_handshake_timeout(Duration::from_millis(ms));
        }

        if let Err(e) = client.connect().await {
            // Do not leave a hung (or broken) server running.
            client.kill().await;
            return Err(e);
        }
        Ok(client)
    }
}

/// Substitute `${VAR_NAME}` patterns in `value` with the current environment.
///
/// Unknown variables are replaced with an empty string.  Malformed patterns
/// (no closing `}`) are left unchanged.
fn resolve_env_var(value: &str) -> String {
    let mut result = value.to_string();
    let mut start = 0;

    while let Some(open_rel) = result[start..].find("${") {
        let open = open_rel + start;
        if let Some(close_rel) = result[open..].find('}') {
            let close = close_rel + open;
            let var_name = &result[open + 2..close];
            let replacement = std::env::var(var_name).unwrap_or_default();
            result.replace_range(open..=close, &replacement);
            start = open + replacement.len();
        } else {
            // No closing brace — leave the rest of the string as-is.
            break;
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_env_var_no_placeholders() {
        assert_eq!(resolve_env_var("plain-value"), "plain-value");
    }

    #[test]
    fn resolve_env_var_known_variable() {
        // SAFETY: tests run single-threaded by default; mutating env here is safe.
        unsafe {
            std::env::set_var("_HONE_MCP_TEST_VAR", "hello");
        }
        assert_eq!(resolve_env_var("${_HONE_MCP_TEST_VAR}"), "hello");
        // SAFETY: same as above.
        unsafe {
            std::env::remove_var("_HONE_MCP_TEST_VAR");
        }
    }

    #[test]
    fn resolve_env_var_unknown_becomes_empty() {
        // Ensure the variable is definitely absent.
        // SAFETY: tests run single-threaded by default; mutating env here is safe.
        unsafe {
            std::env::remove_var("_HONE_MCP_DEFINITELY_ABSENT");
        }
        assert_eq!(
            resolve_env_var("prefix-${_HONE_MCP_DEFINITELY_ABSENT}-suffix"),
            "prefix--suffix"
        );
    }

    #[test]
    fn resolve_env_var_multiple_placeholders() {
        // SAFETY: tests run single-threaded by default; mutating env here is safe.
        unsafe {
            std::env::set_var("_HONE_MCP_A", "foo");
            std::env::set_var("_HONE_MCP_B", "bar");
        }
        assert_eq!(resolve_env_var("${_HONE_MCP_A}/${_HONE_MCP_B}"), "foo/bar");
        // SAFETY: same as above.
        unsafe {
            std::env::remove_var("_HONE_MCP_A");
            std::env::remove_var("_HONE_MCP_B");
        }
    }

    #[test]
    fn resolve_env_var_unclosed_brace_is_left_as_is() {
        // A `${` with no closing `}` must not panic or loop.
        assert_eq!(resolve_env_var("${UNCLOSED"), "${UNCLOSED");
    }
}
