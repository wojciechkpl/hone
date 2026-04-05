use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

use crate::error::ServerError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelConfig {
    pub local_port: u16,
    pub auth_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelInfo {
    pub url: String,
    pub local_port: u16,
    pub is_active: bool,
}

#[async_trait]
pub trait TunnelManager: Send + Sync {
    async fn start(&self, config: &TunnelConfig) -> Result<TunnelInfo, ServerError>;
    async fn stop(&self) -> Result<(), ServerError>;
    async fn status(&self) -> Result<Option<TunnelInfo>, ServerError>;
}

#[derive(Clone, Default)]
pub struct MockTunnelManager {
    state: Arc<RwLock<Option<TunnelInfo>>>,
}

impl MockTunnelManager {
    pub fn new() -> Self {
        Self {
            state: Arc::new(RwLock::new(None)),
        }
    }
}

#[async_trait]
impl TunnelManager for MockTunnelManager {
    async fn start(&self, config: &TunnelConfig) -> Result<TunnelInfo, ServerError> {
        let info = TunnelInfo {
            url: format!("http://mock-tunnel.example.com:{}", config.local_port),
            local_port: config.local_port,
            is_active: true,
        };
        *self.state.write().await = Some(info.clone());
        Ok(info)
    }

    async fn stop(&self) -> Result<(), ServerError> {
        *self.state.write().await = None;
        Ok(())
    }

    async fn status(&self) -> Result<Option<TunnelInfo>, ServerError> {
        Ok(self.state.read().await.clone())
    }
}
