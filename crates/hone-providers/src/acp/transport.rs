use std::collections::VecDeque;
use std::process::Stdio;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use crate::error::ProviderError;

use super::types::{JsonRpcNotification, JsonRpcRequest, JsonRpcResponse};

/// JSON-RPC 2.0 transport over the stdin/stdout of a persistent child process.
///
/// `request()` blocks until the response with the matching `id` arrives.
/// Any notifications received while waiting are buffered and returned by
/// subsequent calls to `next_notification()`.
pub struct AcpTransport {
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    child: Child,
    next_id: u64,
    notification_buf: VecDeque<JsonRpcNotification>,
}

impl AcpTransport {
    /// Spawn a child process and create a transport over its stdin/stdout.
    pub async fn spawn(binary: &str, args: &[&str]) -> Result<Self, ProviderError> {
        let mut cmd = Command::new(binary);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());

        let mut child = cmd.spawn().map_err(|e| ProviderError::Stream {
            provider: "acp".to_owned(),
            source: e,
        })?;

        let stdin = child.stdin.take().ok_or_else(|| ProviderError::Stream {
            provider: "acp".to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "failed to capture child stdin",
            ),
        })?;

        let stdout = child.stdout.take().ok_or_else(|| ProviderError::Stream {
            provider: "acp".to_owned(),
            source: std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "failed to capture child stdout",
            ),
        })?;

        Ok(Self {
            stdin,
            reader: BufReader::new(stdout),
            child,
            next_id: 0,
            notification_buf: VecDeque::new(),
        })
    }

    /// Send a JSON-RPC request and wait synchronously for the matching response.
    ///
    /// Lines received while waiting that have no `id` field (notifications) are
    /// buffered in `notification_buf` for later retrieval.
    pub async fn request(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<Value, ProviderError> {
        let id = self.next_id;
        self.next_id += 1;

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_owned(),
            id,
            method: method.to_owned(),
            params,
        };

        self.write_line(&serde_json::to_string(&request)?).await?;

        // Read lines until we find the response with matching id.
        loop {
            let line = self.read_line().await?;
            if line.is_empty() {
                continue;
            }

            // Attempt to parse as a response first (has `id`).
            if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(&line) {
                if resp.id == id {
                    if let Some(error) = resp.error {
                        return Err(ProviderError::Stream {
                            provider: "acp".to_owned(),
                            source: std::io::Error::new(
                                std::io::ErrorKind::Other,
                                format!("JSON-RPC error {}: {}", error.code, error.message),
                            ),
                        });
                    }
                    return Ok(resp.result.unwrap_or(Value::Null));
                }
                // Response for a different id — unlikely but continue reading.
                continue;
            }

            // Attempt to parse as a notification (no `id`).
            if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(&line) {
                self.notification_buf.push_back(notif);
            }
            // Unrecognised lines are silently ignored.
        }
    }

    /// Send a JSON-RPC notification (fire-and-forget, no id, no response).
    pub async fn notify(
        &mut self,
        method: &str,
        params: Option<Value>,
    ) -> Result<(), ProviderError> {
        let notif = JsonRpcNotification {
            jsonrpc: "2.0".to_owned(),
            method: method.to_owned(),
            params,
        };

        self.write_line(&serde_json::to_string(&notif)?).await
    }

    /// Return the next `session/update` notification.
    ///
    /// Buffered notifications (captured while waiting for a response) are
    /// returned first; then new lines are read from stdout.
    pub async fn next_notification(&mut self) -> Result<JsonRpcNotification, ProviderError> {
        if let Some(notif) = self.notification_buf.pop_front() {
            return Ok(notif);
        }

        loop {
            let line = self.read_line().await?;
            if line.is_empty() {
                continue;
            }

            if let Ok(notif) = serde_json::from_str::<JsonRpcNotification>(&line) {
                return Ok(notif);
            }
            // Responses (lines with `id`) are ignored here; callers should not
            // mix request/notification reads without coordination.
        }
    }

    /// Return `true` if the child process is still running.
    pub fn is_alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) => true,   // still running
            Ok(Some(_)) => false, // exited
            Err(_) => false,
        }
    }

    // ------------------------------------------------------------------
    // Private helpers
    // ------------------------------------------------------------------

    async fn write_line(&mut self, line: &str) -> Result<(), ProviderError> {
        let mut buf = line.to_owned();
        buf.push('\n');
        self.stdin
            .write_all(buf.as_bytes())
            .await
            .map_err(|e| ProviderError::Stream {
                provider: "acp".to_owned(),
                source: e,
            })?;
        self.stdin.flush().await.map_err(|e| ProviderError::Stream {
            provider: "acp".to_owned(),
            source: e,
        })
    }

    async fn read_line(&mut self) -> Result<String, ProviderError> {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .await
            .map_err(|e| ProviderError::Stream {
                provider: "acp".to_owned(),
                source: e,
            })?;
        Ok(line.trim_end_matches('\n').trim_end_matches('\r').to_owned())
    }
}
