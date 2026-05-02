use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TeamUsage {
    pub total_tokens: u64,
    pub total_cost_usd: f64,
    pub by_model: HashMap<String, ModelUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ModelUsage {
    pub tokens: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TeamMember {
    pub id: String,
    pub name: String,
    pub role: String,
    pub joined_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TeamPolicy {
    pub allowed_models: Vec<String>,
    pub spend_cap_usd: Option<f64>,
    pub approved_extensions: Vec<String>,
}

/// In-memory team governance store.
pub struct GovernanceStore {
    members: RwLock<Vec<TeamMember>>,
    policy: RwLock<TeamPolicy>,
    usage: RwLock<TeamUsage>,
}

impl GovernanceStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            members: RwLock::new(Vec::new()),
            policy: RwLock::new(TeamPolicy {
                allowed_models: vec![
                    "claude-opus-4-5".to_owned(),
                    "claude-sonnet-4-5".to_owned(),
                    "claude-haiku-3-5".to_owned(),
                ],
                spend_cap_usd: None,
                approved_extensions: Vec::new(),
            }),
            usage: RwLock::new(TeamUsage {
                total_tokens: 0,
                total_cost_usd: 0.0,
                by_model: HashMap::new(),
            }),
        })
    }

    pub async fn get_members(&self) -> Vec<TeamMember> {
        self.members.read().await.clone()
    }

    pub async fn add_member(&self, member: TeamMember) {
        self.members.write().await.push(member);
    }

    pub async fn get_policy(&self) -> TeamPolicy {
        self.policy.read().await.clone()
    }

    pub async fn set_policy(&self, policy: TeamPolicy) {
        *self.policy.write().await = policy;
    }

    pub async fn get_usage(&self) -> TeamUsage {
        self.usage.read().await.clone()
    }

    pub async fn add_usage(&self, model: &str, tokens: u64, cost: f64) {
        let mut usage = self.usage.write().await;
        usage.total_tokens = usage.total_tokens.saturating_add(tokens);
        usage.total_cost_usd += cost;
        let entry = usage
            .by_model
            .entry(model.to_owned())
            .or_insert(ModelUsage {
                tokens: 0,
                cost_usd: 0.0,
            });
        entry.tokens = entry.tokens.saturating_add(tokens);
        entry.cost_usd += cost;
    }
}
