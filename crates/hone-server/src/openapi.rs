use utoipa::OpenApi;

use crate::collaborative::{Participant, ParticipantRole};
use crate::governance::{ModelUsage, TeamMember, TeamPolicy, TeamUsage};
use crate::routes::GcResult;
use crate::session::SessionInfo;
use crate::tunnel::{TunnelConfig, TunnelInfo};

#[derive(OpenApi)]
#[openapi(
    info(
        title = "Hone HTTP API",
        version = "0.1.0",
        description = "HTTP/2 + SSE API for the Hone AI coding agent daemon",
        license(name = "MIT OR Apache-2.0"),
    ),
    paths(
        crate::routes::health,
        crate::routes::create_session,
        crate::routes::get_session,
        crate::routes::delete_session,
        crate::routes::gc_sessions,
    ),
    components(
        schemas(
            SessionInfo,
            GcResult,
            TunnelConfig,
            TunnelInfo,
            TeamPolicy,
            TeamUsage,
            TeamMember,
            ModelUsage,
            Participant,
            ParticipantRole,
        )
    ),
    tags(
        (name = "system", description = "System health and diagnostics"),
        (name = "sessions", description = "Session lifecycle management"),
        (name = "governance", description = "Team governance and policies"),
        (name = "tunnel", description = "Remote access tunnel management"),
        (name = "collaborative", description = "Multi-user collaboration"),
    )
)]
pub struct ApiDoc;

impl ApiDoc {
    /// Generate the OpenAPI 3.1 spec as a JSON string.
    pub fn json() -> String {
        serde_json::to_string_pretty(&Self::openapi()).unwrap_or_else(|_| "{}".to_string())
    }
}
