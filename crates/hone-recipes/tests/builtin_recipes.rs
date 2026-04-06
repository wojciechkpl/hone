use hone_recipes::Recipe;

// Built-in recipe sources embedded for testing.
const TDD_FEATURE: &str = include_str!("../recipes/tdd-feature.yaml");
const BUG_FIX: &str = include_str!("../recipes/bug-fix.yaml");
const CODE_REVIEW: &str = include_str!("../recipes/code-review.yaml");
const REFACTOR: &str = include_str!("../recipes/refactor.yaml");

#[test]
fn test_tdd_feature_recipe_parses() {
    let recipe = Recipe::from_yaml(TDD_FEATURE).expect("tdd-feature.yaml should parse");
    assert_eq!(recipe.name, "tdd-feature");
    assert_eq!(recipe.steps.len(), 5, "expected 5 steps in tdd-feature");
}

#[test]
fn test_bug_fix_recipe_parses() {
    let recipe = Recipe::from_yaml(BUG_FIX).expect("bug-fix.yaml should parse");
    assert_eq!(recipe.name, "bug-fix");
    assert_eq!(recipe.steps.len(), 4, "expected 4 steps in bug-fix");
    assert!(
        recipe.test_cmd.is_some(),
        "bug-fix recipe should have a test_cmd"
    );
}

#[test]
fn test_code_review_recipe_parses() {
    let recipe = Recipe::from_yaml(CODE_REVIEW).expect("code-review.yaml should parse");
    assert_eq!(recipe.name, "code-review");
    assert_eq!(recipe.steps.len(), 5, "expected 5 steps in code-review");
}

#[test]
fn test_refactor_recipe_parses() {
    let recipe = Recipe::from_yaml(REFACTOR).expect("refactor.yaml should parse");
    assert_eq!(recipe.name, "refactor");
    assert_eq!(recipe.steps.len(), 4, "expected 4 steps in refactor");
    assert!(
        recipe.test_cmd.is_some(),
        "refactor recipe should have a test_cmd"
    );
}

#[test]
fn test_all_builtin_recipes_valid() {
    let sources = [
        ("tdd-feature", TDD_FEATURE),
        ("bug-fix", BUG_FIX),
        ("code-review", CODE_REVIEW),
        ("refactor", REFACTOR),
    ];

    for (label, src) in &sources {
        let result = Recipe::from_yaml(src);
        assert!(
            result.is_ok(),
            "built-in recipe '{label}' failed to parse: {:?}",
            result.err()
        );

        let recipe = result.unwrap();
        assert!(
            !recipe.steps.is_empty(),
            "built-in recipe '{label}' must have at least one step"
        );
        for (i, step) in recipe.steps.iter().enumerate() {
            assert!(
                !step.name.trim().is_empty(),
                "step[{i}] in '{label}' has empty name"
            );
            assert!(
                !step.prompt.trim().is_empty(),
                "step[{i}] in '{label}' has empty prompt"
            );
        }
    }
}
