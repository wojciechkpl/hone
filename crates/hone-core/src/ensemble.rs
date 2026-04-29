//! Agent ensemble with ranker selection (MASAI Ranker pattern).
//!
//! Runs N candidate agents in parallel, collects their responses, then asks a
//! dedicated ranker agent to pick the best one.  Based on the MASAI approach
//! (arXiv 2406.11638) which reports 5-8% accuracy improvements via candidate
//! selection.

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::agent::{Agent, AgentConfig};
use crate::error::AgentError;
use crate::turn::TurnResult;
use hone_proto::AgentEvent;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// An ensemble that generates N candidate responses in parallel and selects
/// the best one using a ranker agent.
///
/// Construct with a prototype [`AgentConfig`] and a list of temperatures —
/// one temperature per candidate slot.  The ranker always runs at temperature
/// 0.0 for deterministic selection.
pub struct AgentEnsemble {
    /// Base configuration cloned once per candidate.
    prototype: AgentConfig,
    /// One temperature per candidate slot.  `len()` determines how many
    /// candidates are spawned.
    temperatures: Vec<f32>,
}

/// The result of a completed ensemble run.
#[derive(Debug, Clone)]
pub struct EnsembleResult {
    /// The candidate selected by the ranker.
    pub winner: TurnResult,
    /// Zero-based index into `all_candidates` of the winning candidate.
    pub winner_index: usize,
    /// All successfully completed candidate responses (in collection order,
    /// which may differ from the slot order due to parallel scheduling).
    pub all_candidates: Vec<TurnResult>,
    /// The raw text emitted by the ranker agent.
    pub ranker_response: String,
    /// Total USD cost across all candidates and the ranker.
    pub total_cost_usd: f64,
}

// ---------------------------------------------------------------------------
// Implementation
// ---------------------------------------------------------------------------

impl AgentEnsemble {
    /// Create an ensemble.
    ///
    /// * `prototype` — base config cloned for every candidate and the ranker.
    /// * `temperatures` — temperature for each candidate slot; determines
    ///   how many candidates are run.
    pub fn new(prototype: AgentConfig, temperatures: Vec<f32>) -> Self {
        Self {
            prototype,
            temperatures,
        }
    }

    /// Run all candidates in parallel, then ask the ranker to pick the best.
    ///
    /// Events from every candidate are forwarded onto `events` as they arrive.
    /// Ranker events are consumed internally (they are not forwarded).
    pub async fn run(
        &self,
        prompt: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<EnsembleResult, AgentError> {
        let n = self.temperatures.len();

        // ----------------------------------------------------------------
        // 1. Spawn all candidates concurrently.
        // ----------------------------------------------------------------
        let mut join_set: JoinSet<(usize, Result<TurnResult, AgentError>)> = JoinSet::new();

        for (i, &temp) in self.temperatures.iter().enumerate() {
            let mut config = self.prototype.clone();
            config.temperature = Some(temp);
            let prompt_owned = prompt.to_owned();
            let fwd_tx = events.clone();

            join_set.spawn(async move {
                let mut agent = Agent::new(config);
                let (cand_tx, mut cand_rx) = mpsc::channel(256);

                // Forward candidate events to the main channel.
                let forwarder = tokio::spawn(async move {
                    while let Some(ev) = cand_rx.recv().await {
                        let _ = fwd_tx.send(ev).await;
                    }
                });

                let result = agent.run_turn(&prompt_owned, cand_tx).await;
                let _ = forwarder.await;
                (i, result)
            });
        }

        // ----------------------------------------------------------------
        // 2. Collect successful candidates; preserve original slot order
        //    so the ranker's "Candidate N" numbering is stable.
        // ----------------------------------------------------------------
        let mut slot_results: Vec<Option<TurnResult>> = (0..n).map(|_| None).collect();

        while let Some(join_outcome) = join_set.join_next().await {
            if let Ok((slot, Ok(result))) = join_outcome {
                if slot < slot_results.len() {
                    slot_results[slot] = Some(result);
                }
            }
            // Panicked tasks or Err results are silently skipped so the
            // ensemble can still produce an answer from the remaining slots.
        }

        // Flatten, preserving slot order.
        let candidates: Vec<TurnResult> = slot_results.into_iter().flatten().collect();

        if candidates.is_empty() {
            return Err(AgentError::Cancelled);
        }

        // ----------------------------------------------------------------
        // 3. Build the ranker prompt.
        // ----------------------------------------------------------------
        let mut ranker_prompt = format!("Original task:\n{prompt}\n\nCandidate responses:\n\n");
        for (idx, cand) in candidates.iter().enumerate() {
            ranker_prompt.push_str(&format!(
                "=== Candidate {} ===\n{}\n\n",
                idx + 1,
                cand.response
            ));
        }
        ranker_prompt.push_str(
            "Pick the best candidate. Respond with ONLY the number \
             (1, 2, 3, etc.) of the best candidate. Do not explain.",
        );

        // ----------------------------------------------------------------
        // 4. Run the ranker; collect its token stream internally.
        // ----------------------------------------------------------------
        let mut ranker_config = self.prototype.clone();
        ranker_config.temperature = Some(0.0);

        let mut ranker = Agent::new(ranker_config);
        let (ranker_tx, mut ranker_rx) = mpsc::channel(256);

        // Collect tokens emitted by the ranker.
        let collector = tokio::spawn(async move {
            let mut text = String::new();
            while let Some(ev) = ranker_rx.recv().await {
                if let AgentEvent::Token { text: t } = ev {
                    text.push_str(&t);
                }
            }
            text
        });

        let ranker_result = ranker.run_turn(&ranker_prompt, ranker_tx).await?;
        let ranker_text = collector.await.unwrap_or_default();

        // ----------------------------------------------------------------
        // 5. Parse the winner index (1-based → 0-based).
        // ----------------------------------------------------------------
        let winner_index: usize = ranker_text
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse::<usize>()
            .ok()
            .and_then(|n| n.checked_sub(1))
            .filter(|&i| i < candidates.len())
            .unwrap_or(0); // fallback to first candidate

        let winner = candidates[winner_index].clone();
        let total_cost =
            candidates.iter().map(|c| c.cost_usd).sum::<f64>() + ranker_result.cost_usd;

        Ok(EnsembleResult {
            winner,
            winner_index,
            all_candidates: candidates,
            ranker_response: ranker_text,
            total_cost_usd: total_cost,
        })
    }
}
