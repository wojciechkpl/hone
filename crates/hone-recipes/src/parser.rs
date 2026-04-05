use crate::error::RecipeError;
use crate::types::Recipe;

impl Recipe {
    pub fn from_yaml(s: &str) -> Result<Recipe, RecipeError> {
        let recipe: Recipe = serde_yaml::from_str(s)?;

        if recipe.name.trim().is_empty() {
            return Err(RecipeError::SchemaValidation(
                "recipe 'name' must not be empty".to_string(),
            ));
        }

        if recipe.steps.is_empty() {
            return Err(RecipeError::SchemaValidation(
                "recipe must have at least one step".to_string(),
            ));
        }

        for (i, step) in recipe.steps.iter().enumerate() {
            if step.name.trim().is_empty() {
                return Err(RecipeError::SchemaValidation(format!(
                    "step[{i}] 'name' must not be empty"
                )));
            }
            if step.prompt.trim().is_empty() {
                return Err(RecipeError::SchemaValidation(format!(
                    "step[{i}] 'prompt' must not be empty"
                )));
            }
        }

        Ok(recipe)
    }
}
