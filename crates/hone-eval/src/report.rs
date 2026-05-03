use std::path::Path;

use crate::runner::{TaskOutcome, TaskStatus};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EvalReport {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub timeout: usize,
    pub agent_error: usize,
    pub total_cost_usd: f64,
    pub total_tokens_input: u32,
    pub total_tokens_output: u32,
    pub outcomes: Vec<TaskOutcome>,
}

impl EvalReport {
    pub fn from_outcomes(outcomes: Vec<TaskOutcome>) -> Self {
        let total = outcomes.len();
        let passed = outcomes
            .iter()
            .filter(|o| o.status == TaskStatus::Pass)
            .count();
        let failed = outcomes
            .iter()
            .filter(|o| o.status == TaskStatus::Fail)
            .count();
        let timeout = outcomes
            .iter()
            .filter(|o| o.status == TaskStatus::Timeout)
            .count();
        let agent_error = outcomes
            .iter()
            .filter(|o| o.status == TaskStatus::AgentError)
            .count();
        let total_cost_usd = outcomes.iter().map(|o| o.cost_usd).sum();
        let total_tokens_input = outcomes.iter().map(|o| o.tokens_input).sum();
        let total_tokens_output = outcomes.iter().map(|o| o.tokens_output).sum();

        Self {
            total,
            passed,
            failed,
            timeout,
            agent_error,
            total_cost_usd,
            total_tokens_input,
            total_tokens_output,
            outcomes,
        }
    }

    pub fn pass_rate(&self) -> f64 {
        if self.total == 0 {
            return 0.0;
        }
        self.passed as f64 / self.total as f64
    }

    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str("# Hone Eval Report\n\n");
        md.push_str(&format!(
            "**Summary**: {}/{} passed ({:.0}%) | cost ${:.4} | {} input tokens | {} output tokens\n\n",
            self.passed,
            self.total,
            self.pass_rate() * 100.0,
            self.total_cost_usd,
            self.total_tokens_input,
            self.total_tokens_output,
        ));
        md.push_str("## Tasks\n\n");
        md.push_str("| id | status | duration_ms | cost_usd |\n");
        md.push_str("|----|--------|-------------|----------|\n");
        for o in &self.outcomes {
            let status_str = match o.status {
                TaskStatus::Pass => "PASS",
                TaskStatus::Fail => "FAIL",
                TaskStatus::Timeout => "TIMEOUT",
                TaskStatus::AgentError => "AGENT_ERROR",
            };
            md.push_str(&format!(
                "| {} | {} | {} | {:.4} |\n",
                o.id, status_str, o.duration_ms, o.cost_usd
            ));
        }
        md
    }
}

pub fn write_json(report: &EvalReport, path: &Path) -> std::io::Result<()> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, json)
}

pub fn write_markdown(report: &EvalReport, path: &Path) -> std::io::Result<()> {
    std::fs::write(path, report.to_markdown())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::{TaskOutcome, TaskStatus};

    fn make_outcome(id: &str, status: TaskStatus, cost: f64, tin: u32, tout: u32) -> TaskOutcome {
        TaskOutcome {
            id: id.to_owned(),
            status,
            duration_ms: 100,
            tokens_input: tin,
            tokens_output: tout,
            cost_usd: cost,
            success_check_stdout: String::new(),
            success_check_stderr: String::new(),
            success_check_exit_code: Some(0),
        }
    }

    #[test]
    fn from_outcomes_aggregates_counts() {
        let outcomes = vec![
            make_outcome("a", TaskStatus::Pass, 0.01, 10, 5),
            make_outcome("b", TaskStatus::Fail, 0.02, 20, 8),
            make_outcome("c", TaskStatus::Timeout, 0.0, 0, 0),
            make_outcome("d", TaskStatus::AgentError, 0.0, 0, 0),
            make_outcome("e", TaskStatus::Pass, 0.03, 30, 12),
        ];
        let report = EvalReport::from_outcomes(outcomes);
        assert_eq!(report.total, 5);
        assert_eq!(report.passed, 2);
        assert_eq!(report.failed, 1);
        assert_eq!(report.timeout, 1);
        assert_eq!(report.agent_error, 1);
        assert!((report.total_cost_usd - 0.06).abs() < 1e-9);
        assert_eq!(report.total_tokens_input, 60);
        assert_eq!(report.total_tokens_output, 25);
    }

    #[test]
    fn pass_rate_with_3_passes_2_fails_returns_0_6() {
        let outcomes = vec![
            make_outcome("a", TaskStatus::Pass, 0.0, 0, 0),
            make_outcome("b", TaskStatus::Pass, 0.0, 0, 0),
            make_outcome("c", TaskStatus::Pass, 0.0, 0, 0),
            make_outcome("d", TaskStatus::Fail, 0.0, 0, 0),
            make_outcome("e", TaskStatus::Fail, 0.0, 0, 0),
        ];
        let report = EvalReport::from_outcomes(outcomes);
        assert!((report.pass_rate() - 0.6).abs() < 1e-9);
    }

    #[test]
    fn pass_rate_with_zero_outcomes_returns_zero() {
        let report = EvalReport::from_outcomes(vec![]);
        assert_eq!(report.pass_rate(), 0.0);
    }

    #[test]
    fn to_markdown_includes_summary_line() {
        let outcomes = vec![
            make_outcome("a", TaskStatus::Pass, 0.01, 10, 5),
            make_outcome("b", TaskStatus::Fail, 0.02, 20, 8),
        ];
        let report = EvalReport::from_outcomes(outcomes);
        let md = report.to_markdown();
        assert!(md.contains("1/2 passed"), "expected '1/2 passed' in: {md}");
    }

    #[test]
    fn to_markdown_includes_per_task_status() {
        let outcomes = vec![
            make_outcome("task-pass", TaskStatus::Pass, 0.0, 0, 0),
            make_outcome("task-fail", TaskStatus::Fail, 0.0, 0, 0),
        ];
        let report = EvalReport::from_outcomes(outcomes);
        let md = report.to_markdown();
        assert!(md.contains("task-pass"), "missing task-pass row");
        assert!(md.contains("task-fail"), "missing task-fail row");
        assert!(md.contains("PASS"), "missing PASS status");
        assert!(md.contains("FAIL"), "missing FAIL status");
    }

    #[test]
    fn write_json_round_trips() {
        let outcomes = vec![
            make_outcome("r1", TaskStatus::Pass, 0.05, 100, 50),
            make_outcome("r2", TaskStatus::Fail, 0.01, 20, 10),
        ];
        let original = EvalReport::from_outcomes(outcomes);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("report.json");
        write_json(&original, &path).expect("write_json");

        let raw = std::fs::read_to_string(&path).expect("read back");
        let decoded: EvalReport = serde_json::from_str(&raw).expect("deserialize");
        assert_eq!(decoded.total, original.total);
        assert_eq!(decoded.passed, original.passed);
        assert_eq!(decoded.outcomes.len(), original.outcomes.len());
        assert_eq!(decoded.outcomes[0].id, "r1");
    }

    #[test]
    fn write_markdown_creates_file_with_expected_substrings() {
        let outcomes = vec![make_outcome("task-x", TaskStatus::Pass, 0.007, 50, 20)];
        let report = EvalReport::from_outcomes(outcomes);
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("report.md");
        write_markdown(&report, &path).expect("write_markdown");

        let content = std::fs::read_to_string(&path).expect("read back");
        assert!(content.contains("Hone Eval Report"), "missing heading");
        assert!(content.contains("task-x"), "missing task id");
        assert!(content.contains("PASS"), "missing status");
    }
}
