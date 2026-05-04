use std::path::{Path, PathBuf};

use crate::model::Spec;
use crate::parse::parse_tasks_md;
use crate::render::render_tasks_md;

/// Persistent store at `<root>/.hone/specs/<slug>/`:
///   requirements.md
///   design.md
///   tasks.md
/// Plus an internal goal.txt holding the original prompt (for round-trip).
pub struct SpecStore {
    base_dir: PathBuf,
}

impl SpecStore {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Resolves to `<working_dir>/.hone/specs`.
    pub fn default_for(working_dir: &Path) -> Self {
        Self::new(working_dir.join(".hone/specs"))
    }

    /// Persist a Spec. Creates `<base>/<slug>/` and writes the three docs +
    /// goal.txt. Returns the directory path.
    pub async fn save(&self, spec: &Spec) -> Result<PathBuf, SpecStoreError> {
        let dir = self.base_dir.join(&spec.slug);
        tokio::fs::create_dir_all(&dir).await?;
        tokio::fs::write(dir.join("requirements.md"), &spec.requirements_md).await?;
        tokio::fs::write(dir.join("design.md"), &spec.design_md).await?;
        tokio::fs::write(dir.join("tasks.md"), render_tasks_md(&spec.tasks)).await?;
        tokio::fs::write(dir.join("goal.txt"), &spec.goal).await?;
        Ok(dir)
    }

    /// Load by slug. Reads the three docs + goal.txt, parses tasks.md.
    pub async fn load(&self, slug: &str) -> Result<Spec, SpecStoreError> {
        let dir = self.base_dir.join(slug);
        if !dir.exists() {
            return Err(SpecStoreError::NotFound {
                slug: slug.to_owned(),
            });
        }

        let read_file = |name: &'static str| {
            let path = dir.join(name);
            async move {
                if !path.exists() {
                    return Err(SpecStoreError::MissingFile {
                        slug: slug.to_owned(),
                        file: name.to_owned(),
                    });
                }
                tokio::fs::read_to_string(&path)
                    .await
                    .map_err(SpecStoreError::Io)
            }
        };

        let requirements_md = read_file("requirements.md").await?;
        let design_md = read_file("design.md").await?;
        let tasks_md = read_file("tasks.md").await?;
        let goal = read_file("goal.txt").await?;

        let tasks = parse_tasks_md(&tasks_md);

        Ok(Spec {
            slug: slug.to_owned(),
            goal,
            requirements_md,
            design_md,
            tasks,
        })
    }

    /// List known specs (subdirectory names of base_dir).
    pub async fn list(&self) -> Result<Vec<String>, SpecStoreError> {
        let mut entries = match tokio::fs::read_dir(&self.base_dir).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(vec![]);
            }
            Err(e) => return Err(SpecStoreError::Io(e)),
        };

        let mut slugs: Vec<String> = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                slugs.push(entry.file_name().to_string_lossy().into_owned());
            }
        }
        slugs.sort();
        Ok(slugs)
    }

    /// Delete by slug.
    pub async fn delete(&self, slug: &str) -> Result<(), SpecStoreError> {
        let dir = self.base_dir.join(slug);
        if !dir.exists() {
            return Err(SpecStoreError::NotFound {
                slug: slug.to_owned(),
            });
        }
        tokio::fs::remove_dir_all(&dir).await?;
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SpecStoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("spec '{slug}' not found")]
    NotFound { slug: String },
    #[error("missing required file '{file}' in spec '{slug}'")]
    MissingFile { slug: String, file: String },
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::model::{AcceptanceCriterion, Task, TaskStatus};
    use tempfile::TempDir;

    fn make_spec(slug: &str, goal: &str) -> Spec {
        Spec {
            slug: slug.to_owned(),
            goal: goal.to_owned(),
            requirements_md: "# Requirements\n\nUsers can log in.\n".to_owned(),
            design_md: "# Design\n\nUse JWT tokens.\n".to_owned(),
            tasks: vec![Task {
                id: "T001".to_owned(),
                title: "Implement login".to_owned(),
                description: "Build the login endpoint.".to_owned(),
                acceptance_criteria: vec![AcceptanceCriterion {
                    text: "Returns 200 on valid credentials".to_owned(),
                    done: false,
                }],
                depends_on: vec![],
                status: TaskStatus::Pending,
            }],
        }
    }

    // --------------------------------------------------------------------------
    // save / load
    // --------------------------------------------------------------------------

    #[tokio::test]
    async fn save_creates_three_files_in_slug_subdir() {
        let dir = TempDir::new().unwrap();
        let store = SpecStore::new(dir.path());
        let spec = make_spec("build-user-auth", "Build user auth");

        store.save(&spec).await.unwrap();

        let slug_dir = dir.path().join("build-user-auth");
        assert!(slug_dir.join("requirements.md").exists(), "requirements.md missing");
        assert!(slug_dir.join("design.md").exists(), "design.md missing");
        assert!(slug_dir.join("tasks.md").exists(), "tasks.md missing");
        assert!(slug_dir.join("goal.txt").exists(), "goal.txt missing");
    }

    #[tokio::test]
    async fn save_then_load_round_trips() {
        let dir = TempDir::new().unwrap();
        let store = SpecStore::new(dir.path());
        let original = make_spec("my-spec", "My spec goal");

        store.save(&original).await.unwrap();
        let loaded = store.load("my-spec").await.unwrap();

        assert_eq!(loaded.slug, original.slug);
        assert_eq!(loaded.goal, original.goal);
        assert_eq!(loaded.requirements_md, original.requirements_md);
        assert_eq!(loaded.design_md, original.design_md);
        assert_eq!(loaded.tasks.len(), original.tasks.len());
        assert_eq!(loaded.tasks[0].id, "T001");
    }

    // --------------------------------------------------------------------------
    // load error cases
    // --------------------------------------------------------------------------

    #[tokio::test]
    async fn load_unknown_slug_returns_not_found() {
        let dir = TempDir::new().unwrap();
        let store = SpecStore::new(dir.path());

        let result = store.load("nonexistent-slug").await;
        assert!(matches!(result, Err(SpecStoreError::NotFound { .. })));
    }

    #[tokio::test]
    async fn load_missing_required_file_returns_error() {
        // Write only requirements.md; omit design.md to trigger MissingFile
        let dir = TempDir::new().unwrap();
        let slug_dir = dir.path().join("partial-spec");
        std::fs::create_dir_all(&slug_dir).unwrap();
        std::fs::write(slug_dir.join("requirements.md"), "# Requirements\n").unwrap();
        std::fs::write(slug_dir.join("goal.txt"), "Some goal").unwrap();
        // Intentionally NOT writing design.md or tasks.md

        let store = SpecStore::new(dir.path());
        let result = store.load("partial-spec").await;
        assert!(
            matches!(result, Err(SpecStoreError::MissingFile { .. })),
            "expected MissingFile, got: {:?}",
            result
        );
    }

    // --------------------------------------------------------------------------
    // list
    // --------------------------------------------------------------------------

    #[tokio::test]
    async fn list_returns_known_slugs_sorted() {
        let dir = TempDir::new().unwrap();
        let store = SpecStore::new(dir.path());

        let spec_b = make_spec("beta-spec", "Beta goal");
        let spec_a = make_spec("alpha-spec", "Alpha goal");
        store.save(&spec_b).await.unwrap();
        store.save(&spec_a).await.unwrap();

        let slugs = store.list().await.unwrap();
        assert_eq!(slugs, vec!["alpha-spec", "beta-spec"]);
    }

    #[tokio::test]
    async fn list_skips_files_at_top_level() {
        let dir = TempDir::new().unwrap();
        let store = SpecStore::new(dir.path());

        // Drop a plain file in base_dir — list must not include it
        std::fs::write(dir.path().join("stray-file.txt"), "noise").unwrap();

        let spec = make_spec("real-spec", "A real spec");
        store.save(&spec).await.unwrap();

        let slugs = store.list().await.unwrap();
        assert_eq!(slugs, vec!["real-spec"]);
        assert!(!slugs.contains(&"stray-file.txt".to_owned()));
    }

    #[tokio::test]
    async fn list_missing_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        // Point store at a subdirectory that doesn't exist yet
        let store = SpecStore::new(dir.path().join("no-such-dir"));

        let slugs = store.list().await.unwrap();
        assert!(slugs.is_empty());
    }

    // --------------------------------------------------------------------------
    // delete
    // --------------------------------------------------------------------------

    #[tokio::test]
    async fn delete_removes_slug_subdir() {
        let dir = TempDir::new().unwrap();
        let store = SpecStore::new(dir.path());
        let spec = make_spec("to-delete", "Delete me");
        store.save(&spec).await.unwrap();

        assert!(dir.path().join("to-delete").exists());
        store.delete("to-delete").await.unwrap();
        assert!(!dir.path().join("to-delete").exists());
    }

    #[tokio::test]
    async fn delete_unknown_slug_returns_not_found() {
        let dir = TempDir::new().unwrap();
        let store = SpecStore::new(dir.path());

        let result = store.delete("does-not-exist").await;
        assert!(matches!(result, Err(SpecStoreError::NotFound { .. })));
    }

    // --------------------------------------------------------------------------
    // default_for
    // --------------------------------------------------------------------------

    #[test]
    fn default_for_resolves_to_dot_hone_specs() {
        let working_dir = Path::new("/some/project");
        let store = SpecStore::default_for(working_dir);
        assert_eq!(store.base_dir, Path::new("/some/project/.hone/specs"));
    }
}
