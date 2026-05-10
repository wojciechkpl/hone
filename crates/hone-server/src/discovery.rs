//! `GET /agents` and `GET /recipes` — read-only discovery endpoints.
//!
//! Both return JSON arrays the GUI can render without any extra parsing.
//! Each entry carries a `source` tag (`builtin` / `user` / `project`) so
//! the GUI can show where the definition came from and let the user diff
//! a project override against the built-in.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::extract::{Path as AxumPath, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use hone_core::agent_def::AgentDef;
use hone_core::agent_registry::AgentRegistry;
use hone_recipes::Recipe;
use serde::{Deserialize, Serialize};

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
pub async fn list_agents_handler(State(state): State<AppState>) -> impl IntoResponse {
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
pub async fn list_recipes_handler(State(state): State<AppState>) -> impl IntoResponse {
    let summaries = collect_recipes(&state);
    Json(summaries)
}

fn recipe_dirs(state: &AppState) -> Vec<(PathBuf, DiscoverySource)> {
    let workspace_root = &state.workspace_root;
    let mut dirs = Vec::new();

    // Built-in: workspace_root/recipes/ — the bin/hone shipping recipes
    // live here in the source tree but operators may also drop a
    // "recipes" folder at workspace_root.
    dirs.push((workspace_root.join("recipes"), DiscoverySource::Builtin));

    // User-level: honor injected dir when present (e.g. tests), otherwise
    // fall back to ~/.config/hone/recipes/.
    if let Some(d) = state.recipes_user_dir.clone() {
        dirs.push((d, DiscoverySource::User));
    } else if let Some(d) = dirs::config_dir() {
        dirs.push((d.join("hone").join("recipes"), DiscoverySource::User));
    }

    // Project-level: <project_root>/.hone/recipes/
    if let Some(root) = hone_config::find_project_root(workspace_root) {
        dirs.push((root.join(".hone/recipes"), DiscoverySource::Project));
    }

    dirs
}

fn collect_recipes(state: &AppState) -> Vec<RecipeSummary> {
    let mut summaries: Vec<RecipeSummary> = Vec::new();
    for (dir, source) in recipe_dirs(state) {
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

// ---------------------------------------------------------------------------
// Detail endpoints
// ---------------------------------------------------------------------------

/// Full agent detail returned by `GET /agents/{name}`. Carries the entire
/// `AgentDef` (including the markdown body the loader preserves) plus the
/// resolved source.
#[derive(Debug, Clone, Serialize)]
pub struct AgentDetail {
    pub name: String,
    pub description: String,
    pub model: String,
    pub tools: Vec<String>,
    pub sub_agents: Vec<String>,
    /// The full system prompt (markdown body after frontmatter). Always
    /// included so the GUI can preview / fork the agent.
    pub system_prompt: String,
    pub has_sandbox_profile: bool,
    pub source: DiscoverySource,
}

/// Full recipe detail returned by `GET /recipes/{name}`. Includes every
/// step plus the resolved on-disk path.
#[derive(Debug, Clone, Serialize)]
pub struct RecipeDetail {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<RecipeStepWire>,
    pub test_cmd: Option<String>,
    pub source: DiscoverySource,
    pub path: String,
}

/// Wire shape of a recipe step. `hone_recipes::RecipeStep` is re-serialised
/// here so the GUI does not depend on the recipes crate's exact field
/// ordering.
#[derive(Debug, Clone, Serialize)]
pub struct RecipeStepWire {
    pub name: String,
    pub prompt: String,
}

/// `GET /agents/{name}` — full detail for one agent. 404 if not found in
/// any layer.
pub async fn agent_detail_handler(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Result<Json<AgentDetail>, (StatusCode, String)> {
    find_agent(&state.workspace_root, &name)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, format!("no agent named '{name}'")))
}

/// `GET /recipes/{name}` — full detail for one recipe. 404 if not found
/// in any layer.
pub async fn recipe_detail_handler(
    State(state): State<AppState>,
    AxumPath(name): AxumPath<String>,
) -> Result<Json<RecipeDetail>, (StatusCode, String)> {
    find_recipe(&state, &name)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, format!("no recipe named '{name}'")))
}

fn find_agent(working_dir: &Path, name: &str) -> Option<AgentDetail> {
    // Same source-resolution logic as `collect_agents`, applied to one name.
    let mut builtin_registry = AgentRegistry::new();
    builtin_registry.load_builtins();
    let builtin_names: std::collections::HashSet<String> = builtin_registry
        .list()
        .iter()
        .map(|d| d.name.clone())
        .collect();

    let user_defs: Vec<AgentDef> = match dirs::config_dir() {
        Some(d) => AgentDef::load_dir_lenient(&d.join("hone").join("agents")),
        None => Vec::new(),
    };
    let user_names: std::collections::HashSet<String> =
        user_defs.iter().map(|d| d.name.clone()).collect();

    let project_defs: Vec<AgentDef> = match hone_config::find_project_root(working_dir) {
        Some(root) => AgentDef::load_dir_lenient(&root.join(".hone/agents")),
        None => Vec::new(),
    };
    let project_names: std::collections::HashSet<String> =
        project_defs.iter().map(|d| d.name.clone()).collect();

    let merged = AgentRegistry::load_hierarchical(working_dir);
    let def = merged.get(name)?;
    let source = if project_names.contains(&def.name) {
        DiscoverySource::Project
    } else if user_names.contains(&def.name) {
        DiscoverySource::User
    } else if builtin_names.contains(&def.name) {
        DiscoverySource::Builtin
    } else {
        DiscoverySource::Builtin
    };
    Some(AgentDetail {
        name: def.name.clone(),
        description: def.description.clone(),
        model: def.model.clone(),
        tools: def.tools.clone(),
        sub_agents: def.sub_agents.clone(),
        system_prompt: def.system_prompt.clone(),
        has_sandbox_profile: def.sandbox.is_some(),
        source,
    })
}

fn find_recipe(state: &AppState, name: &str) -> Option<RecipeDetail> {
    // Walk the same directory layering, but stop at the highest-precedence
    // match (project > user > builtin). That mirrors the loader semantics
    // a runtime would use to actually run the recipe.
    let mut search: Vec<(PathBuf, DiscoverySource)> = recipe_dirs(state);
    search.reverse(); // last entry was project — check it first

    for (dir, source) in search {
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
            if recipe.name == name {
                let steps = recipe
                    .steps
                    .into_iter()
                    .map(|s| RecipeStepWire {
                        name: s.name,
                        prompt: s.prompt,
                    })
                    .collect();
                return Some(RecipeDetail {
                    name: recipe.name,
                    description: recipe.description,
                    steps,
                    test_cmd: recipe.test_cmd,
                    source,
                    path: path.display().to_string(),
                });
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Write endpoint — POST /recipes
// ---------------------------------------------------------------------------

/// Request body for `POST /recipes`.
#[derive(Debug, Deserialize)]
pub struct CreateRecipeRequest {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<CreateRecipeStep>,
    pub test_cmd: Option<String>,
}

/// A single step in a [`CreateRecipeRequest`].
#[derive(Debug, Deserialize)]
pub struct CreateRecipeStep {
    pub name: String,
    pub prompt: String,
}

fn validate_recipe_name(name: &str) -> Result<(), (StatusCode, String)> {
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "recipe name must be non-empty".into(),
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "recipe name may only contain [a-zA-Z0-9_-]".into(),
        ));
    }
    Ok(())
}

fn user_recipes_dir(state: &AppState) -> PathBuf {
    state.recipes_user_dir.clone().unwrap_or_else(|| {
        dirs::config_dir()
            .map(|d| d.join("hone").join("recipes"))
            .unwrap_or_else(|| PathBuf::from(".hone-recipes"))
    })
}

/// `POST /recipes` — write a new recipe YAML to the user-level directory and
/// return the created [`RecipeDetail`].
pub async fn create_recipe_handler(
    State(state): State<AppState>,
    Json(req): Json<CreateRecipeRequest>,
) -> Result<(StatusCode, Json<RecipeDetail>), (StatusCode, String)> {
    validate_recipe_name(&req.name)?;
    if req.steps.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "recipe must have at least one step".into(),
        ));
    }
    for s in &req.steps {
        if s.name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "step.name must be non-empty".into(),
            ));
        }
        if s.prompt.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "step.prompt must be non-empty".into(),
            ));
        }
    }

    let dir = user_recipes_dir(&state);
    std::fs::create_dir_all(&dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("create dir: {e}"),
        )
    })?;
    let path = dir.join(format!("{}.yaml", req.name));
    if path.exists() {
        return Err((
            StatusCode::CONFLICT,
            format!("recipe '{}' already exists", req.name),
        ));
    }

    // Build a hone_recipes::Recipe so the on-disk shape matches what the GET
    // handler expects to round-trip-parse.
    let recipe = hone_recipes::Recipe {
        name: req.name.clone(),
        description: req.description.clone(),
        steps: req
            .steps
            .iter()
            .map(|s| hone_recipes::RecipeStep {
                name: s.name.clone(),
                prompt: s.prompt.clone(),
                when: None,
            })
            .collect(),
        test_cmd: req.test_cmd.clone(),
    };
    let yaml = serde_yaml::to_string(&recipe).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("serialize recipe: {e}"),
        )
    })?;
    std::fs::write(&path, yaml).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("write recipe: {e}"),
        )
    })?;

    Ok((
        StatusCode::CREATED,
        Json(RecipeDetail {
            name: req.name,
            description: req.description,
            steps: req
                .steps
                .into_iter()
                .map(|s| RecipeStepWire {
                    name: s.name,
                    prompt: s.prompt,
                })
                .collect(),
            test_cmd: req.test_cmd,
            source: DiscoverySource::User,
            path: path.display().to_string(),
        }),
    ))
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
            agents
                .iter()
                .any(|a| matches!(a.source, DiscoverySource::Builtin)),
            "at least one agent must be tagged builtin",
        );
    }

    #[test]
    fn collect_agents_results_are_sorted_by_name() {
        let tmp = TempDir::new().expect("tmp");
        let agents = collect_agents(tmp.path());
        for pair in agents.windows(2) {
            assert!(
                pair[0].name <= pair[1].name,
                "agents must be sorted by name"
            );
        }
    }

    fn state_at(root: &std::path::Path) -> AppState {
        AppState::new(None).with_workspace_root(root.to_path_buf())
    }

    #[test]
    fn collect_recipes_handles_missing_dirs() {
        let tmp = TempDir::new().expect("tmp");
        // No recipes dir under tmp; user/project dirs may or may not have
        // some on the test machine. We just assert this does not panic.
        let _ = collect_recipes(&state_at(tmp.path()));
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

        let recipes = collect_recipes(&state_at(tmp.path()));
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
        let recipes = collect_recipes(&state_at(tmp.path()));
        assert!(
            recipes.iter().all(|r| !r.path.ends_with("notes.txt")),
            "non-yaml files must be skipped",
        );
    }

    #[test]
    fn find_agent_returns_none_for_unknown_name() {
        let tmp = TempDir::new().expect("tmp");
        assert!(find_agent(tmp.path(), "this-does-not-exist-xyz").is_none());
    }

    #[test]
    fn find_agent_returns_a_known_builtin_with_full_prompt() {
        let tmp = TempDir::new().expect("tmp");
        // Pick the first built-in by listing.
        let agents = collect_agents(tmp.path());
        let first = agents.first().expect("at least one builtin");
        let detail = find_agent(tmp.path(), &first.name).expect("detail for builtin");
        assert_eq!(detail.name, first.name);
        assert!(matches!(detail.source, DiscoverySource::Builtin));
        // The system prompt is the markdown body; built-in agents always
        // have one. Empty body would be a regression in the loader.
        assert!(!detail.system_prompt.is_empty(), "builtin must have a body");
    }

    #[test]
    fn find_recipe_returns_full_steps_when_found() {
        let tmp = TempDir::new().expect("tmp");
        let recipes_dir = tmp.path().join("recipes");
        std::fs::create_dir_all(&recipes_dir).expect("mkdir");
        let yaml = r#"
name: alpha
description: alpha desc
steps:
  - name: s1
    prompt: do alpha
  - name: s2
    prompt: then beta
"#;
        std::fs::write(recipes_dir.join("alpha.yaml"), yaml).expect("write");

        let detail = find_recipe(&state_at(tmp.path()), "alpha").expect("found");
        assert_eq!(detail.name, "alpha");
        assert_eq!(detail.steps.len(), 2);
        assert_eq!(detail.steps[0].name, "s1");
        assert_eq!(detail.steps[1].prompt, "then beta");
        assert!(matches!(detail.source, DiscoverySource::Builtin));
    }

    #[test]
    fn find_recipe_returns_none_for_unknown_name() {
        let tmp = TempDir::new().expect("tmp");
        assert!(find_recipe(&state_at(tmp.path()), "no-such-recipe").is_none());
    }
}
