use std::future::Future;

use tokio::process::Command;
use tracing::debug;

use crate::error::RecipeError;
use crate::types::{Recipe, RecipeReport, RecipeStep, StepResult};

pub struct RecipeRunner<E> {
    recipe: Recipe,
    executor: E,
}

impl<E, Fut> RecipeRunner<E>
where
    E: Fn(usize, &RecipeStep) -> Fut,
    Fut: Future<Output = Result<String, RecipeError>>,
{
    pub fn new(recipe: Recipe, executor: E) -> Self {
        Self { recipe, executor }
    }

    pub async fn run(self) -> Result<RecipeReport, RecipeError> {
        let mut step_results: Vec<StepResult> = Vec::with_capacity(self.recipe.steps.len());

        for (index, step) in self.recipe.steps.iter().enumerate() {
            if step.when == Some(false) {
                debug!(step = %step.name, "skipping step (when: false)");
                step_results.push(StepResult {
                    step_name: step.name.clone(),
                    output: String::new(),
                    skipped: true,
                });
                continue;
            }

            debug!(step = %step.name, "executing step");
            let output = (self.executor)(index, step).await?;

            step_results.push(StepResult {
                step_name: step.name.clone(),
                output,
                skipped: false,
            });

            if let Some(ref test_cmd) = self.recipe.test_cmd {
                run_test_command(test_cmd, &step.name).await?;
            }
        }

        Ok(RecipeReport {
            recipe_name: self.recipe.name.clone(),
            steps: step_results,
        })
    }
}

async fn run_test_command(cmd: &str, step_name: &str) -> Result<(), RecipeError> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .output()
        .await
        .map_err(|e| RecipeError::StepFailed {
            step: step_name.to_string(),
            source: Box::new(e),
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let combined = if stderr.is_empty() { stdout } else { stderr };
        return Err(RecipeError::TestFailed {
            step: step_name.to_string(),
            output: combined,
        });
    }

    Ok(())
}
