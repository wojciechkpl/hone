//! Hone eval harness: runs canned tasks against an agent and scores them.

pub mod config;
pub mod report;
pub mod runner;

pub use config::{EvalConfig, EvalConfigError, TaskSpec};
pub use report::{EvalReport, write_json, write_markdown};
pub use runner::{EvalRunner, EvalRunnerError, TaskOutcome, TaskStatus};
