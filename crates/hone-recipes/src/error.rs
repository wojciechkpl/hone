#[derive(Debug, thiserror::Error)]
pub enum RecipeError {
    #[error("YAML parse error: {0}")]
    Parse(#[from] serde_yaml::Error),

    #[error("Schema validation error: {0}")]
    SchemaValidation(String),

    #[error("Step '{step}' failed: {source}")]
    StepFailed {
        step: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("Test command failed after step '{step}': {output}")]
    TestFailed { step: String, output: String },
}
