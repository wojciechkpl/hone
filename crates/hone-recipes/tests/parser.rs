use hone_recipes::{Recipe, RecipeError};

#[test]
fn test_parse_minimal_recipe() {
    let yaml = r#"
name: my-recipe
steps:
  - name: step-one
    prompt: "Do the first thing"
  - name: step-two
    prompt: "Do the second thing"
"#;

    let recipe = Recipe::from_yaml(yaml).expect("should parse valid recipe");
    assert_eq!(recipe.steps.len(), 2);
    assert_eq!(recipe.steps[0].name, "step-one");
    assert_eq!(recipe.steps[1].name, "step-two");
}

#[test]
fn test_invalid_recipe_schema_errors() {
    let yaml = r#"
steps:
  - name: step-one
    prompt: "Do something"
"#;

    let result = Recipe::from_yaml(yaml);
    assert!(
        result.is_err(),
        "expected an error for missing 'name' field"
    );

    match result.unwrap_err() {
        RecipeError::SchemaValidation(_) | RecipeError::Parse(_) => {}
        other => panic!("expected SchemaValidation or Parse error, got: {other:?}"),
    }
}
