//! `GET /agents` and `GET /recipes` — read-only discovery endpoints.
//!
//! Both return JSON arrays the GUI can render without any extra parsing.
//! Each entry carries a `source` tag (`builtin` / `user` / `project`) so
//! the GUI can show where the definition came from and let the user diff
//! a project override against the built-in.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use hone_core::agent_def::AgentDef;
use hone_core::agent_registry::AgentRegistry;
use hone_recipes::Recipe;
use serde::Serialize;

use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoverySource {
    Builtin,
    User,
    Project,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentSummary {
    pub name: String,
    pub description: String,
    pub model: String,
    pub tools: Vec<String>,
    pub sub_agents: Vec<String>,
    pub has_sandbox_profile: bool,
    pub source: DiscoverySource,
}

impl From<&AgentDef> for AgentSummary {
    fn from(def: &AgentDef) -> Self {
        Self {
            name: def.name.clone(),
            description: def.description.clone(),
            model: def.model.clone(),
            tools: def.tools.clone(),
            sub_agents: def.sub_agents.clone(),
            has_sandbox_profile: def.sandbox.is_some(),
            source: DiscoverySource::Builtin, // overwritten by caller
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RecipeSummary {
    pub name: String,
    pub description: Option<String>,
    pub step_count: usize,
    pub has_test_cmd: bool,
    pub source: DiscoverySource,
    /// Absolute path the recipe was loaded from. Useful for the GUI's
    /// "Open in editor" affordance.
    pub path: String,
}

/// `GET /agents`
///
/// Builds the layered registry the agent loop uses (built-ins → user →
/// project) and returns each entry tagged with its source.
pub async fn list_agents_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let working_dir = &state.workspace_root;
    let summaries = collect_agents(working_dir);
    Json(summaries)
}

fn collect_agents(working_dir: &Path) -> Vec<AgentSummary> {
    // Layer 1: built-ins.
    let mut builtin_registry = AgentRegistry::new();
    builtin_registry.load_builtins();
    let builtin_names: std::collections::HashSet<String> = builtin_registry
        .list()
        .iter()
        .map(|d| d.name.clone())
        .collect();

    // Layer 2: user-level (~/.config/hone/agents).
    let user_defs: Vec<AgentDef> = match dirs::config_dir() {
        Some(d) => AgentDef::load_dir_lenient(&d.join("hone").join("agents")),
        None => Vec::new(),
    };
    let user_names: std::collections::HashSet<String> =
        user_defs.iter().map(|d| d.name.clone()).collect();

    // Layer 3: project-level (<project_root>/.hone/agents).
    let project_defs: Vec<AgentDef> = match hone_config::find_project_root(working_dir) {
        Some(root) => AgentDef::load_dir_lenient(&root.join(".hone/agents")),
        None => Vec::new(),
    };
    let project_names: std::collections::HashSet<String> =
        project_defs.iter().map(|d| d.name.clone()).collect();

    // Resolve final set with last-writer-wins (project > user > builtin) and
    // tag the source.
    let mut resolved: Vec<AgentSummary> = Vec::new();
    let merged = AgentRegistry::load_hierarchical(working_dir);
    for def in merged.list() {
        let mut s = AgentSummary::from(def);
        s.source = if project_names.contains(&def.name) {
            DiscoverySource::Project
        } else if user_names.contains(&def.name) {
            DiscoverySource::User
        } else if builtin_names.contains(&def.name) {
            DiscoverySource::Builtin
        } else {
            // Should not happen — load_hierarchical only sources from these
            // three layers. Default to Builtin so the wire shape stays
            // closed.
            DiscoverySource::Builtin
        };
        resolved.push(s);
    }
    resolved.sort_by(|a, b| a.name.cmp(&b.name));
    resolved
}

/// `GET /recipes`
///
/// Scans the well-known recipe directories and returns one summary per
/// `*.yaml` file that parses as a [`Recipe`].
pub async fn list_recipes_handler(
    State(state): State<AppState>,
) -> impl IntoResponse {
    let summaries = collect_recipes(&state.workspace_root);
    Json(summaries)
}

fn recipe_dirs(workspace_root: &Path) -> Vec<(PathBuf, DiscoverySource)> {
    let mut dirs = Vec::new();

    // Built-in: workspace_root/recipes/ — the bin/hone shipping recipes
    // live here in the source tree but operators may also drop a
    // "recipes" folder at workspace_root.
    dirs.push((workspace_root.join("recipes"), DiscoverySource::Builtin));

    // User-level: ~/.config/hone/recipes/
    if let Some(d) = dirs::config_dir() {
        dirs.push((d.join("hone").join("recipes"), DiscoverySource::User));
    }

    // Project-level: <project_root>/.hone/recipes/
    if let Some(root) = hone_config::find_project_root(workspace_root) {
        dirs.push((root.join(".hone/recipes"), DiscoverySource::Project));
    }

    dirs
}

fn collect_recipes(workspace_root: &Path) -> Vec<RecipeSummary> {
    let mut summaries: Vec<RecipeSummary> = Vec::new();
    for (dir, source) in recipe_dirs(workspace_root) {
        let read_dir = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for entry in read_dir.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            if ext != Some("yaml") && ext != Some("yml") {
                continue;
            }
            let body = match std::fs::read_to_string(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let recipe = match Recipe::from_yaml(&body) {
                Ok(r) => r,
                Err(_) => continue,
            };
            summaries.push(RecipeSummary {
                name: recipe.name,
                description: recipe.description,
                step_count: recipe.steps.len(),
                has_test_cmd: recipe.test_cmd.is_some(),
                source: source.clone(),
                path: path.display().to_string(),
            });
        }
    }
    summaries.sort_by(|a, b| a.name.cmp(&b.name));
    summaries
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn collect_agents_returns_builtins_at_minimum() {
        let tmp = TempDir::new().expect("tmp");
        let agents = collect_agents(tmp.path());
        assert!(!agents.is_empty(), "built-in agents must always exist");
        assert!(
            agents.iter().any(|a| matches!(a.source, DiscoverySource::Builtin)),
            "at least one agent must be tagged builtin",
        );
    }

    #[test]
    fn collect_agents_results_are_sorted_by_name() {
        let tmp = TempDir::new().expect("tmp");
        let agents = collect_agents(tmp.path());
        for pair in agents.windows(2) {
            assert!(pair[0].name <= pair[1].name, "agents must be sorted by name");
        }
    }

    #[test]
    fn collect_recipes_handles_missing_dirs() {
        let tmp = TempDir::new().expect("tmp");
        // No recipes dir under tmp; user/project dirs may or may not have
        // some on the test machine. We just assert this does not panic.
        let _ = collect_recipes(tmp.path());
    }

    #[test]
    fn collect_recipes_parses_a_well_formed_yaml() {
        let tmp = TempDir::new().expect("tmp");
        let recipes_dir = tmp.path().join("recipes");
        std::fs::create_dir_all(&recipes_dir).expect("mkdir");
        let yaml = r#"
name: smoke
description: a smoke-test recipe
steps:
  - name: step1
    prompt: do thing
test_cmd: cargo test
"#;
        std::fs::write(recipes_dir.join("smoke.yaml"), yaml).expect("write");

        let recipes = collect_recipes(tmp.path());
        let smoke = recipes
            .iter()
            .find(|r| r.name == "smoke")
            .expect("smoke recipe");
        assert_eq!(smoke.step_count, 1);
        assert!(smoke.has_test_cmd);
        assert!(matches!(smoke.source, DiscoverySource::Builtin));
        assert!(smoke.path.ends_with("smoke.yaml"));
    }

    #[test]
    fn collect_recipes_skips_non_yaml_files() {
        let tmp = TempDir::new().expect("tmp");
        let recipes_dir = tmp.path().join("recipes");
        std::fs::create_dir_all(&recipes_dir).expect("mkdir");
        std::fs::write(recipes_dir.join("notes.txt"), "hello").expect("write");
        let recipes = collect_recipes(tmp.path());
        assert!(
            recipes.iter().all(|r| !r.path.ends_with("notes.txt")),
            "non-yaml files must be skipped",
        );
    }
}
