use std::sync::{Arc, Mutex};

use hone_recipes::{Recipe, RecipeError, RecipeRunner, RecipeStep};

fn make_recipe(steps: Vec<RecipeStep>, test_cmd: Option<&str>) -> Recipe {
    Recipe {
        name: "test-recipe".to_string(),
        description: None,
        steps,
        test_cmd: test_cmd.map(str::to_string),
    }
}

fn make_step(name: &str, prompt: &str, when: Option<bool>) -> RecipeStep {
    RecipeStep {
        name: name.to_string(),
        prompt: prompt.to_string(),
        when,
    }
}

#[tokio::test]
async fn test_step_with_condition_skips_when_false() {
    let executed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let executed_clone = Arc::clone(&executed);

    let recipe = make_recipe(
        vec![
            make_step("step-a", "prompt a", None),
            make_step("step-b", "prompt b", Some(false)),
            make_step("step-c", "prompt c", None),
        ],
        None,
    );

    let runner = RecipeRunner::new(recipe, move |_idx, step: &RecipeStep| {
        let name = step.name.clone();
        let executed = Arc::clone(&executed_clone);
        async move {
            executed.lock().unwrap().push(name);
            Ok("done".to_string())
        }
    });

    let report = runner.run().await.expect("recipe should succeed");

    let names = executed.lock().unwrap().clone();
    assert_eq!(names, vec!["step-a", "step-c"]);

    let skipped: Vec<_> = report.steps.iter().filter(|s| s.skipped).collect();
    assert_eq!(skipped.len(), 1);
    assert_eq!(skipped[0].step_name, "step-b");
}

#[tokio::test]
async fn test_recipe_runner_executes_steps_in_order() {
    let recorded: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_clone = Arc::clone(&recorded);

    let recipe = make_recipe(
        vec![
            make_step("first", "do first", None),
            make_step("second", "do second", None),
        ],
        None,
    );

    let runner = RecipeRunner::new(recipe, move |_idx, step: &RecipeStep| {
        let name = step.name.clone();
        let recorded = Arc::clone(&recorded_clone);
        async move {
            recorded.lock().unwrap().push(name);
            Ok("ok".to_string())
        }
    });

    runner.run().await.expect("recipe should succeed");

    let names = recorded.lock().unwrap().clone();
    assert_eq!(names, vec!["first", "second"]);
}

#[tokio::test]
async fn test_tdd_recipe_runs_tests_between_steps_success() {
    let recipe = make_recipe(
        vec![
            make_step("step-a", "prompt a", None),
            make_step("step-b", "prompt b", None),
        ],
        Some("true"),
    );

    let runner = RecipeRunner::new(recipe, |_idx, _step: &RecipeStep| async move {
        Ok("done".to_string())
    });

    runner
        .run()
        .await
        .expect("recipe with passing test_cmd should succeed");
}

#[tokio::test]
async fn test_tdd_recipe_runs_tests_between_steps_failure() {
    let recipe = make_recipe(
        vec![
            make_step("step-a", "prompt a", None),
            make_step("step-b", "prompt b", None),
        ],
        Some("false"),
    );

    let runner = RecipeRunner::new(recipe, |_idx, _step: &RecipeStep| async move {
        Ok("done".to_string())
    });

    let result = runner.run().await;
    assert!(result.is_err(), "recipe with failing test_cmd should fail");

    match result.unwrap_err() {
        RecipeError::TestFailed { step, .. } => {
            assert_eq!(step, "step-a", "first step triggers the test failure");
        }
        other => panic!("expected TestFailed, got: {other:?}"),
    }
}
