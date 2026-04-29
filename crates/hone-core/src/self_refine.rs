use tokio::sync::mpsc;

use crate::agent::{Agent, AgentHandle};
use crate::error::AgentError;
use hone_proto::{AgentEvent, TokenUsage};

/// Single-agent self-refinement loop: generate → critique → refine.
///
/// Source: Self-Refine (NeurIPS 2023) — +8–15 % code quality with zero extra
/// agents or external test infrastructure.
pub struct SelfRefineLoop {
    max_rounds: u32,
}

/// The outcome of a [`SelfRefineLoop::run`] call.
pub struct RefineResult {
    /// Total rounds executed (1 = no critique needed, n ≥ 2 = n-1 refinements).
    pub rounds: u32,
    /// The last text response produced by the agent.
    pub final_response: String,
    /// Cumulative token usage across all turns in the loop.
    pub usage: TokenUsage,
}

/// Sentinel string the model emits in a critique turn when it is satisfied.
const APPROVED_SENTINEL: &str = "APPROVED";

/// Prompt appended after the initial generation to request a self-critique.
const CRITIQUE_PROMPT: &str = "Review your previous response critically. List any bugs, edge cases you missed, or improvements. \
     If the code is correct, respond with APPROVED.";

/// Prompt appended after a critique to request a refined version.
const REFINE_PROMPT: &str =
    "Based on your critique, improve the code. Address each issue you identified.";

impl SelfRefineLoop {
    /// Create a new loop.
    ///
    /// * `max_rounds` — maximum number of generate+critique+refine cycles.
    ///   A value of 1 means a single generation with no refinement; the
    ///   research sweet-spot is 2–3.
    pub fn new(max_rounds: u32) -> Self {
        Self { max_rounds }
    }

    /// Run the self-refinement loop.
    ///
    /// 1. Agent generates an initial response to `initial_prompt`.
    /// 2. Agent critiques its own output.
    /// 3. If the critique contains `"APPROVED"` → stop early.
    /// 4. Agent refines based on the critique.
    /// 5. Repeat critique+refine until `APPROVED` or `max_rounds` reached.
    pub async fn run(
        &self,
        agent: &mut Agent,
        initial_prompt: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<RefineResult, AgentError> {
        let mut total_usage = TokenUsage::default();
        // ── Round 1: initial generation ─────────────────────────────────────
        let initial = agent.run_turn(initial_prompt, events.clone()).await?;
        total_usage = total_usage + initial.usage;
        let mut final_response = initial.response;

        if self.max_rounds <= 1 {
            return Ok(RefineResult {
                rounds: 1,
                final_response,
                usage: total_usage,
            });
        }

        // ── Rounds 2‥max_rounds: critique → (refine) ────────────────────────
        let mut rounds_completed = 1u32;

        for _ in 2..=self.max_rounds {
            // Critique turn
            let critique = agent.run_turn(CRITIQUE_PROMPT, events.clone()).await?;
            total_usage = total_usage + critique.usage;
            rounds_completed += 1;

            if critique.response.contains(APPROVED_SENTINEL) {
                // The agent is satisfied — return the last generated response.
                break;
            }

            // Refine turn
            let refined = agent.run_turn(REFINE_PROMPT, events.clone()).await?;
            total_usage = total_usage + refined.usage;
            final_response = refined.response.clone();
            rounds_completed += 1;
        }

        Ok(RefineResult {
            rounds: rounds_completed,
            final_response,
            usage: total_usage,
        })
    }

    /// Run the self-refinement loop against an [`AgentHandle`] actor.
    ///
    /// Semantics are identical to [`Self::run`]; each turn is dispatched
    /// through the handle's command channel so the actor serializes them.
    pub async fn run_with_handle(
        &self,
        handle: &AgentHandle,
        initial_prompt: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<RefineResult, AgentError> {
        let mut total_usage = TokenUsage::default();

        let initial = handle.run_turn(initial_prompt, events.clone()).await?;
        total_usage = total_usage + initial.usage;
        let mut final_response = initial.response;

        if self.max_rounds <= 1 {
            return Ok(RefineResult {
                rounds: 1,
                final_response,
                usage: total_usage,
            });
        }

        let mut rounds_completed = 1u32;

        for _ in 2..=self.max_rounds {
            let critique = handle.run_turn(CRITIQUE_PROMPT, events.clone()).await?;
            total_usage = total_usage + critique.usage;
            rounds_completed += 1;

            if critique.response.contains(APPROVED_SENTINEL) {
                break;
            }

            let refined = handle.run_turn(REFINE_PROMPT, events.clone()).await?;
            total_usage = total_usage + refined.usage;
            final_response = refined.response.clone();
            rounds_completed += 1;
        }

        Ok(RefineResult {
            rounds: rounds_completed,
            final_response,
            usage: total_usage,
        })
    }
}
