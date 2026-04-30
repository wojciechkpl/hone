use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
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
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryQuery {
    pub text: String,
    pub limit: usize,
}

#[derive(Debug, Clone)]
pub enum MemoryFilter {
    Tag(String),
    Before(u64),
    After(u64),
    Kind(MemoryKind),
}
