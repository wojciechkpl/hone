/// A single subtask entry shown in the plan preview overlay.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanPreviewSubtask {
    pub id: String,
    pub agent: String,
    pub description: String,
}

/// Full state for the plan-preview overlay.
///
/// Constructed via [`PlanPreviewState::from_decompose`] after the lead agent
/// has decomposed a prompt into a [`hone_core::TaskPlan`].
#[derive(Debug, Clone)]
pub struct PlanPreviewState {
    /// The original user prompt that was decomposed.
    pub prompt: String,
    /// The plan goal forwarded from [`hone_core::TaskPlan::goal`].
    pub goal: String,
    /// Subtasks extracted from the plan, with only the display-relevant fields.
    pub subtasks: Vec<PlanPreviewSubtask>,
    /// Cost incurred by the lead agent to produce the decomposition.
    pub lead_cost_usd: f64,
    /// Projected total cost: `lead_cost_usd + N * DEFAULT_WORKER_COST_USD`.
    pub estimated_total_usd: f64,
}

impl PlanPreviewState {
    /// Build a [`PlanPreviewState`] from a completed decomposition.
    ///
    /// `lead_cost_usd` is the amount already spent by the lead agent.
    /// Worker cost is projected using [`hone_core::DEFAULT_WORKER_COST_USD`].
    pub fn from_decompose(prompt: &str, plan: hone_core::TaskPlan, lead_cost_usd: f64) -> Self {
        let estimated_total_usd =
            lead_cost_usd + plan.subtasks.len() as f64 * hone_core::DEFAULT_WORKER_COST_USD;
        let subtasks = plan
            .subtasks
            .into_iter()
            .map(|s| PlanPreviewSubtask {
                id: s.id,
                agent: s.agent,
                description: s.description,
            })
            .collect();
        Self {
            prompt: prompt.to_string(),
            goal: plan.goal,
            subtasks,
            lead_cost_usd,
            estimated_total_usd,
        }
    }
}

/// User action chosen from the plan-preview overlay.
#[derive(Debug, Clone, PartialEq)]
pub enum PlanAction {
    /// Execute the plan as shown.
    Run,
    /// Return the prompt to the input box so the user can edit it.
    Edit,
    /// Dismiss the overlay without doing anything.
    Cancel,
}
