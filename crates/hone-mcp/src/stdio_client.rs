use std::sync::Arc;

use async_trait::async_trait;
use hone_tools::Tool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStdin, ChildStdout};
use tokio::sync::Mutex;

use crate::error::McpError;
use crate::tool_wrapper::McpTool;
use crate::traits::{McpClient, McpServerInfo, McpToolDef};

// ---------------------------------------------------------------------------
// JSON-RPC wire types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct JsonRpcRequest {
    jsonrpc: &'static str,
    id: u64,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

/// A JSON-RPC notification — no `id`, no response expected.
#[derive(Serialize)]
struct JsonRpcNotification {
    jsonrpc: &'static str,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    params: Option<Value>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcResponse {
    #[allow(dead_code)]
    id: Option<u64>,
    result: Option<Value>,
    error: Option<JsonRpcErrorObj>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcErrorObj {
    code: i64,
    message: String,
}

// ---------------------------------------------------------------------------
// Inner mutable transport state
// ---------------------------------------------------------------------------

struct TransportInner {
    /// Keep the child alive so it is not reaped while we hold its pipes.
    #[allow(dead_code)]
    process: Child,
    stdin: BufWriter<ChildStdin>,
    reader: BufReader<ChildStdout>,
    next_id: u64,
}

impl TransportInner {
    async fn send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
        server_name: &str,
    ) -> Result<Value, McpError> {
        let id = self.next_id;
        self.next_id += 1;

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&req)?;
        line.push('\n');

        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.to_string(),
                source: e,
            })?;
        self.stdin
            .flush()
            .await
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.to_string(),
                source: e,
            })?;

        // Read lines until we get a non-empty one (skip blank lines / log output).
        loop {
            let mut response_line = String::new();
            let bytes = self
                .reader
                .read_line(&mut response_line)
                .await
                .map_err(|e| McpError::ConnectionFailed {
                    server: server_name.to_string(),
                    source: e,
                })?;

            if bytes == 0 {
                return Err(McpError::ProtocolError {
                    server: server_name.to_string(),
                    message: "server closed stdout unexpectedly".to_string(),
                });
            }

            let trimmed = response_line.trim();
            if trimmed.is_empty() {
                continue;
            }

            let resp: JsonRpcResponse = serde_json::from_str(trimmed)?;

            if let Some(err) = resp.error {
                return Err(McpError::JsonRpc {
                    code: err.code,
                    message: err.message,
                });
            }

            return resp.result.ok_or_else(|| McpError::ProtocolError {
                server: server_name.to_string(),
                message: "JSON-RPC response had neither result nor error".to_string(),
            });
        }
    }

    async fn send_notification(
        &mut self,
        method: &str,
        params: Option<Value>,
        server_name: &str,
    ) -> Result<(), McpError> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0",
            method: method.to_string(),
            params,
        };

        let mut line = serde_json::to_string(&notif)?;
        line.push('\n');

        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.to_string(),
                source: e,
            })?;
        self.stdin
            .flush()
            .await
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.to_string(),
                source: e,
            })?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Public client
// ---------------------------------------------------------------------------

/// A real MCP client that communicates with an MCP server subprocess over stdio (JSON-RPC 2.0).
///
/// # Usage
///
/// ```no_run
/// # async fn example() -> Result<(), hone_mcp::McpError> {
/// use hone_mcp::{McpClient, StdioMcpClient};
///
/// let mut client = StdioMcpClient::spawn("my-mcp-server", &[]).await?;
/// let info = client.connect().await?;
/// let tools = client.list_tools().await?;
/// # Ok(())
/// # }
/// ```
pub struct StdioMcpClient {
    server_name: String,
    /// Shared so that `tools_as_native` can hand the same transport to `McpTool` wrappers.
    transport: Arc<Mutex<TransportInner>>,
    /// Populated by `connect`. Guarded for shared-reference access.
    server_info: Mutex<Option<McpServerInfo>>,
}

impl StdioMcpClient {
    /// Spawn an MCP server subprocess and return a client ready for [`McpClient::connect`].
    pub async fn spawn(command: &str, args: &[&str]) -> Result<Self, McpError> {
        let server_name = command.to_string();

        let mut child = tokio::process::Command::new(command)
            .args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.clone(),
                source: e,
            })?;

        // Safe: Command was configured with .stdin/.stdout(Stdio::piped) above.
        #[allow(clippy::expect_used)]
        let stdin = child.stdin.take().expect("stdin piped");
        #[allow(clippy::expect_used)]
        let stdout = child.stdout.take().expect("stdout piped");

        Ok(Self {
            server_name,
            transport: Arc::new(Mutex::new(TransportInner {
                process: child,
                stdin: BufWriter::new(stdin),
                reader: BufReader::new(stdout),
                next_id: 1,
            })),
            server_info: Mutex::new(None),
        })
    }

    /// Spawn with an explicit logical name and additional environment variables.
    ///
    /// The subprocess environment is scrubbed before launch — only `PATH`, `HOME`,
    /// and the caller-supplied `extra_env` pairs are present.  This prevents
    /// accidental inheritance of `ANTHROPIC_API_KEY` and similar credentials.
    pub async fn spawn_with_name(
        server_name: &str,
        command: &str,
        args: &[&str],
        extra_env: &[(String, String)],
    ) -> Result<Self, McpError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        // Security: clear the inherited environment so API keys are not leaked
        // to untrusted MCP subprocesses.  Re-inject only the minimum needed for
        // the subprocess to locate its own runtime dependencies.
        cmd.env_clear();
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        if let Ok(home) = std::env::var("HOME") {
            cmd.env("HOME", home);
        }
        // Apply the server-specific env on top of the baseline.
        for (k, v) in extra_env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| McpError::ConnectionFailed {
            server: server_name.to_string(),
            source: e,
        })?;

        // Safe: Command was configured with .stdin/.stdout(Stdio::piped) above.
        #[allow(clippy::expect_used)]
        let stdin = child.stdin.take().expect("stdin piped");
        #[allow(clippy::expect_used)]
        let stdout = child.stdout.take().expect("stdout piped");

        Ok(Self {
            server_name: server_name.to_string(),
            transport: Arc::new(Mutex::new(TransportInner {
                process: child,
                stdin: BufWriter::new(stdin),
                reader: BufReader::new(stdout),
                next_id: 1,
            })),
            server_info: Mutex::new(None),
        })
    }

    /// Gracefully stop the MCP server subprocess.
    ///
    /// Sends a `notifications/cancelled` notification and then waits up to 5
    /// seconds for the process to exit cleanly before issuing a hard kill.
    pub async fn shutdown(&mut self) -> Result<(), McpError> {
        // Best-effort notification — the server may already be gone.
        let _ = self
            .transport
            .lock()
            .await
            .send_notification("notifications/cancelled", None, &self.server_name)
            .await;

        let kill_timeout = tokio::time::Duration::from_secs(5);
        let mut transport = self.transport.lock().await;
        match tokio::time::timeout(kill_timeout, transport.process.wait()).await {
            Ok(_) => {}
            Err(_) => {
                // Timed out — forcibly kill the subprocess.
                let _ = transport.process.kill().await;
            }
        }
        Ok(())
    }

    /// Return the number of tools reported by the server during `connect()`.
    ///
    /// Returns `0` when `connect()` has not yet been called or the lock is
    /// currently held by another task.
    pub fn server_info_tool_count(&self) -> usize {
        self.server_info
            .try_lock()
            .ok()
            .and_then(|g| g.as_ref().map(|i| i.tools.len()))
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// McpClient implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl McpClient for StdioMcpClient {
    /// Perform the MCP initialization handshake and populate the tool cache.
    async fn connect(&mut self) -> Result<McpServerInfo, McpError> {
        let init_params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "hone",
                "version": "0.1.0"
            }
        });

        let mut transport = self.transport.lock().await;

        let init_result = transport
            .send_request("initialize", Some(init_params), &self.server_name)
            .await?;

        let name = init_result
            .get("serverInfo")
            .and_then(|si| si.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or(&self.server_name)
            .to_string();

        let version = init_result
            .get("serverInfo")
            .and_then(|si| si.get("version"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        transport
            .send_notification("notifications/initialized", None, &self.server_name)
            .await?;

        let tools_result = transport
            .send_request("tools/list", None, &self.server_name)
            .await?;

        drop(transport);

        let tools = parse_tools_list(&tools_result, &self.server_name)?;

        let info = McpServerInfo {
            name,
            version,
            tools,
        };

        *self.server_info.lock().await = Some(info.clone());
        Ok(info)
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDef>, McpError> {
        // Serve from cache when connect() has already been called.
        if let Some(info) = self.server_info.lock().await.as_ref() {
            return Ok(info.tools.clone());
        }

        // Fall back to a live query.
        let result = self
            .transport
            .lock()
            .await
            .send_request("tools/list", None, &self.server_name)
            .await?;

        parse_tools_list(&result, &self.server_name)
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<Value, McpError> {
        let params = serde_json::json!({
            "name": name,
            "arguments": args,
        });

        self.transport
            .lock()
            .await
            .send_request("tools/call", Some(params), &self.server_name)
            .await
    }

    fn tools_as_native(&self) -> Vec<Box<dyn Tool>> {
        let guard = match self.server_info.try_lock() {
            Ok(g) => g,
            Err(_) => return vec![],
        };

        let tools = match guard.as_ref() {
            Some(info) => info.tools.clone(),
            None => return vec![],
        };
        drop(guard);

        // Build one shared forwarding client that references the same transport Arc.
        let forwarding: Arc<dyn McpClient> = Arc::new(TransportForwardingClient {
            server_name: self.server_name.clone(),
            transport: Arc::clone(&self.transport),
            tools: tools.clone(),
        });

        tools
            .iter()
            .map(|def| -> Box<dyn Tool> {
                Box::new(McpTool::new(
                    self.server_name.clone(),
                    def.clone(),
                    Arc::clone(&forwarding),
                ))
            })
            .collect()
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }
}

// ---------------------------------------------------------------------------
// Forwarding client — shares the same transport Arc with StdioMcpClient
// ---------------------------------------------------------------------------

/// Internal shim that lets [`McpTool`] dispatch `call_tool` through the same
/// [`TransportInner`] that the parent [`StdioMcpClient`] owns.
struct TransportForwardingClient {
    server_name: String,
    transport: Arc<Mutex<TransportInner>>,
    tools: Vec<McpToolDef>,
}

#[async_trait]
impl McpClient for TransportForwardingClient {
    async fn connect(&mut self) -> Result<McpServerInfo, McpError> {
        // Not used by McpTool; the parent already called connect().
        Err(McpError::ProtocolError {
            server: self.server_name.clone(),
            message: "connect() not available on forwarding client".to_string(),
        })
    }

    async fn list_tools(&self) -> Result<Vec<McpToolDef>, McpError> {
        Ok(self.tools.clone())
    }

    async fn call_tool(&self, name: &str, args: Value) -> Result<Value, McpError> {
        let params = serde_json::json!({
            "name": name,
            "arguments": args,
        });

        self.transport
            .lock()
            .await
            .send_request("tools/call", Some(params), &self.server_name)
            .await
    }

    fn tools_as_native(&self) -> Vec<Box<dyn Tool>> {
        vec![]
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }
}

// ---------------------------------------------------------------------------
// Parse helpers
// ---------------------------------------------------------------------------

fn parse_tools_list(result: &Value, server_name: &str) -> Result<Vec<McpToolDef>, McpError> {
    let tools_arr = result
        .get("tools")
        .and_then(|v| v.as_array())
        .ok_or_else(|| McpError::ProtocolError {
            server: server_name.to_string(),
            message: "tools/list response missing 'tools' array".to_string(),
        })?;

    tools_arr
        .iter()
        .map(|tool| {
            let name = tool
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::ProtocolError {
                    server: server_name.to_string(),
                    message: "tool entry missing 'name' field".to_string(),
                })?
                .to_string();

            let description = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let input_schema = tool
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object"}));

            Ok(McpToolDef {
                name,
                description,
                input_schema,
            })
        })
        .collect()
}
