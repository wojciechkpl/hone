use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Memory {
    pub id: String,
    pub content: String,
    pub tags: Vec<String>,
    pub weight: f64,
    pub created_at: u64,
    pub decay_at: Option<u64>,
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
}
