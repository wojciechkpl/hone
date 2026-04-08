use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use utoipa::ToSchema;

use crate::error::ServerError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum ParticipantRole {
    Owner,
    Editor,
    Viewer,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, ToSchema)]
pub struct Participant {
    pub user_id: String,
    pub name: String,
    pub role: ParticipantRole,
    pub joined_at: u64,
}

/// Tracks participants per session.
pub struct CollaborativeStore {
    participants: RwLock<HashMap<String, Vec<Participant>>>,
}

impl CollaborativeStore {
    pub fn new() -> Self {
        Self {
            participants: RwLock::new(HashMap::new()),
        }
    }

    pub async fn join(
        &self,
        session_id: &str,
        user_id: String,
        name: String,
    ) -> Result<Participant, ServerError> {
        let mut map = self.participants.write().await;
        let list = map.entry(session_id.to_owned()).or_default();

        if list.iter().any(|p| p.user_id == user_id) {
            return Err(ServerError::AlreadyJoined {
                session_id: session_id.to_owned(),
                user_id,
            });
        }

        let role = if list.is_empty() {
            ParticipantRole::Owner
        } else {
            ParticipantRole::Editor
        };

        let joined_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let participant = Participant {
            user_id,
            name,
            role,
            joined_at,
        };

        list.push(participant.clone());
        Ok(participant)
    }

    pub async fn leave(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<(), ServerError> {
        let mut map = self.participants.write().await;
        let list = map.entry(session_id.to_owned()).or_default();

        let pos = list
            .iter()
            .position(|p| p.user_id == user_id)
            .ok_or_else(|| ServerError::NotParticipant {
                session_id: session_id.to_owned(),
                user_id: user_id.to_owned(),
            })?;

        list.remove(pos);
        Ok(())
    }

    pub async fn kick(
        &self,
        session_id: &str,
        owner_user_id: &str,
        target_user_id: &str,
    ) -> Result<(), ServerError> {
        let mut map = self.participants.write().await;
        let list = map.entry(session_id.to_owned()).or_default();

        let owner_is_owner = list
            .iter()
            .any(|p| p.user_id == owner_user_id && p.role == ParticipantRole::Owner);

        if !owner_is_owner {
            return Err(ServerError::Forbidden);
        }

        let pos = list
            .iter()
            .position(|p| p.user_id == target_user_id)
            .ok_or_else(|| ServerError::NotParticipant {
                session_id: session_id.to_owned(),
                user_id: target_user_id.to_owned(),
            })?;

        list.remove(pos);
        Ok(())
    }

    pub async fn list_participants(&self, session_id: &str) -> Vec<Participant> {
        self.participants
            .read()
            .await
            .get(session_id)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn is_participant(&self, session_id: &str, user_id: &str) -> bool {
        self.participants
            .read()
            .await
            .get(session_id)
            .map(|list| list.iter().any(|p| p.user_id == user_id))
            .unwrap_or(false)
    }
}

impl Default for CollaborativeStore {
    fn default() -> Self {
        Self::new()
    }
}

pub type SharedCollaborativeStore = Arc<CollaborativeStore>;
