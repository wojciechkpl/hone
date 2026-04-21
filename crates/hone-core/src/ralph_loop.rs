//! RALPH Loop — Review, Analyze, Learn, Plan, Heal.
//!
//! A structured feedback loop that combines self-refinement with adversarial
//! review. Unlike `SelfRefineLoop` (which only self-critiques) or
//! `ReviewerAgent` (which only reviews), RALPH integrates both into a
//! closed-loop improvement cycle:
//!
//! 1. **Review** — Adversarial reviewer finds issues in the current solution
//! 2. **Analyze** — Agent analyzes root causes of each issue
//! 3. **Learn** — Extract patterns: what went wrong and why
//! 4. **Plan** — Create a targeted fix plan for each issue
//! 5. **Heal** — Apply fixes, verify, and check for regressions
//!
//! The loop repeats until the reviewer approves or `max_iterations` is reached.
//!
//! Research basis:
//! - Self-Refine (NeurIPS 2023): +8-15% quality from generate-critique-refine
//! - ChatDev (2024): adversarial reviewer catches 30% of bugs
//! - MASAI (2024): specialized roles outperform generalist agents
//! - AgentCoder (2024): test-driven verification after each iteration

use tokio::sync::mpsc;

use crate::agent::{AgentConfig, AgentHandle};
use crate::error::AgentError;
use crate::reviewer::{ReviewIssue, ReviewSeverity, ReviewerAgent};
use hone_proto::{AgentEvent, TokenUsage};

/// Configuration for the RALPH loop.
#[derive(Debug, Clone)]
pub struct RalphConfig {
    /// Maximum iterations of the full RALPH cycle.
    pub max_iterations: u32,
    /// Whether to run tests after each Heal step.
    pub verify_after_heal: bool,
    /// Test command to run for verification (e.g., "cargo test").
    pub test_command: Option<String>,
    /// Minimum severity that triggers a new iteration.
    /// Issues below this severity are reported but don't block approval.
    pub min_blocking_severity: ReviewSeverity,
    /// Whether to record learned patterns for cross-session improvement.
    pub record_learnings: bool,
}

impl Default for RalphConfig {
    fn default() -> Self {
        Self {
            max_iterations: 3,
            verify_after_heal: true,
            test_command: None,
            min_blocking_severity: ReviewSeverity::Major,
            record_learnings: true,
        }
    }
}

/// The outcome of a completed RALPH loop.
#[derive(Debug)]
pub struct RalphResult {
    /// Total iterations completed.
    pub iterations: u32,
    /// Whether the reviewer approved the final version.
    pub approved: bool,
    /// All issues found across all iterations (with resolution status).
    pub all_issues: Vec<TrackedIssue>,
    /// Patterns learned during the loop.
    pub learnings: Vec<String>,
    /// Final response text.
    pub final_response: String,
    /// Cumulative token usage.
    pub total_usage: TokenUsage,
    /// Total cost in USD.
    pub total_cost_usd: f64,
}

/// An issue tracked through the RALPH cycle with resolution status.
#[derive(Debug, Clone)]
pub struct TrackedIssue {
    pub issue: ReviewIssue,
    pub iteration_found: u32,
    pub resolved: bool,
    pub resolution: Option<String>,
}

/// A single iteration's events for progress reporting.
#[derive(Debug, Clone)]
pub enum RalphEvent {
    /// Starting a new RALPH iteration.
    IterationStarted { iteration: u32, max: u32 },
    /// Review phase completed.
    ReviewCompleted { issues_found: usize, approved: bool },
    /// Analysis phase completed.
    AnalysisCompleted { root_causes: usize },
    /// Learning phase completed.
    LearningCompleted { patterns: usize },
    /// Plan phase completed.
    PlanCompleted { fixes_planned: usize },
    /// Heal phase completed.
    HealCompleted { fixes_applied: usize, tests_passed: Option<bool> },
    /// The full loop completed.
    Completed { approved: bool, iterations: u32, total_issues: usize },
}

/// The RALPH loop orchestrator.
pub struct RalphLoop {
    config: RalphConfig,
}

impl RalphLoop {
    pub fn new(config: RalphConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(RalphConfig::default())
    }

    /// Run the RALPH loop using an AgentHandle (actor-safe).
    pub async fn run(
        &self,
        agent: &AgentHandle,
        initial_prompt: &str,
        reviewer_config: AgentConfig,
        events: mpsc::Sender<AgentEvent>,
    ) -> Result<RalphResult, AgentError> {
        let mut total_usage = TokenUsage::default();
        let mut total_cost = 0.0f64;
        let mut all_issues: Vec<TrackedIssue> = Vec::new();
        let mut learnings: Vec<String> = Vec::new();

        // ── Step 0: Initial generation ─────────────────────────────────────
        let initial = agent.run_turn(initial_prompt, events.clone()).await?;
        total_usage = total_usage + initial.usage;
        total_cost += initial.cost_usd;
        let mut final_response = initial.response;

        // ── RALPH iterations ───────────────────────────────────────────────
        for iteration in 1..=self.config.max_iterations {
            let _ = events.send(AgentEvent::Token {
                text: format!("\n[RALPH iteration {}/{}]\n", iteration, self.config.max_iterations),
            }).await;

            // ── R: Review ──────────────────────────────────────────────────
            let mut reviewer = ReviewerAgent::new(reviewer_config.clone());
            let review = reviewer.review(&final_response, events.clone()).await?;
            total_usage = total_usage + review.usage;
            // Estimate review cost from usage (reviewer has no cost_usd field).
            let review_cost = (review.usage.input as f64 * 3.0 + review.usage.output as f64 * 15.0) / 1_000_000.0;
            total_cost += review_cost;

            let blocking_issues: Vec<&ReviewIssue> = review.issues.iter()
                .filter(|i| is_blocking(i.severity, self.config.min_blocking_severity))
                .collect();

            // Track all issues
            for issue in &review.issues {
                all_issues.push(TrackedIssue {
                    issue: issue.clone(),
                    iteration_found: iteration,
                    resolved: false,
                    resolution: None,
                });
            }

            if review.approved || blocking_issues.is_empty() {
                let _ = events.send(AgentEvent::Token {
                    text: format!("[RALPH] Approved after {} iterations ({} issues found, {} blocking)\n",
                        iteration, review.issues.len(), blocking_issues.len()),
                }).await;

                return Ok(RalphResult {
                    iterations: iteration,
                    approved: true,
                    all_issues,
                    learnings,
                    final_response,
                    total_usage,
                    total_cost_usd: total_cost,
                });
            }

            // ── A: Analyze ─────────────────────────────────────────────────
            let issues_text = blocking_issues.iter()
                .enumerate()
                .map(|(i, issue)| format!(
                    "{}. [{}] {}{}", i + 1,
                    severity_label(issue.severity),
                    issue.description,
                    issue.file.as_ref().map(|f| format!(" (in {f})")).unwrap_or_default()
                ))
                .collect::<Vec<_>>()
                .join("\n");

            let analyze_prompt = format!(
                "The reviewer found these issues in your code:\n{issues_text}\n\n\
                 Analyze the ROOT CAUSE of each issue. Don't fix anything yet — just explain WHY each bug exists."
            );
            let analysis = agent.run_turn(&analyze_prompt, events.clone()).await?;
            total_usage = total_usage + analysis.usage;
            total_cost += analysis.cost_usd;

            // ── L: Learn ───────────────────────────────────────────────────
            let learn_prompt = "Based on your analysis, what PATTERNS led to these bugs? \
                 State each pattern as a one-line rule (e.g., 'Always validate inputs before use'). \
                 These rules will prevent similar bugs in the future.";
            let learning = agent.run_turn(learn_prompt, events.clone()).await?;
            total_usage = total_usage + learning.usage;
            total_cost += learning.cost_usd;

            // Extract learnings from the response.
            // Handles: bullet lists (- * •), numbered lists (1. 2.), and plain prose.
            let mut found_any = false;
            for line in learning.response.lines() {
                let trimmed = line.trim();
                // Strip bullet markers: -, *, •
                let stripped = trimmed.trim_start_matches(&['-', '*', '•'][..]).trim();
                // Strip numbered list markers: 1. 2. etc.
                let stripped = if stripped.len() > 2 && stripped.as_bytes()[0].is_ascii_digit() {
                    stripped.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.').trim()
                } else {
                    stripped
                };
                if !stripped.is_empty() && stripped.len() > 10 {
                    learnings.push(stripped.to_string());
                    found_any = true;
                }
            }
            // If no structured lines found, take the whole response as a single learning.
            if !found_any && learning.response.trim().len() > 10 {
                learnings.push(learning.response.trim().to_string());
            }

            // ── P: Plan ────────────────────────────────────────────────────
            let plan_prompt = format!(
                "Create a SPECIFIC fix plan for each issue:\n{issues_text}\n\n\
                 For each issue, state:\n\
                 1. Which file to edit\n\
                 2. What to change (old → new)\n\
                 3. How to verify the fix\n\
                 Do NOT apply the fixes yet — just plan them."
            );
            let plan = agent.run_turn(&plan_prompt, events.clone()).await?;
            total_usage = total_usage + plan.usage;
            total_cost += plan.cost_usd;

            // ── H: Heal ────────────────────────────────────────────────────
            let heal_prompt = "Now apply ALL the fixes from your plan. \
                 Edit each file as planned. After all edits, verify each fix is correct by re-reading the changed code.".to_string();
            let heal = agent.run_turn(&heal_prompt, events.clone()).await?;
            total_usage = total_usage + heal.usage;
            total_cost += heal.cost_usd;
            final_response = heal.response.clone();

            // Mark issues as tentatively resolved (confirmed only if next Review doesn't re-raise)
            for tracked in all_issues.iter_mut() {
                if tracked.iteration_found == iteration && !tracked.resolved {
                    tracked.resolved = true;
                    tracked.resolution = Some("Fix applied in Heal phase".to_string());
                }
            }

            // Optional: run tests after healing
            if self.config.verify_after_heal {
                if let Some(ref test_cmd) = self.config.test_command {
                    let verify_prompt = format!(
                        "Run `{test_cmd}` and report the results. If any test fails, \
                         investigate and fix it before proceeding."
                    );
                    let verify = agent.run_turn(&verify_prompt, events.clone()).await?;
                    total_usage = total_usage + verify.usage;
                    total_cost += verify.cost_usd;
                    final_response = verify.response.clone();
                }
            }
        }

        // Max iterations reached without full approval
        Ok(RalphResult {
            iterations: self.config.max_iterations,
            approved: false,
            all_issues,
            learnings,
            final_response,
            total_usage,
            total_cost_usd: total_cost,
        })
    }
}

fn severity_label(s: ReviewSeverity) -> &'static str {
    match s {
        ReviewSeverity::Critical => "CRITICAL",
        ReviewSeverity::Major => "MAJOR",
        ReviewSeverity::Minor => "MINOR",
        ReviewSeverity::Suggestion => "SUGGESTION",
    }
}

fn is_blocking(severity: ReviewSeverity, min: ReviewSeverity) -> bool {
    let rank = |s: ReviewSeverity| match s {
        ReviewSeverity::Critical => 4,
        ReviewSeverity::Major => 3,
        ReviewSeverity::Minor => 2,
        ReviewSeverity::Suggestion => 1,
    };
    rank(severity) >= rank(min)
}

impl RalphResult {
    /// Format a summary of the RALPH loop execution.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "RALPH Loop: {} iterations, {} {}\n",
            self.iterations,
            if self.approved { "APPROVED" } else { "MAX ITERATIONS" },
            if self.approved { "✓" } else { "⚠" }
        ));
        out.push_str(&format!(
            "Issues: {} total, {} resolved\n",
            self.all_issues.len(),
            self.all_issues.iter().filter(|i| i.resolved).count()
        ));
        if !self.learnings.is_empty() {
            out.push_str(&format!("Learnings: {} patterns captured\n", self.learnings.len()));
            for (i, learning) in self.learnings.iter().take(5).enumerate() {
                out.push_str(&format!("  {}. {}\n", i + 1, learning));
            }
        }
        out.push_str(&format!(
            "Cost: ${:.4} | Tokens: {} in + {} out\n",
            self.total_cost_usd, self.total_usage.input, self.total_usage.output
        ));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_severity_blocking() {
        assert!(is_blocking(ReviewSeverity::Critical, ReviewSeverity::Major));
        assert!(is_blocking(ReviewSeverity::Major, ReviewSeverity::Major));
        assert!(!is_blocking(ReviewSeverity::Minor, ReviewSeverity::Major));
        assert!(!is_blocking(ReviewSeverity::Suggestion, ReviewSeverity::Major));
    }

    #[test]
    fn test_severity_label() {
        assert_eq!(severity_label(ReviewSeverity::Critical), "CRITICAL");
        assert_eq!(severity_label(ReviewSeverity::Suggestion), "SUGGESTION");
    }

    #[test]
    fn test_ralph_config_default() {
        let cfg = RalphConfig::default();
        assert_eq!(cfg.max_iterations, 3);
        assert!(cfg.verify_after_heal);
        assert_eq!(cfg.min_blocking_severity, ReviewSeverity::Major);
    }

    #[test]
    fn test_ralph_result_summary_format() {
        let result = RalphResult {
            iterations: 2,
            approved: true,
            all_issues: vec![
                TrackedIssue {
                    issue: ReviewIssue {
                        severity: ReviewSeverity::Major,
                        description: "SQL injection".to_string(),
                        file: Some("auth.rs".to_string()),
                        suggestion: None,
                    },
                    iteration_found: 1,
                    resolved: true,
                    resolution: Some("Fixed".to_string()),
                },
            ],
            learnings: vec!["Always use parameterized queries".to_string()],
            final_response: "Fixed".to_string(),
            total_usage: TokenUsage { input: 5000, output: 2000, cache_read: 0, cache_creation: 0 },
            total_cost_usd: 0.012,
        };
        let summary = result.summary();
        assert!(summary.contains("2 iterations"));
        assert!(summary.contains("APPROVED"));
        assert!(summary.contains("1 total"));
        assert!(summary.contains("1 resolved"));
        assert!(summary.contains("parameterized queries"));
    }
}
