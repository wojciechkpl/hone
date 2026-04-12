use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Recipe {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<RecipeStep>,
    pub test_cmd: Option<String>,
}

impl Recipe {
    /// Substitute `{{var}}` placeholders in all text fields with the provided
    /// values.  Placeholders with no matching key are left unchanged.
    pub fn with_variables(mut self, vars: &HashMap<String, String>) -> Self {
        self.steps = self
            .steps
            .into_iter()
            .map(|mut step| {
                step.prompt = substitute_variables(&step.prompt, vars);
                step
            })
            .collect();
        self.test_cmd = self.test_cmd.map(|cmd| substitute_variables(&cmd, vars));
        self
    }
}

/// Replace every `{{key}}` occurrence in `text` with the corresponding value
/// from `vars`.  Unknown placeholders are left as-is.
pub fn substitute_variables(text: &str, vars: &HashMap<String, String>) -> String {
    let mut result = text.to_owned();
    for (key, value) in vars {
        result = result.replace(&format!("{{{{{key}}}}}"), value);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn minimal_recipe() -> Recipe {
        Recipe {
            name: "test".to_owned(),
            description: None,
            steps: vec![RecipeStep {
                name: "step1".to_owned(),
                prompt: "Implement {{feature_name}} using TDD.".to_owned(),
                when: None,
            }],
            test_cmd: Some("{{test_command}}".to_owned()),
        }
    }

    #[test]
    fn substitutes_step_prompt_and_test_cmd() {
        let recipe = minimal_recipe().with_variables(&vars(&[
            ("feature_name", "auth"),
            ("test_command", "cargo test"),
        ]));
        assert_eq!(recipe.steps[0].prompt, "Implement auth using TDD.");
        assert_eq!(recipe.test_cmd.as_deref(), Some("cargo test"));
    }

    #[test]
    fn missing_variable_left_unchanged() {
        // Only feature_name provided; test_command placeholder stays.
        let recipe = minimal_recipe()
            .with_variables(&vars(&[("feature_name", "auth")]));
        assert_eq!(recipe.steps[0].prompt, "Implement auth using TDD.");
        assert_eq!(recipe.test_cmd.as_deref(), Some("{{test_command}}"));
    }

    #[test]
    fn empty_vars_map_leaves_text_unchanged() {
        let recipe = minimal_recipe().with_variables(&HashMap::new());
        assert_eq!(
            recipe.steps[0].prompt,
            "Implement {{feature_name}} using TDD."
        );
        assert_eq!(recipe.test_cmd.as_deref(), Some("{{test_command}}"));
    }

    #[test]
    fn substitute_variables_replaces_all_occurrences() {
        let v = vars(&[("x", "Y")]);
        let result = substitute_variables("{{x}} and {{x}}", &v);
        assert_eq!(result, "Y and Y");
    }

    #[test]
    fn nested_braces_not_confused() {
        // Single braces should not be treated as placeholders.
        let v = vars(&[("x", "Y")]);
        let result = substitute_variables("{x} vs {{x}}", &v);
        assert_eq!(result, "{x} vs Y");
    }

    #[test]
    fn substitution_applied_to_all_steps() {
        let recipe = Recipe {
            name: "multi".to_owned(),
            description: None,
            steps: vec![
                RecipeStep {
                    name: "a".to_owned(),
                    prompt: "do {{thing}}".to_owned(),
                    when: None,
                },
                RecipeStep {
                    name: "b".to_owned(),
                    prompt: "verify {{thing}}".to_owned(),
                    when: None,
                },
            ],
            test_cmd: None,
        }
        .with_variables(&vars(&[("thing", "something")]));

        assert_eq!(recipe.steps[0].prompt, "do something");
        assert_eq!(recipe.steps[1].prompt, "verify something");
    }
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
