/// Maximum request body size enforced at the router level for ALL routes.
///
/// Chosen to be above Axum 0.8's implicit per-extractor `DefaultBodyLimit`
/// of 2 MiB so that legitimate `POST /run` payloads at the 2 MiB boundary
/// continue to reach the `Json<T>` extractor, while bodies well above any
/// sane limit (e.g. 16 MiB) are rejected with `413 Payload Too Large` before
/// any handler logic runs — including routes that have no body extractor at
/// all (e.g. `POST /sessions`).
pub const HONE_MAX_BODY_BYTES: usize = 4 * 1024 * 1024; // 4 MiB

pub mod auth;
pub mod bore_tunnel;
pub mod collaborative;
pub mod distributed;
pub mod distributed_reaper;
pub mod distributed_routes;
pub mod collaborative_routes;
pub mod error;
pub mod execution;
pub mod governance;
pub mod governance_routes;
pub mod openapi;
pub mod rate_limit;
pub mod routes;
pub mod scheduler;
pub mod session;
pub mod session_storage;
pub mod sqlite_session;
pub mod sse;
pub mod state;
pub mod tunnel;
pub mod tunnel_routes;
pub mod web;

pub use bore_tunnel::BoreTunnelManager;
pub use collaborative::{CollaborativeStore, Participant, ParticipantRole};
pub use distributed::{
    ClaimedTask, CompletionPayload, DistributedError, DistributedTaskStore, TaskState, WorkerId,
};
pub use distributed_reaper::spawn_lease_reaper;
pub use distributed_routes::distributed_router;
pub use execution::build_sandbox;
pub use governance::{GovernanceStore, ModelUsage, TeamMember, TeamPolicy, TeamUsage};
pub use openapi::ApiDoc;
pub use rate_limit::RateLimiter;
pub use routes::router;
pub use session_storage::SessionStorage;
pub use sqlite_session::SqliteSessionStore;
pub use state::AppState;
pub use tunnel::{MockTunnelManager, TunnelManager};
