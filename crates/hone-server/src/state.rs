use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::collaborative::CollaborativeStore;
use crate::governance::GovernanceStore;
use crate::rate_limit::RateLimiter;
use crate::session::SessionStore;
use crate::session_storage::SessionStorage;
use crate::tunnel::{MockTunnelManager, TunnelManager};

/// Default: 100 requests per minute per client — generous enough that existing
/// tests never hit the ceiling.
const DEFAULT_RATE_LIMIT: u32 = 100;
const DEFAULT_RATE_WINDOW: Duration = Duration::from_secs(60);

/// Resolve the default workspace root used when the operator does not supply
/// one explicitly.  Falls back to `/` only if `current_dir()` fails — the
/// same fallback used by the agent code path.
fn default_workspace_root() -> PathBuf {
    std::env::current_dir()
        .and_then(|p| p.canonicalize().or(Ok(p)))
        .unwrap_or_else(|_| PathBuf::from("/"))
}

#[derive(Clone)]
pub struct AppState {
    pub sessions: Arc<SessionStorage>,
    /// If `Some`, all requests must include a matching `Authorization: Bearer <token>` header.
    pub auth_token: Option<String>,
    pub tunnel_manager: Arc<dyn TunnelManager>,
    pub governance: Arc<GovernanceStore>,
    pub collaborative: Arc<CollaborativeStore>,
    pub rate_limiter: RateLimiter,
    /// Anthropic API key from `ANTHROPIC_API_KEY` env var. Drives `POST /run`.
    pub provider_key: Option<String>,
    /// Allowed root for `POST /run`'s `working_dir` field.  Any client-supplied
    /// `working_dir` is canonicalized and required to live under this prefix;
    /// requests outside it are rejected with `400 BAD REQUEST` before any
    /// agent is spawned.  Defaults to the server process's `current_dir()`
    /// (canonicalized).  See AUDIT-CRITICAL-1 / `tests/run_working_dir.rs`.
    pub workspace_root: PathBuf,
}

impl AppState {
    pub fn new(auth_token: Option<String>) -> Self {
        Self {
            sessions: Arc::new(SessionStorage::InMemory(SessionStore::new())),
            auth_token,
            tunnel_manager: Arc::new(MockTunnelManager::new()),
            governance: GovernanceStore::new(),
            collaborative: Arc::new(CollaborativeStore::new()),
            rate_limiter: RateLimiter::new(DEFAULT_RATE_LIMIT, DEFAULT_RATE_WINDOW),
            provider_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            workspace_root: default_workspace_root(),
        }
    }

    pub fn with_sessions(auth_token: Option<String>, sessions: Arc<SessionStorage>) -> Self {
        Self {
            sessions,
            auth_token,
            tunnel_manager: Arc::new(MockTunnelManager::new()),
            governance: GovernanceStore::new(),
            collaborative: Arc::new(CollaborativeStore::new()),
            rate_limiter: RateLimiter::new(DEFAULT_RATE_LIMIT, DEFAULT_RATE_WINDOW),
            provider_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            workspace_root: default_workspace_root(),
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
            rate_limiter: RateLimiter::new(DEFAULT_RATE_LIMIT, DEFAULT_RATE_WINDOW),
            provider_key: std::env::var("ANTHROPIC_API_KEY").ok(),
            workspace_root: default_workspace_root(),
        }
    }

    /// Build a state with a custom tunnel manager.
    pub fn with_tunnel(mut self, tunnel_manager: Arc<dyn TunnelManager>) -> Self {
        self.tunnel_manager = tunnel_manager;
        self
    }

    /// Build a state with a custom rate limiter (useful for tests).
    pub fn with_rate_limiter(mut self, limiter: RateLimiter) -> Self {
        self.rate_limiter = limiter;
        self
    }

    /// Override the provider key (useful for tests).
    pub fn with_provider_key(mut self, key: Option<String>) -> Self {
        self.provider_key = key;
        self
    }

    /// Override the allowed workspace root for `POST /run`.  Useful when an
    /// operator wants to scope sessions to a project directory rather than
    /// the server's cwd.
    pub fn with_workspace_root(mut self, root: PathBuf) -> Self {
        self.workspace_root = root;
        self
    }
}
