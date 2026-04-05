use crate::session::SessionStore;

#[derive(Clone)]
pub struct AppState {
    pub sessions: SessionStore,
    /// If `Some`, all requests must include a matching `Authorization: Bearer <token>` header.
    pub auth_token: Option<String>,
}

impl AppState {
    pub fn new(auth_token: Option<String>) -> Self {
        Self {
            sessions: SessionStore::new(),
            auth_token,
        }
    }
}
