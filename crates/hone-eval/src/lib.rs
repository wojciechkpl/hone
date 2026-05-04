//! Hone eval harness: runs canned tasks against an agent and scores them.

pub mod bisect;
pub mod config;
pub mod history;
pub mod report;
pub mod runner;

pub use bisect::{AttributionCandidate, RegressionBisector, RegressionReport};
pub use config::{EvalConfig, EvalConfigError, TaskSpec};
pub use history::{EvalHistoryError, EvalRunRecord, EvalRunStore, RunSummary};
pub use report::{EvalReport, write_json, write_markdown};
pub use runner::{EvalRunner, EvalRunnerError, TaskOutcome, TaskStatus};
