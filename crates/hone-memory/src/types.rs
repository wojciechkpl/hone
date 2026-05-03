use serde::{Deserialize, Serialize};

use crate::embedding::Embedding;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryKind {
    /// Free-form Q&A (today's default — the noisy ones).
    #[default]
    Conversation,
    /// Stable facts about the user's role / preferences / knowledge.
    User,
    /// Corrections AND validated successes; structured `rule / why / how-to-apply`.
    Feedback,
    /// Time-bounded facts: ongoing work, decisions, deadlines.
    Project,
    /// Pointers into external systems (Linear, Slack, Grafana, etc.).
    Reference,
    /// System-emitted records (outcomes, audit trails). Excluded from typed-recall.
    Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub weight: f64,
    pub created_at: u64,
    pub decay_at: Option<u64>,
    #[serde(default)]
    pub kind: MemoryKind,
    #[serde(default)]
    pub description: Option<String>,
    /// Optional pre-computed embedding vector. Absent for memories stored
    /// before the semantic-recall feature was introduced; those memories
    /// are skipped by `recall_semantic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Embedding>,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            id: String::new(),
            content: String::new(),
            tags: vec![],
            weight: 1.0,
            created_at: 0,
            decay_at: None,
            kind: MemoryKind::Conversation,
            description: None,
            embedding: None,
        }
    }
}

/// A recall query.
///
/// Note: `MemoryQuery::default()` produces `{ text: "", limit: 0, kind: None }`
/// and stores honour `limit: 0` literally (returning zero results).  Always
/// set a non-zero `limit` explicitly when constructing via spread.
#[derive(Debug, Clone, Default)]
pub struct MemoryQuery {
    pub text: String,
    pub limit: usize,
    /// When `Some(k)`, the store filters to memories of that kind BEFORE
    /// applying `limit`. When `None`, no kind filter is applied.
    pub kind: Option<MemoryKind>,
}

#[derive(Debug, Clone)]
pub enum MemoryFilter {
    Tag(String),
    Before(u64),
    After(u64),
    Kind(MemoryKind),
}
