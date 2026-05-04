use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Clone, serde::Deserialize)]
pub struct EvalConfig {
    pub tasks: Vec<TaskSpec>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct TaskSpec {
    pub id: String,
    pub prompt: String,
    pub success_check: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_working_dir")]
    pub working_dir: String,
    pub budget_usd: Option<f64>,
}

fn default_timeout() -> u64 {
    600
}

fn default_working_dir() -> String {
    ".".into()
}

#[derive(Debug, thiserror::Error)]
pub enum EvalConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("toml parse error: {0}")]
    Toml(String),
    #[error("duplicate task id: {0}")]
    DuplicateId(String),
}

impl EvalConfig {
    pub fn from_toml_str(s: &str) -> Result<Self, EvalConfigError> {
        let config: EvalConfig =
            toml::from_str(s).map_err(|e| EvalConfigError::Toml(e.to_string()))?;
        let mut seen = HashSet::new();
        for task in &config.tasks {
            if !seen.insert(task.id.clone()) {
                return Err(EvalConfigError::DuplicateId(task.id.clone()));
            }
        }
        Ok(config)
    }

    pub fn from_toml_file(path: &Path) -> Result<Self, EvalConfigError> {
        let s = std::fs::read_to_string(path)?;
        Self::from_toml_str(&s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TWO_TASK_TOML: &str = r#"
[[tasks]]
id = "fix-divide-by-zero"
prompt = "Fix the divide-by-zero bug in src/math.rs"
success_check = "cargo test --test math_test"
timeout_secs = 300
working_dir = "."
budget_usd = 5.0

[[tasks]]
id = "add-greeting"
prompt = "Add a greet(name) function to greet.py"
success_check = "python -m pytest tests/test_greet.py"
"#;

    #[test]
    fn from_toml_str_parses_two_tasks() {
        let config = EvalConfig::from_toml_str(TWO_TASK_TOML).expect("should parse");
        assert_eq!(config.tasks.len(), 2);
        assert_eq!(config.tasks[0].id, "fix-divide-by-zero");
        assert_eq!(config.tasks[1].id, "add-greeting");
    }

    #[test]
    fn from_toml_str_default_timeout_is_600() {
        let toml = r#"
[[tasks]]
id = "no-timeout"
prompt = "do something"
success_check = "true"
"#;
        let config = EvalConfig::from_toml_str(toml).expect("should parse");
        assert_eq!(config.tasks[0].timeout_secs, 600);
    }

    #[test]
    fn from_toml_str_default_working_dir_is_dot() {
        let toml = r#"
[[tasks]]
id = "no-wd"
prompt = "do something"
success_check = "true"
"#;
        let config = EvalConfig::from_toml_str(toml).expect("should parse");
        assert_eq!(config.tasks[0].working_dir, ".");
    }

    #[test]
    fn from_toml_str_optional_budget_usd_omitted_is_none() {
        let toml = r#"
[[tasks]]
id = "no-budget"
prompt = "do something"
success_check = "true"
"#;
        let config = EvalConfig::from_toml_str(toml).expect("should parse");
        assert!(config.tasks[0].budget_usd.is_none());
    }

    #[test]
    fn from_toml_str_invalid_toml_errors() {
        let bad = "[[tasks\nthis is not valid toml!!";
        let err = EvalConfig::from_toml_str(bad).expect_err("should fail");
        assert!(matches!(err, EvalConfigError::Toml(_)));
    }

    #[test]
    fn from_toml_str_duplicate_ids_errors() {
        let toml = r#"
[[tasks]]
id = "same"
prompt = "first"
success_check = "true"

[[tasks]]
id = "same"
prompt = "second"
success_check = "false"
"#;
        let err = EvalConfig::from_toml_str(toml).expect_err("should fail on duplicate");
        assert!(matches!(err, EvalConfigError::DuplicateId(id) if id == "same"));
    }
}
