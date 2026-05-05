use std::path::{Path, PathBuf};

use anyhow::Result;
use hone_spec::{DefaultSpecGenerator, Spec, SpecGenerator, SpecStore};

// Re-export DefaultSpecGenerator so main.rs can refer to it via spec_cmd::.
pub use hone_spec::DefaultSpecGenerator as SpecGeneratorImpl;

/// Output of `cmd_spec` — what got created on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct SpecCommandOutput {
    /// `<project_root>/.hone/specs/<slug>/`
    pub spec_dir: PathBuf,
    pub slug: String,
    pub goal: String,
    /// Number of tasks parsed from tasks.md after generation.
    pub task_count: usize,
}

/// Drive a SpecGenerator + SpecStore for one goal. The provider+model are
/// provided externally so tests can pass a `RecordingSpecGenerator`; production
/// callers build a `DefaultSpecGenerator` from the wizard config.
pub async fn cmd_spec(
    goal: &str,
    project_root: &Path,
    generator: &dyn SpecGenerator,
) -> Result<SpecCommandOutput, SpecCommandError> {
    // Empty / whitespace-only goal must fail before any generator call.
    if goal.trim().is_empty() {
        return Err(SpecCommandError::EmptyGoal);
    }

    let slug = DefaultSpecGenerator::slugify_goal(goal);

    // Detect pre-existing spec directory before touching the generator.
    let store = SpecStore::default_for(project_root);
    let spec_dir = project_root.join(".hone/specs").join(&slug);
    if spec_dir.exists() {
        return Err(SpecCommandError::AlreadyExists(spec_dir));
    }

    // Call the injected generator.
    let spec = generator
        .generate(goal)
        .await
        .map_err(|e| SpecCommandError::Generator(e.to_string()))?;

    let task_count = spec.tasks.len();

    // Persist to disk.
    let saved_dir = store
        .save(&spec)
        .await
        .map_err(|e| SpecCommandError::Store(e.to_string()))?;

    Ok(SpecCommandOutput {
        spec_dir: saved_dir,
        slug,
        goal: goal.to_owned(),
        task_count,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum SpecCommandError {
    #[error("empty goal")]
    EmptyGoal,
    #[error("spec already exists at {0}; remove it first or use a different goal")]
    AlreadyExists(PathBuf),
    #[error("generator: {0}")]
    Generator(String),
    #[error("store: {0}")]
    Store(String),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::todo
)]
mod tests {
    use super::*;

    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use hone_spec::{
        Spec,
        model::{AcceptanceCriterion, Task, TaskStatus},
    };
    use tempfile::TempDir;

    // ------------------------------------------------------------------------
    // RecordingSpecGenerator
    //
    // Returns a canned Spec and counts how many times generate() was called.
    // Arc<AtomicUsize> is used for the counter so it's safely shareable across
    // concurrent tasks (tokio::join! test).
    // ------------------------------------------------------------------------

    struct RecordingSpecGenerator {
        /// Canned Spec returned on each successful call.
        canned_spec: Spec,
        /// Set to Some(msg) to make generate() return an error.
        force_error: Option<String>,
        /// Incremented atomically on every generate() call.
        call_count: Arc<AtomicUsize>,
        /// Last goal seen by generate(). Mutex guards the Option write.
        last_goal: Arc<std::sync::Mutex<Option<String>>>,
    }

    impl RecordingSpecGenerator {
        fn new(canned_spec: Spec) -> Self {
            Self {
                canned_spec,
                force_error: None,
                call_count: Arc::new(AtomicUsize::new(0)),
                last_goal: Arc::new(std::sync::Mutex::new(None)),
            }
        }

        fn with_error(msg: impl Into<String>) -> Self {
            Self {
                canned_spec: make_canned_spec("noop", "noop"),
                force_error: Some(msg.into()),
                call_count: Arc::new(AtomicUsize::new(0)),
                last_goal: Arc::new(std::sync::Mutex::new(None)),
            }
        }

        fn call_count(&self) -> usize {
            self.call_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl SpecGenerator for RecordingSpecGenerator {
        async fn generate(&self, goal: &str) -> Result<Spec, hone_spec::GenerateError> {
            self.call_count.fetch_add(1, Ordering::SeqCst);
            *self.last_goal.lock().unwrap() = Some(goal.to_owned());
            if let Some(ref msg) = self.force_error {
                return Err(hone_spec::GenerateError::Provider(msg.clone()));
            }
            Ok(self.canned_spec.clone())
        }
    }

    // ------------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------------

    fn make_canned_spec(slug: &str, goal: &str) -> Spec {
        Spec {
            slug: slug.to_owned(),
            goal: goal.to_owned(),
            requirements_md: "# Requirements\n\nUsers can log in.\n".to_owned(),
            design_md: "# Design\n\nUse JWT tokens.\n".to_owned(),
            tasks: vec![
                Task {
                    id: "T001".to_owned(),
                    title: "Implement login".to_owned(),
                    description: "Build the login endpoint.".to_owned(),
                    acceptance_criteria: vec![AcceptanceCriterion {
                        text: "Returns 200 on valid credentials".to_owned(),
                        done: false,
                    }],
                    depends_on: vec![],
                    status: TaskStatus::Pending,
                },
                Task {
                    id: "T002".to_owned(),
                    title: "Write tests".to_owned(),
                    description: "Add unit + integration tests.".to_owned(),
                    acceptance_criteria: vec![],
                    depends_on: vec!["T001".to_owned()],
                    status: TaskStatus::Pending,
                },
                Task {
                    id: "T003".to_owned(),
                    title: "Deploy".to_owned(),
                    description: "Deploy to staging.".to_owned(),
                    acceptance_criteria: vec![],
                    depends_on: vec!["T002".to_owned()],
                    status: TaskStatus::Pending,
                },
            ],
        }
    }

    // ------------------------------------------------------------------------
    // Tests
    // ------------------------------------------------------------------------

    #[tokio::test]
    async fn cmd_spec_creates_spec_dir_under_dot_hone_specs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let generator = RecordingSpecGenerator::new(make_canned_spec("build-foo", "build foo"));

        cmd_spec("build foo", root, &generator).await.unwrap();

        // The slug for "build foo" should be "build-foo".
        let expected_dir = root.join(".hone/specs/build-foo");
        assert!(expected_dir.exists(), "spec dir missing: {}", expected_dir.display());
        assert!(expected_dir.join("requirements.md").exists(), "requirements.md missing");
        assert!(expected_dir.join("design.md").exists(), "design.md missing");
        assert!(expected_dir.join("tasks.md").exists(), "tasks.md missing");
        assert!(expected_dir.join("goal.txt").exists(), "goal.txt missing");
    }

    #[tokio::test]
    async fn cmd_spec_returns_slug_derived_from_goal() {
        let tmp = TempDir::new().unwrap();
        let generator =
            RecordingSpecGenerator::new(make_canned_spec("build-user-auth", "Build user auth"));

        let output = cmd_spec("Build user auth", tmp.path(), &generator).await.unwrap();

        assert_eq!(output.slug, "build-user-auth");
    }

    #[tokio::test]
    async fn cmd_spec_returns_task_count_from_generated_tasks_md() {
        let tmp = TempDir::new().unwrap();
        // Canned spec has exactly 3 tasks.
        let generator = RecordingSpecGenerator::new(make_canned_spec("some-goal", "some goal"));

        let output = cmd_spec("some goal", tmp.path(), &generator).await.unwrap();

        assert_eq!(output.task_count, 3, "expected 3 tasks from canned spec");
    }

    #[tokio::test]
    async fn cmd_spec_passes_goal_through_to_generator() {
        let tmp = TempDir::new().unwrap();
        let generator = RecordingSpecGenerator::new(make_canned_spec("my-feature", "my feature"));
        let last_goal = Arc::clone(&generator.last_goal);

        cmd_spec("my feature", tmp.path(), &generator).await.unwrap();

        let seen = last_goal.lock().unwrap().clone().unwrap();
        assert_eq!(seen, "my feature");
    }

    #[tokio::test]
    async fn cmd_spec_empty_goal_returns_empty_goal_error_without_calling_generator() {
        let tmp = TempDir::new().unwrap();
        let generator = RecordingSpecGenerator::new(make_canned_spec("x", "x"));

        let result = cmd_spec("", tmp.path(), &generator).await;

        assert!(
            matches!(result, Err(SpecCommandError::EmptyGoal)),
            "expected EmptyGoal, got: {:?}",
            result
        );
        assert_eq!(generator.call_count(), 0, "generator must not be called on empty goal");
    }

    #[tokio::test]
    async fn cmd_spec_whitespace_only_goal_treated_as_empty() {
        let tmp = TempDir::new().unwrap();
        let generator = RecordingSpecGenerator::new(make_canned_spec("x", "x"));

        let result = cmd_spec("   \n  ", tmp.path(), &generator).await;

        assert!(
            matches!(result, Err(SpecCommandError::EmptyGoal)),
            "expected EmptyGoal for whitespace-only goal, got: {:?}",
            result
        );
        assert_eq!(generator.call_count(), 0, "generator must not be called for whitespace-only goal");
    }

    #[tokio::test]
    async fn cmd_spec_already_existing_spec_dir_returns_already_exists_err() {
        let tmp = TempDir::new().unwrap();
        let slug_dir = tmp.path().join(".hone/specs/build-user-auth");
        std::fs::create_dir_all(&slug_dir).unwrap();

        let generator =
            RecordingSpecGenerator::new(make_canned_spec("build-user-auth", "Build user auth"));

        let result = cmd_spec("Build user auth", tmp.path(), &generator).await;

        assert!(
            matches!(result, Err(SpecCommandError::AlreadyExists(_))),
            "expected AlreadyExists, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn cmd_spec_generator_error_propagates_as_generator_variant() {
        let tmp = TempDir::new().unwrap();
        let generator = RecordingSpecGenerator::with_error("provider timeout");

        let result = cmd_spec("some goal", tmp.path(), &generator).await;

        assert!(
            matches!(result, Err(SpecCommandError::Generator(_))),
            "expected Generator error, got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn cmd_spec_writes_goal_txt_with_original_input() {
        let tmp = TempDir::new().unwrap();
        let goal = "Build user auth";
        let generator = RecordingSpecGenerator::new(make_canned_spec("build-user-auth", goal));

        cmd_spec(goal, tmp.path(), &generator).await.unwrap();

        let written =
            std::fs::read_to_string(tmp.path().join(".hone/specs/build-user-auth/goal.txt"))
                .unwrap();
        // goal.txt must contain the verbatim input goal (SpecStore writes spec.goal,
        // which cmd_spec copies from the `goal` argument).
        assert_eq!(written, goal);
    }

    #[tokio::test]
    async fn cmd_spec_returned_spec_dir_matches_disk() {
        let tmp = TempDir::new().unwrap();
        let generator = RecordingSpecGenerator::new(make_canned_spec("match-check", "match check"));

        let output = cmd_spec("match check", tmp.path(), &generator).await.unwrap();

        assert!(output.spec_dir.exists(), "output.spec_dir does not exist on disk");
        assert_eq!(
            output.spec_dir.canonicalize().unwrap(),
            tmp.path()
                .join(".hone/specs/match-check")
                .canonicalize()
                .unwrap()
        );
    }

    #[tokio::test]
    async fn cmd_spec_concurrent_calls_with_different_goals_succeed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        // Each future needs its own generator because &dyn SpecGenerator isn't Send
        // across an await unless the underlying type is. We construct three separate
        // RecordingSpecGenerators with distinct canned slugs.
        let root1 = root.clone();
        let root2 = root.clone();
        let root3 = root.clone();

        let f1 = async move {
            let generator = RecordingSpecGenerator::new(make_canned_spec("goal-alpha", "goal alpha"));
            cmd_spec("goal alpha", &root1, &generator).await
        };
        let f2 = async move {
            let generator = RecordingSpecGenerator::new(make_canned_spec("goal-beta", "goal beta"));
            cmd_spec("goal beta", &root2, &generator).await
        };
        let f3 = async move {
            let generator = RecordingSpecGenerator::new(make_canned_spec("goal-gamma", "goal gamma"));
            cmd_spec("goal gamma", &root3, &generator).await
        };

        let (r1, r2, r3) = tokio::join!(f1, f2, f3);
        r1.expect("goal-alpha failed");
        r2.expect("goal-beta failed");
        r3.expect("goal-gamma failed");

        assert!(root.join(".hone/specs/goal-alpha").exists(), "goal-alpha dir missing");
        assert!(root.join(".hone/specs/goal-beta").exists(), "goal-beta dir missing");
        assert!(root.join(".hone/specs/goal-gamma").exists(), "goal-gamma dir missing");
    }
}
