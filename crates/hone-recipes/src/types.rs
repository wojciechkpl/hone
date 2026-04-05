#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Recipe {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<RecipeStep>,
    pub test_cmd: Option<String>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RecipeStep {
    pub name: String,
    pub prompt: String,
    pub when: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub step_name: String,
    pub output: String,
    pub skipped: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecipeReport {
    pub recipe_name: String,
    pub steps: Vec<StepResult>,
}

impl RecipeReport {
    pub fn executed_steps(&self) -> impl Iterator<Item = &StepResult> {
        self.steps.iter().filter(|s| !s.skipped)
    }
}
