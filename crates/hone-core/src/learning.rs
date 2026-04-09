//! Cross-session learning: record outcomes and feed them into adaptive routing.
//!
//! After each agent turn, the outcome (success/failure, cost, tokens, complexity
//! tier, model used) is recorded as a tagged memory. On subsequent sessions, the
//! [`AdaptiveClassifier`] queries past outcomes to refine its routing decisions.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::complexity::ComplexityTier;
use hone_memory::{Memory, MemoryQuery, MemoryStore};

/// A recorded outcome from a completed agent turn or orchestrator run.
#[derive(Debug, Clone)]
pub struct OutcomeRecord {
    /// The complexity tier that was selected for this task.
    pub tier: ComplexityTier,
    /// Model used (e.g., "anthropic:claude-sonnet-4-5").
    pub model: String,
    /// Whether the task completed successfully.
    pub success: bool,
    /// Total cost in USD.
    pub cost_usd: f64,
    /// Total input + output tokens.
    pub total_tokens: u64,
    /// Brief description of the task (first 200 chars of user request).
    pub task_summary: String,
}

impl OutcomeRecord {
    /// Serialize as a memory-storable string with a structured format.
    fn to_content(&self) -> String {
        format!(
            "outcome:tier={:?}|model={}|success={}|cost={:.4}|tokens={}|task={}",
            self.tier,
            self.model,
            self.success,
            self.cost_usd,
            self.total_tokens,
            &self.task_summary[..200.min(self.task_summary.len())],
        )
    }

    /// Parse from stored memory content.
    fn from_content(content: &str) -> Option<Self> {
        let content = content.strip_prefix("outcome:")?;
        let mut tier = None;
        let mut model = String::new();
        let mut success = false;
        let mut cost_usd = 0.0;
        let mut total_tokens = 0u64;
        let mut task_summary = String::new();

        for part in content.split('|') {
            if let Some(v) = part.strip_prefix("tier=") {
                tier = match v {
                    "Simple" => Some(ComplexityTier::Simple),
                    "Medium" => Some(ComplexityTier::Medium),
                    "Complex" => Some(ComplexityTier::Complex),
                    _ => None,
                };
            } else if let Some(v) = part.strip_prefix("model=") {
                model = v.to_string();
            } else if let Some(v) = part.strip_prefix("success=") {
                success = v == "true";
            } else if let Some(v) = part.strip_prefix("cost=") {
                cost_usd = v.parse().unwrap_or(0.0);
            } else if let Some(v) = part.strip_prefix("tokens=") {
                total_tokens = v.parse().unwrap_or(0);
            } else if let Some(v) = part.strip_prefix("task=") {
                task_summary = v.to_string();
            }
        }

        Some(Self {
            tier: tier?,
            model,
            success,
            cost_usd,
            total_tokens,
            task_summary,
        })
    }
}

/// Records outcomes and provides adaptive complexity routing based on
/// historical performance.
pub struct AdaptiveClassifier {
    store: Arc<Mutex<dyn MemoryStore>>,
}

impl AdaptiveClassifier {
    pub fn new(store: Arc<Mutex<dyn MemoryStore>>) -> Self {
        Self { store }
    }

    /// Record an outcome for future learning.
    pub async fn record_outcome(&self, outcome: OutcomeRecord) {
        let id = uuid::Uuid::now_v7().to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let memory = Memory {
            id,
            content: outcome.to_content(),
            tags: vec![
                "outcome".to_string(),
                format!("tier:{:?}", outcome.tier),
                if outcome.success {
                    "success".to_string()
                } else {
                    "failure".to_string()
                },
            ],
            weight: 1.0,
            created_at: now,
            decay_at: Some(now + 30 * 86_400), // decay after 30 days
        };

        let mut store = self.store.lock().await;
        if let Err(e) = store.record(memory).await {
            tracing::warn!("failed to record outcome: {e}");
        }
    }

    /// Query historical outcomes and suggest whether to escalate or simplify
    /// the complexity tier for a similar task.
    ///
    /// Returns `None` if there's insufficient history (< 5 outcomes).
    pub async fn suggest_tier(&self, task_summary: &str) -> Option<ComplexityTier> {
        let store = self.store.lock().await;
        let results = store
            .recall(&MemoryQuery {
                text: "outcome:".to_string(),
                limit: 100,
            })
            .await
            .ok()?;

        if results.len() < 5 {
            return None; // insufficient history
        }

        let outcomes: Vec<OutcomeRecord> = results
            .iter()
            .filter_map(|m| OutcomeRecord::from_content(&m.content))
            .collect();

        if outcomes.is_empty() {
            return None;
        }

        // Compute success rate per tier.
        let (mut simple_succ, mut simple_total) = (0u32, 0u32);
        let (mut medium_succ, mut medium_total) = (0u32, 0u32);
        let (mut complex_succ, mut complex_total) = (0u32, 0u32);

        for o in &outcomes {
            let (succ, total) = match o.tier {
                ComplexityTier::Simple => (&mut simple_succ, &mut simple_total),
                ComplexityTier::Medium => (&mut medium_succ, &mut medium_total),
                ComplexityTier::Complex => (&mut complex_succ, &mut complex_total),
            };
            *total += 1;
            if o.success {
                *succ += 1;
            }
        }

        // If Simple tier has > 80% success rate, prefer Simple (saves tokens).
        if simple_total >= 3 && simple_succ as f64 / simple_total as f64 > 0.8 {
            let rule_tier = crate::complexity::ComplexityClassifier::classify(task_summary);
            if rule_tier == ComplexityTier::Medium {
                tracing::info!(
                    simple_success_rate = simple_succ as f64 / simple_total as f64,
                    "adaptive: downgrading Medium → Simple based on history"
                );
                return Some(ComplexityTier::Simple);
            }
        }

        // If Simple tier has < 40% success rate, escalate to Medium.
        if simple_total >= 3 && (simple_succ as f64 / simple_total as f64) < 0.4 {
            let rule_tier = crate::complexity::ComplexityClassifier::classify(task_summary);
            if rule_tier == ComplexityTier::Simple {
                tracing::info!(
                    simple_success_rate = simple_succ as f64 / simple_total as f64,
                    "adaptive: upgrading Simple → Medium based on history"
                );
                return Some(ComplexityTier::Medium);
            }
        }

        None // no override — use rule-based classifier
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_outcome_round_trip() {
        let outcome = OutcomeRecord {
            tier: ComplexityTier::Medium,
            model: "anthropic:claude-sonnet-4-5".to_string(),
            success: true,
            cost_usd: 0.0234,
            total_tokens: 15000,
            task_summary: "fix the login bug".to_string(),
        };

        let content = outcome.to_content();
        let parsed = OutcomeRecord::from_content(&content).unwrap();

        assert_eq!(parsed.tier, ComplexityTier::Medium);
        assert_eq!(parsed.model, "anthropic:claude-sonnet-4-5");
        assert!(parsed.success);
        assert!((parsed.cost_usd - 0.0234).abs() < 0.001);
        assert_eq!(parsed.total_tokens, 15000);
    }

    #[test]
    fn test_outcome_parse_failure() {
        let parsed = OutcomeRecord::from_content("not an outcome");
        assert!(parsed.is_none());
    }
}
