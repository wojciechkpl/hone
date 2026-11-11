use std::sync::Arc;
use std::time::Duration;

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
    /// Any JSON value (a server may use string ids); `null` and absent both
    /// read as `None`.
    id: Option<Value>,
    /// Set on notifications and server-to-client requests, which are not
    /// responses to ours.
    method: Option<String>,
    result: Option<Value>,
    error: Option<JsonRpcErrorObj>,
}

#[derive(Deserialize, Debug)]
struct JsonRpcErrorObj {
    code: i64,
    message: String,
}

/// A handshake is bounded by at least this long (see `handshake_limit`).
const DEFAULT_HANDSHAKE_FLOOR: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Inner mutable transport state
// ---------------------------------------------------------------------------

struct TransportInner {
    /// Keep the child alive so it is not reaped while we hold its pipes.
    #[allow(dead_code)]
    process: Child,
    /// `None` once closed at shutdown (the server sees end of input).
    stdin: Option<BufWriter<ChildStdin>>,
    reader: BufReader<ChildStdout>,
    next_id: u64,
    /// Bounds each request (write + wait for its response). `None`: no limit.
    request_timeout: Option<Duration>,
    /// Overrides the bound of the startup handshake (`Some(ZERO)` = none).
    handshake_timeout: Option<Duration>,
}

impl TransportInner {
    /// The server's stdin, or a broken-pipe error once it was closed.
    fn stdin_writer(&mut self, server_name: &str) -> Result<&mut BufWriter<ChildStdin>, McpError> {
        self.stdin
            .as_mut()
            .ok_or_else(|| McpError::ConnectionFailed {
                server: server_name.to_string(),
                source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "stdin closed"),
            })
    }

    /// The bound of the startup handshake: the override if set (`ZERO` = none),
    /// else the larger of the request timeout and 60 s (a first `npx` start may
    /// download its package), and none when requests are unbounded.
    fn handshake_limit(&self) -> Option<Duration> {
        match self.handshake_timeout {
            Some(d) => (!d.is_zero()).then_some(d),
            None => self.request_timeout.map(|r| r.max(DEFAULT_HANDSHAKE_FLOOR)),
        }
    }

    /// Send a request and wait for its response, bounded by the request
    /// timeout (if any): on expiry the call fails with [`McpError::Timeout`]
    /// and the late response is skipped by id on a later call.
    async fn send_request(
        &mut self,
        method: &str,
        params: Option<Value>,
        server_name: &str,
    ) -> Result<Value, McpError> {
        let limit = self.request_timeout;
        self.send_request_within(limit, method, params, server_name)
            .await
    }

    /// [`Self::send_request`] with an explicit bound (`None` = unbounded).
    async fn send_request_within(
        &mut self,
        limit_for_call: Option<Duration>,
        method: &str,
        params: Option<Value>,
        server_name: &str,
    ) -> Result<Value, McpError> {
        match limit_for_call {
            None => {
                self.send_request_unbounded(method, params, server_name)
                    .await
            }
            Some(limit) => tokio::time::timeout(
                limit,
                self.send_request_unbounded(method, params, server_name),
            )
            .await
            .map_err(|_| McpError::Timeout {
                server: server_name.to_string(),
                timeout_ms: u64::try_from(limit.as_millis()).unwrap_or(u64::MAX),
            })?,
        }
    }

    async fn send_request_unbounded(
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

        self.stdin_writer(server_name)?
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.to_string(),
                source: e,
            })?;
        self.stdin_writer(server_name)?
            .flush()
            .await
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.to_string(),
                source: e,
            })?;

        // Read lines until the response to THIS request: skip blank lines,
        // notifications and responses with another id (e.g. the late reply to a
        // call that was dropped by a timeout or a cancel), so a result is never
        // handed to the wrong caller. A server request gets a reply meanwhile.
        let our_id = Value::from(id);
        loop {
            let mut raw_line = Vec::new();
            let bytes = self
                .reader
                .read_until(b'\n', &mut raw_line)
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

            // Bytes up to the newline, decoded here: a line cut inside a
            // multi-byte character (after a dropped read) is not valid UTF-8 and
            // is skipped like any other non-JSON line.
            let response_line = match String::from_utf8(raw_line) {
                Ok(l) => l,
                Err(_) => {
                    tracing::debug!(
                        server = server_name,
                        "skipping a line that is not valid UTF-8 on the server's stdout"
                    );
                    continue;
                }
            };
            let trimmed = response_line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Log output (or the rest of a line whose read was dropped) is not a
            // response: skip it.
            let resp: JsonRpcResponse = match serde_json::from_str(trimmed) {
                Ok(r) => r,
                Err(e) => {
                    tracing::debug!(
                        server = server_name,
                        error = %e,
                        "skipping a non-JSON line on the server's stdout"
                    );
                    continue;
                }
            };

            if let Some(method) = &resp.method {
                // A server-to-client request (it has an id) must be answered or
                // the server may wait on it forever; a notification is skipped.
                if let Some(server_id) = resp.id {
                    let reply = if method == "ping" {
                        serde_json::json!({ "jsonrpc": "2.0", "id": server_id, "result": {} })
                    } else {
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": server_id,
                            "error": { "code": -32601, "message": "Method not found" },
                        })
                    };
                    let mut reply_line = reply.to_string();
                    reply_line.push('\n');
                    // `self` is borrowed mutably for the whole call, so no other
                    // writer or reader can be waiting on these pipes.
                    self.stdin_writer(server_name)?
                        .write_all(reply_line.as_bytes())
                        .await
                        .map_err(|e| McpError::ConnectionFailed {
                            server: server_name.to_string(),
                            source: e,
                        })?;
                    self.stdin_writer(server_name)?.flush().await.map_err(|e| {
                        McpError::ConnectionFailed {
                            server: server_name.to_string(),
                            source: e,
                        }
                    })?;
                }
                continue;
            }

            // An error with a null or absent id (e.g. "Invalid Request") answers
            // the outstanding request; anything else must carry our id.
            let answers_us = match &resp.id {
                Some(got) => *got == our_id,
                None => resp.error.is_some(),
            };
            if !answers_us {
                tracing::debug!(
                    server = server_name,
                    expected = id,
                    got = ?resp.id,
                    "skipping a message that is not the response to this request"
                );
                continue;
            }

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

        self.stdin_writer(server_name)?
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::ConnectionFailed {
                server: server_name.to_string(),
                source: e,
            })?;
        self.stdin_writer(server_name)?
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
            .kill_on_drop(true)
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
                stdin: Some(BufWriter::new(stdin)),
                reader: BufReader::new(stdout),
                next_id: 1,
                request_timeout: None,
                handshake_timeout: None,
            })),
            server_info: Mutex::new(None),
        })
    }

    /// Bound every request (`initialize`, `tools/list`, `tools/call`, ...) by
    /// `timeout`: write plus wait for the response. `Duration::ZERO` means no
    /// timeout, which is also the default of the raw constructors.
    /// Notifications are not bounded. Call it before `connect()`.
    #[must_use]
    pub fn with_request_timeout(mut self, timeout: Duration) -> Self {
        let limit = (!timeout.is_zero()).then_some(timeout);
        self.transport_mut().request_timeout = limit;
        self
    }

    /// Bound the startup handshake (`connect()`: `initialize` and the first
    /// `tools/list`) by `timeout`; `Duration::ZERO` means no bound. Without it
    /// the handshake gets the larger of the request timeout and 60 s (a first
    /// `npx` start may download its package), and none if requests are
    /// unbounded. Later requests keep the request timeout.
    #[must_use]
    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.transport_mut().handshake_timeout = Some(timeout);
        self
    }

    /// The transport settings, for the builders. The transport is only shared
    /// (with tool wrappers) after `tools_as_native`, so a freshly built client
    /// is its sole owner.
    fn transport_mut(&mut self) -> &mut TransportInner {
        match Arc::get_mut(&mut self.transport) {
            Some(mutex) => mutex.get_mut(),
            // Unreachable for a client that was just constructed; a client whose
            // transport is already shared must not be reconfigured.
            None => unreachable!("the transport is shared: configure the client before use"),
        }
    }

    /// Kill the server process and reap it (startup failure path).
    pub(crate) async fn kill(&mut self) {
        let mut transport = self.transport.lock().await;
        let _ = transport.process.kill().await;
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
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true);

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
                stdin: Some(BufWriter::new(stdin)),
                reader: BufReader::new(stdout),
                next_id: 1,
                request_timeout: None,
                handshake_timeout: None,
            })),
            server_info: Mutex::new(None),
        })
    }

    /// Gracefully stop the MCP server subprocess.
    ///
    /// Sends a `notifications/cancelled` notification, closes the server's
    /// stdin and waits for the process to exit; all of that shares one 5 second
    /// budget, after which the process is killed. (Waiting for the transport
    /// lock, held by a request in flight, is outside the budget.)
    pub async fn shutdown(&mut self) -> Result<(), McpError> {
        let budget = tokio::time::Duration::from_secs(5);
        let server_name = self.server_name.clone();
        let mut transport = self.transport.lock().await;
        let ended = tokio::time::timeout(budget, async {
            // Best-effort notification — the server may already be gone.
            let _ = transport
                .send_notification("notifications/cancelled", None, &server_name)
                .await;
            // Close the server's stdin (after flushing the notification), so a
            // server that exits when its input ends is gone at once; the wait
            // and kill remain the fallback for one that does not.
            if let Some(mut stdin) = transport.stdin.take() {
                let _ = stdin.flush().await;
                drop(stdin);
            }
            let _ = transport.process.wait().await;
        })
        .await;
        if ended.is_err() {
            // Out of budget (also when it ran out writing the notification):
            // forcibly kill the subprocess.
            let _ = transport.process.kill().await;
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

        // The handshake (initialize + first tools/list) has its own bound.
        let handshake = transport.handshake_limit();
        let init_result = transport
            .send_request_within(
                handshake,
                "initialize",
                Some(init_params),
                &self.server_name,
            )
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
            .send_request_within(handshake, "tools/list", None, &self.server_name)
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
