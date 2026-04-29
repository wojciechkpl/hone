use std::sync::Arc;

use async_trait::async_trait;
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::{Child, Command};
use tokio::sync::RwLock;

use crate::error::ServerError;
use crate::tunnel::{TunnelConfig, TunnelInfo, TunnelManager};

/// Real tunnel backed by the `bore` CLI (https://github.com/ekzhang/bore).
/// Requires `bore` binary to be installed and available on PATH.
pub struct BoreTunnelManager {
    inner: Arc<RwLock<BoreTunnelInner>>,
    bore_server: String,
}

struct BoreTunnelInner {
    info: Option<TunnelInfo>,
    child: Option<Child>,
}

impl BoreTunnelManager {
    /// Create a new BoreTunnelManager using the default bore.pub server.
    pub fn new() -> Self {
        Self::with_server("bore.pub")
    }

    /// Create a manager pointing at a specific bore server.
    pub fn with_server(server: impl Into<String>) -> Self {
        Self {
            inner: Arc::new(RwLock::new(BoreTunnelInner {
                info: None,
                child: None,
            })),
            bore_server: server.into(),
        }
    }

    /// Check if the bore binary is available on PATH.
    pub async fn is_available() -> bool {
        Command::new("which")
            .arg("bore")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}

#[async_trait]
impl TunnelManager for BoreTunnelManager {
    async fn start(&self, config: &TunnelConfig) -> Result<TunnelInfo, ServerError> {
        let mut inner = self.inner.write().await;

        // If already running, return existing info.
        if let Some(existing) = &inner.info {
            return Ok(existing.clone());
        }

        // Spawn: bore local <local_port> --to <server>
        let mut child = Command::new("bore")
            .arg("local")
            .arg(config.local_port.to_string())
            .arg("--to")
            .arg(&self.bore_server)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| ServerError::Internal(format!("failed to spawn bore: {e}")))?;

        // Read stderr for the "listening at bore.pub:XXXXX" line.
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| ServerError::Internal("failed to capture bore stderr".to_string()))?;

        let mut reader = BufReader::new(stderr).lines();

        // Bore prints: "INFO bore_cli::client: listening at bore.pub:XXXXX"
        // Parse with a short timeout.
        let read_task = async {
            while let Ok(Some(line)) = reader.next_line().await {
                tracing::debug!("bore: {line}");
                if let Some(idx) = line.find("listening at") {
                    let rest = &line[idx + "listening at".len()..];
                    if let Some(colon) = rest.rfind(':') {
                        let port_str = rest[colon + 1..].trim();
                        if let Ok(p) = port_str.parse::<u16>() {
                            return Some(p);
                        }
                    }
                }
            }
            None
        };

        let remote_port = tokio::time::timeout(std::time::Duration::from_secs(10), read_task)
            .await
            .ok()
            .flatten();

        let port = remote_port.ok_or_else(|| {
            ServerError::Internal("failed to parse bore tunnel port from stderr".to_string())
        })?;

        let info = TunnelInfo {
            url: format!("http://{}:{}", self.bore_server, port),
            local_port: config.local_port,
            is_active: true,
        };

        inner.info = Some(info.clone());
        inner.child = Some(child);

        Ok(info)
    }

    async fn stop(&self) -> Result<(), ServerError> {
        let mut inner = self.inner.write().await;
        if let Some(mut child) = inner.child.take() {
            let _ = child.kill().await;
        }
        inner.info = None;
        Ok(())
    }

    async fn status(&self) -> Result<Option<TunnelInfo>, ServerError> {
        let inner = self.inner.read().await;
        Ok(inner.info.clone())
    }
}

impl Default for BoreTunnelManager {
    fn default() -> Self {
        Self::new()
    }
}
