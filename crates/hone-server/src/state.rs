use std::sync::Arc;

use crate::session::SessionStore;
use crate::tunnel::{MockTunnelManager, TunnelManager};

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionStore,
    /// If `Some`, all requests must include a matching `Authorization: Bearer <token>` header.
    pub auth_token: Option<String>,
    pub tunnel_manager: Arc<dyn TunnelManager>,
}

impl AppState {
    pub fn new(auth_token: Option<String>) -> Self {
        Self {
            sessions: SessionStore::new(),
            auth_token,
            tunnel_manager: Arc::new(MockTunnelManager::new()),
        }
    }

    pub fn with_tunnel_manager(
        auth_token: Option<String>,
        tunnel_manager: Arc<dyn TunnelManager>,
    ) -> Self {
        Self {
            sessions: SessionStore::new(),
            auth_token,
            tunnel_manager,
        }
    }
}
