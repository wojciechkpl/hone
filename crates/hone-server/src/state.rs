use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use hone_providers::ModelProvider;

use crate::collaborative::CollaborativeStore;
use crate::distributed::DistributedTaskStore;
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
    /// Bearer tokens whose holders carry the "admin" role.  Any token NOT in
    /// this set is treated as non-admin regardless of whether it passes the
    /// `auth_token` gate.  When the set is empty (the default), no caller is
    /// admin — operators must explicitly opt in by populating the set.
    /// Wrapped in `Arc` so cloning `AppState` is cheap (O(1)) even with large sets.
    pub admin_tokens: Arc<HashSet<String>>,
    /// Optional provider override used by tests to avoid live API calls.
    /// When `Some`, `run_handler` uses this instead of `AnthropicProvider`.
    pub mock_provider: Option<Arc<dyn ModelProvider>>,
    /// When Some, /distributed/* endpoints are enabled.
    pub distributed_store: Option<Arc<DistributedTaskStore>>,
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
            admin_tokens: Arc::new(HashSet::new()),
            mock_provider: None,
            distributed_store: None,
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
            admin_tokens: Arc::new(HashSet::new()),
            mock_provider: None,
            distributed_store: None,
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
            admin_tokens: Arc::new(HashSet::new()),
            mock_provider: None,
            distributed_store: None,
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

    /// Populate the set of bearer tokens whose holders carry the "admin" role.
    ///
    /// Only callers presenting a token from this set are permitted to call
    /// `POST /team/policy`.  An empty set (the default) means no caller is
    /// admin — operators must explicitly opt in.
    pub fn with_admin_tokens(mut self, tokens: HashSet<String>) -> Self {
        self.admin_tokens = Arc::new(tokens);
        self
    }

    /// Inject a test-only provider that replaces `AnthropicProvider` in
    /// `run_handler`.  Allows driving the full `/run` → usage-tracking path
    /// without a live API key.
    pub fn with_mock_provider(mut self, p: Arc<dyn ModelProvider>) -> Self {
        self.mock_provider = Some(p);
        self
    }

    /// Attach a distributed task store, enabling the `/distributed/*` endpoints.
    pub fn with_distributed_store(mut self, store: Arc<DistributedTaskStore>) -> Self {
        self.distributed_store = Some(store);
        self
    }
}
