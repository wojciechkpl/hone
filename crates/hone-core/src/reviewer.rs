use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::agent::{Agent, AgentConfig};
use crate::error::AgentError;
use hone_proto::{AgentEvent, TokenUsage};

/// Independent code reviewer that catches bugs before tests run.
///
/// Uses an adversarial system prompt and a higher temperature than the coder
/// agent to avoid confirmation bias (research: ChatDev, CodeR).
pub struct ReviewerAgent {
    agent_config: AgentConfig,
}

/// The outcome of a [`ReviewerAgent::review`] call.
pub struct ReviewResult {
    /// Issues identified by the reviewer.
    pub issues: Vec<ReviewIssue>,
    /// `true` when no `Critical` or `Major` issues were found, or when the
    /// reviewer output could not be parsed (fail-open).
    pub approved: bool,
    /// The raw text response returned by the model.
    pub response: String,
    /// Token usage for this review turn.
    pub usage: TokenUsage,
}

/// A single issue found during review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewIssue {
    pub severity: ReviewSeverity,
    pub description: String,
    pub file: Option<String>,
    pub suggestion: Option<String>,
}

/// Severity classification for a [`ReviewIssue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReviewSeverity {
    Critical,
    Major,
    Minor,
    Suggestion,
}

/// Adversarial system prompt that directs the reviewer to look for problems.
const REVIEWER_SYSTEM_PROMPT: &str = "\
You are a strict code reviewer. Your job is to find bugs, security issues, and quality problems.

Review the following changes and output a JSON array of issues:
[{\"severity\": \"critical|major|minor|suggestion\", \"description\": \"...\", \"file\": \"...\", \"suggestion\": \"...\"}]

Rules:
- Be adversarial. Assume bugs exist until proven otherwise.
- Check for: logic errors, off-by-one, null/None handling, SQL injection, XSS, race conditions.
- If no issues found, output an empty array [].
- Output ONLY JSON, no explanation.";

impl ReviewerAgent {
    /// Create a reviewer from a base `AgentConfig`.
    ///
    /// The reviewer's system prompt and temperature are overridden to inject
    /// adversarial review instructions and increase temperature to 0.7 to
    /// reduce confirmation bias relative to the coder's lower temperature.
    pub fn new(base_config: AgentConfig) -> Self {
        let agent_config = AgentConfig {
            system_prompt: REVIEWER_SYSTEM_PROMPT.to_owned(),
            // Higher temperature than the coder (0.0–0.3) to avoid groupthink.
            temperature: Some(0.7),
            provider: Arc::clone(&base_config.provider),
            tools: hone_tools::ToolRegistry::new(),
            sandbox: Arc::clone(&base_config.sandbox),
            model: base_config.model,
            max_tokens: base_config.max_tokens,
            budget_limit: base_config.budget_limit,
            pricing: base_config.pricing,
            working_dir: base_config.working_dir,
            memory_store: None,
            retry_config: base_config.retry_config,
        };

        Self { agent_config }
    }

    /// Review the provided `context` (typically a diff or file listing) and
    /// return a [`ReviewResult`].
    ///
    /// If the model response cannot be parsed as a JSON array of issues the
    /// call succeeds with `approved = true` (fail-open policy: an unreadable
    /// review should not block progress).
    pub async fn review(
        &mut self,
        context: &str,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<ReviewResult, AgentError> {
        let mut agent = Agent::new(AgentConfig {
            provider: Arc::clone(&self.agent_config.provider),
            tools: hone_tools::ToolRegistry::new(),
            sandbox: Arc::clone(&self.agent_config.sandbox),
            system_prompt: self.agent_config.system_prompt.clone(),
            model: self.agent_config.model.clone(),
            max_tokens: self.agent_config.max_tokens,
            temperature: self.agent_config.temperature,
            budget_limit: self.agent_config.budget_limit,
            pricing: self.agent_config.pricing.clone(),
            working_dir: self.agent_config.working_dir.clone(),
            memory_store: None,
            retry_config: self.agent_config.retry_config.clone(),
        });

        let turn = agent.run_turn(context, events).await?;

        let raw = turn.response.trim();

        // Attempt to parse the model output as a JSON array of issues.
        let (issues, approved) = match serde_json::from_str::<Vec<ReviewIssue>>(raw) {
            Ok(parsed) => {
                // Approve when there are no critical or major issues.
                let blocking = parsed.iter().any(|i| {
                    matches!(i.severity, ReviewSeverity::Critical | ReviewSeverity::Major)
                });
                (parsed, !blocking)
            }
            Err(_) => {
                // Cannot parse — fail-open: assume the code is fine.
                (Vec::new(), true)
            }
        };

        Ok(ReviewResult {
            issues,
            approved,
            response: turn.response,
            usage: turn.usage,
        })
    }
}
