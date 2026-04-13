//! Persistent plan storage for multi-agent orchestration.
//!
//! When the orchestrator decomposes a task into subtasks, the plan is
//! written to a markdown file (default: `.hone/plan.md`) that the agent
//! can read and update as work progresses.

use std::path::{Path, PathBuf};
use tokio::fs;

use crate::error::AgentError;
use crate::task_plan::{TaskPlan, WorkerResult, WorkerStatus};

/// Location where plans are persisted.
pub struct PlanStore {
    path: PathBuf,
}

impl PlanStore {
    /// Create a store writing to the given path. Typically `.hone/plan.md`
    /// inside the working directory.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Create a store at the conventional location inside working_dir.
    pub fn default_for(working_dir: &Path) -> Self {
        Self::new(working_dir.join(".hone/plan.md"))
    }

    /// Write the initial plan to disk (or update it with new status).
    pub async fn write_plan(
        &self,
        plan: &TaskPlan,
        results: &[WorkerResult],
    ) -> Result<(), AgentError> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| AgentError::Tool {
                name: "plan_store".to_string(),
                source: hone_tools::ToolError::IoError(e),
            })?;
        }

        let content = render_plan_markdown(plan, results);
        fs::write(&self.path, content).await.map_err(|e| AgentError::Tool {
            name: "plan_store".to_string(),
            source: hone_tools::ToolError::IoError(e),
        })?;
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Render a plan as a markdown document with task status indicators.
fn render_plan_markdown(plan: &TaskPlan, results: &[WorkerResult]) -> String {
    use std::collections::HashMap;
    let status_by_id: HashMap<&str, &WorkerStatus> = results
        .iter()
        .map(|r| (r.task_id.as_str(), &r.status))
        .collect();

    let mut out = String::new();
    out.push_str("# Task Plan\n\n");
    out.push_str(&format!("**Goal:** {}\n\n", plan.goal));
    out.push_str(&format!("**Total subtasks:** {}\n\n", plan.subtasks.len()));
    out.push_str("## Subtasks\n\n");

    for task in &plan.subtasks {
        let status = status_by_id.get(task.id.as_str());
        let marker = match status {
            Some(WorkerStatus::Completed) => "[x]",
            Some(WorkerStatus::Failed(_)) => "[!]",
            Some(WorkerStatus::TimedOut(_)) => "[T]",
            None => "[ ]",
        };
        out.push_str(&format!("- {marker} **{}** (agent: `{}`)\n", task.id, task.agent));
        out.push_str(&format!("  - {}\n", task.description));
        if !task.depends_on.is_empty() {
            out.push_str(&format!("  - depends on: {:?}\n", task.depends_on));
        }
        if !task.acceptance_tests.is_empty() {
            out.push_str("  - acceptance criteria:\n");
            for (i, test) in task.acceptance_tests.iter().enumerate() {
                out.push_str(&format!("    {}. {}\n", i + 1, test));
            }
        }
        match status {
            Some(WorkerStatus::Failed(reason)) => {
                out.push_str(&format!("  - FAILED: {reason}\n"));
            }
            Some(WorkerStatus::TimedOut(reason)) => {
                out.push_str(&format!("  - TIMED OUT: {reason}\n"));
            }
            _ => {}
        }
    }

    out.push_str("\n## Status\n\n");
    let completed = results.iter().filter(|r| r.status == WorkerStatus::Completed).count();
    let failed = results.iter().filter(|r| matches!(r.status, WorkerStatus::Failed(_))).count();
    let timed_out = results
        .iter()
        .filter(|r| matches!(r.status, WorkerStatus::TimedOut(_)))
        .count();
    out.push_str(&format!("- Completed: {completed}/{}\n", plan.subtasks.len()));
    if failed > 0 {
        out.push_str(&format!("- Failed: {failed}\n"));
    }
    if timed_out > 0 {
        out.push_str(&format!("- Timed out: {timed_out}\n"));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_plan::{SubTask, TaskPlan};
    use hone_proto::TokenUsage;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_write_plan_creates_file() {
        let dir = tempdir().unwrap();
        let store = PlanStore::new(dir.path().join(".hone/plan.md"));
        let plan = TaskPlan {
            goal: "test goal".into(),
            subtasks: vec![SubTask {
                id: "1".into(),
                description: "do a thing".into(),
                agent: "rust-expert".into(),
                context: String::new(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
            }],
        };
        store.write_plan(&plan, &[]).await.unwrap();
        assert!(store.path().exists());
        let content = std::fs::read_to_string(store.path()).unwrap();
        assert!(content.contains("test goal"));
        assert!(content.contains("[ ]"));
    }

    #[tokio::test]
    async fn test_write_plan_shows_completed_status() {
        let dir = tempdir().unwrap();
        let store = PlanStore::default_for(dir.path());
        let plan = TaskPlan {
            goal: "x".into(),
            subtasks: vec![SubTask {
                id: "1".into(),
                description: "d".into(),
                agent: "rust-expert".into(),
                context: String::new(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
            }],
        };
        let results = vec![WorkerResult {
            task_id: "1".into(),
            agent: "rust-expert".into(),
            status: WorkerStatus::Completed,
            response: "ok".into(),
            usage: TokenUsage::default(),
            cost_usd: 0.0,
        }];
        store.write_plan(&plan, &results).await.unwrap();
        let content = std::fs::read_to_string(store.path()).unwrap();
        assert!(content.contains("[x]"));
        assert!(content.contains("Completed: 1/1"));
    }

    #[tokio::test]
    async fn test_failed_status_rendered() {
        let dir = tempdir().unwrap();
        let store = PlanStore::default_for(dir.path());
        let plan = TaskPlan {
            goal: "x".into(),
            subtasks: vec![SubTask {
                id: "1".into(),
                description: "d".into(),
                agent: "rust-expert".into(),
                context: String::new(),
                depends_on: vec![],
                acceptance_tests: vec![],
                budget_usd: None,
                scope: vec![],
            }],
        };
        let results = vec![WorkerResult {
            task_id: "1".into(),
            agent: "rust-expert".into(),
            status: WorkerStatus::Failed("boom".into()),
            response: String::new(),
            usage: TokenUsage::default(),
            cost_usd: 0.0,
        }];
        store.write_plan(&plan, &results).await.unwrap();
        let content = std::fs::read_to_string(store.path()).unwrap();
        assert!(content.contains("[!]"));
        assert!(content.contains("FAILED: boom"));
        assert!(content.contains("Failed: 1"));
    }
}
