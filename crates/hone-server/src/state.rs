use std::sync::Arc;

use crate::collaborative::CollaborativeStore;
use crate::governance::GovernanceStore;
use crate::session::SessionStore;
use crate::session_storage::SessionStorage;
use crate::tunnel::{MockTunnelManager, TunnelManager};

#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<SessionStorage>,
    /// If `Some`, all requests must include a matching `Authorization: Bearer <token>` header.
    pub auth_token: Option<String>,
    pub tunnel_manager: Arc<dyn TunnelManager>,
    pub governance: Arc<GovernanceStore>,
    pub collaborative: Arc<CollaborativeStore>,
}

impl AppState {
    pub fn new(auth_token: Option<String>) -> Self {
        Self {
            sessions: Arc::new(SessionStorage::InMemory(SessionStore::new())),
            auth_token,
            tunnel_manager: Arc::new(MockTunnelManager::new()),
            governance: GovernanceStore::new(),
            collaborative: Arc::new(CollaborativeStore::new()),
        }
    }

    pub fn with_sessions(auth_token: Option<String>, sessions: Arc<SessionStorage>) -> Self {
        Self {
            sessions,
            auth_token,
            tunnel_manager: Arc::new(MockTunnelManager::new()),
            governance: GovernanceStore::new(),
            collaborative: Arc::new(CollaborativeStore::new()),
        }
    }

    pub fn with_tunnel_manager(
        auth_token: Option<String>,
        tunnel_manager: Arc<dyn TunnelManager>,
    ) -> Self {
        Self {
            sessions: Arc::new(SessionStorage::InMemory(SessionStore::new())),
            auth_token,
            tunnel_manager,
            governance: GovernanceStore::new(),
            collaborative: Arc::new(CollaborativeStore::new()),
        }
    }
}
