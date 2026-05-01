/// Integration tests for `AgentRegistry::load_hierarchical`.
///
/// # Environment variable safety
///
/// Several tests manipulate the `HOME` environment variable, which is
/// process-global.  When `cargo test` runs integration tests it compiles each
/// `tests/*.rs` file into its own binary, but **all tests within that binary
/// run in the same process**.  Concurrent mutation of `HOME` would cause races.
///
/// We serialise every test that touches `HOME` through `ENV_MUTEX`.  Tests
/// that do not touch environment variables do not need the lock and are marked
/// accordingly.  If `serial_test` is later added as a dev-dependency the
/// `#[serial]` attribute can replace the manual lock pattern.
use std::path::Path;
use std::sync::Mutex;

use hone_core::AgentRegistry;

// ---------------------------------------------------------------------------
// Shared mutex for tests that mutate process-global environment variables.
// ---------------------------------------------------------------------------

/// All tests that call `std::env::set_var("HOME", …)` or
/// `std::env::remove_var("HOME")` MUST acquire this lock for the duration of
/// the test to prevent races with other tests in the same process.
static ENV_MUTEX: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// RAII guard that restores `HOME` to its original value when dropped.
/// Acquire `ENV_MUTEX` before constructing this guard and hold the guard for
/// the duration of the test.
struct HomeGuard {
    original: Option<String>,
}

impl HomeGuard {
    /// Override `HOME` with `new_home`.
    fn set(new_home: &Path) -> Self {
        let original = std::env::var("HOME").ok();
        // SAFETY: We hold ENV_MUTEX, so no concurrent thread is reading HOME.
        unsafe { std::env::set_var("HOME", new_home) };
        HomeGuard { original }
    }

    /// Remove `HOME` entirely (simulates an unset HOME).
    fn unset() -> Self {
        let original = std::env::var("HOME").ok();
        // SAFETY: We hold ENV_MUTEX.
        unsafe { std::env::remove_var("HOME") };
        HomeGuard { original: Some(original.unwrap_or_default()) }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        // SAFETY: We still hold ENV_MUTEX when this drops (guard is declared
        // after the mutex lock in every test).
        unsafe {
            match self.original.take() {
                Some(v) if !v.is_empty() => std::env::set_var("HOME", v),
                _ => std::env::remove_var("HOME"),
            }
        }
    }
}

/// Write a minimal valid agent `.md` file.
///
/// ```text
/// ---
/// name: <name>
/// description: <description>
/// ---
///
/// System prompt for <name>.
/// ```
fn write_agent_md(dir: &Path, filename: &str, name: &str, description: &str) {
    let content = format!(
        "---\nname: {name}\ndescription: {description}\ntools:\n  - Read\n---\n\nSystem prompt for {name}.\n"
    );
    std::fs::write(dir.join(filename), content)
        .expect("write_agent_md: failed to write agent file");
}

/// The full list of built-in agent names embedded at compile time (12 total).
const BUILTIN_NAMES: &[&str] = &[
    "api-designer",
    "code-reviewer",
    "data-engineer",
    "debugger",
    "documentation-agent",
    "ml-engineer",
    "performance-optimizer",
    "python-expert",
    "researcher",
    "rust-expert",
    "security-auditor",
    "tdd-workflow",
];

// ---------------------------------------------------------------------------
// Test 1 – no overlays → only built-ins
// ---------------------------------------------------------------------------

/// When neither the user config dir nor any project-level agents dir exists,
/// `load_hierarchical` must return exactly the 12 compiled-in built-in agents.
#[test]
fn load_hierarchical_with_no_overlays_returns_only_builtins() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    // A HOME that has no .config/hone/agents directory.
    let home_dir = tempfile::TempDir::new().expect("create home tempdir");
    // A working_dir with no project marker so no project root is found.
    let work_dir = tempfile::TempDir::new().expect("create work tempdir");

    let _guard = HomeGuard::set(home_dir.path());

    let registry = AgentRegistry::load_hierarchical(work_dir.path());

    // All 12 built-ins must be present.
    for name in BUILTIN_NAMES {
        assert!(
            registry.get(name).is_some(),
            "expected built-in '{name}' to be registered"
        );
    }

    // Total count must equal exactly the number of built-ins — no extras.
    assert_eq!(
        registry.list().len(),
        BUILTIN_NAMES.len(),
        "registry should contain exactly the built-ins and nothing else"
    );
}

// ---------------------------------------------------------------------------
// Test 2 – user-level agents are loaded
// ---------------------------------------------------------------------------

/// A valid agent `.md` placed at `~/.config/hone/agents/foo.md` must be
/// registered in addition to the built-ins.
#[test]
fn load_hierarchical_loads_user_agents_from_home_config() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    let home_dir = tempfile::TempDir::new().expect("create home tempdir");
    let agents_dir = home_dir.path().join(".config/hone/agents");
    std::fs::create_dir_all(&agents_dir).expect("create user agents dir");
    write_agent_md(&agents_dir, "foo.md", "foo", "a user-level test agent");

    let work_dir = tempfile::TempDir::new().expect("create work tempdir");

    let _guard = HomeGuard::set(home_dir.path());

    let registry = AgentRegistry::load_hierarchical(work_dir.path());

    // The custom agent must be present.
    assert!(
        registry.get("foo").is_some(),
        "user-level agent 'foo' must be registered"
    );

    // All built-ins must still be present.
    for name in BUILTIN_NAMES {
        assert!(
            registry.get(name).is_some(),
            "built-in '{name}' must still be registered when user agents are present"
        );
    }

    // Total = 12 built-ins + 1 user agent.
    assert_eq!(
        registry.list().len(),
        BUILTIN_NAMES.len() + 1,
        "registry should have built-ins plus the one user agent"
    );
}

// ---------------------------------------------------------------------------
// Test 3 – user agent overrides built-in (same name)
// ---------------------------------------------------------------------------

/// When a user-level agent shares a name with a built-in, the user-level
/// definition must replace the built-in (last-write-wins, user > built-in).
#[test]
fn load_hierarchical_user_agent_overrides_builtin() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    let home_dir = tempfile::TempDir::new().expect("create home tempdir");
    let agents_dir = home_dir.path().join(".config/hone/agents");
    std::fs::create_dir_all(&agents_dir).expect("create user agents dir");
    write_agent_md(
        &agents_dir,
        "python-expert.md",
        "python-expert",
        "custom override",
    );

    let work_dir = tempfile::TempDir::new().expect("create work tempdir");

    let _guard = HomeGuard::set(home_dir.path());

    let registry = AgentRegistry::load_hierarchical(work_dir.path());

    let agent = registry
        .get("python-expert")
        .expect("python-expert must be registered");

    assert_eq!(
        agent.description, "custom override",
        "user-level python-expert must shadow the built-in; got: {:?}",
        agent.description
    );
}

// ---------------------------------------------------------------------------
// Test 4 – project agent overrides user agent (same name)
// ---------------------------------------------------------------------------

/// Project-level agents have the highest precedence.  A `foo` agent defined
/// at the project level must override the same-named agent at the user level.
#[test]
fn load_hierarchical_project_agent_overrides_user() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    // User-level agent: description = "user foo".
    let home_dir = tempfile::TempDir::new().expect("create home tempdir");
    let user_agents_dir = home_dir.path().join(".config/hone/agents");
    std::fs::create_dir_all(&user_agents_dir).expect("create user agents dir");
    write_agent_md(&user_agents_dir, "foo.md", "foo", "user foo");

    // Project tempdir with .git marker and project-level agent: description = "project foo".
    let project_dir = tempfile::TempDir::new().expect("create project tempdir");
    std::fs::create_dir(project_dir.path().join(".git")).expect("create .git dir");
    let project_agents_dir = project_dir.path().join(".hone/agents");
    std::fs::create_dir_all(&project_agents_dir).expect("create project agents dir");
    write_agent_md(&project_agents_dir, "foo.md", "foo", "project foo");

    let _guard = HomeGuard::set(home_dir.path());

    // working_dir is the project root itself.
    let registry = AgentRegistry::load_hierarchical(project_dir.path());

    let agent = registry
        .get("foo")
        .expect("'foo' must be registered");

    assert_eq!(
        agent.description, "project foo",
        "project-level agent must override user-level agent; got: {:?}",
        agent.description
    );
}

// ---------------------------------------------------------------------------
// Test 5 – working_dir is a subdirectory; project root is found by walking up
// ---------------------------------------------------------------------------

/// `load_hierarchical` must walk ancestor directories to locate the project
/// root (the nearest ancestor containing `.git`).  An agent placed in
/// `<project>/.hone/agents/` must be visible even when `working_dir` is
/// several levels deep.
#[test]
fn load_hierarchical_walks_up_to_find_project_root() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    let home_dir = tempfile::TempDir::new().expect("create home tempdir");

    // Project root with .git and a project agent.
    let project_dir = tempfile::TempDir::new().expect("create project tempdir");
    std::fs::create_dir(project_dir.path().join(".git")).expect("create .git dir");
    let project_agents_dir = project_dir.path().join(".hone/agents");
    std::fs::create_dir_all(&project_agents_dir).expect("create project agents dir");
    write_agent_md(&project_agents_dir, "bar.md", "bar", "a project-level agent");

    // working_dir is several levels inside the project.
    let deep_dir = project_dir.path().join("sub/dir/deep");
    std::fs::create_dir_all(&deep_dir).expect("create deep working dir");

    let _guard = HomeGuard::set(home_dir.path());

    let registry = AgentRegistry::load_hierarchical(&deep_dir);

    assert!(
        registry.get("bar").is_some(),
        "project-level agent 'bar' must be found when working_dir is nested deep inside project"
    );
}

// ---------------------------------------------------------------------------
// Test 6 – missing user agents dir is silently skipped
// ---------------------------------------------------------------------------

/// If `~/.config/hone/agents` does not exist the function must not panic or
/// return an error.  The registry must still contain all built-ins.
#[test]
fn load_hierarchical_skips_missing_user_dir() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    // HOME exists but has no .config subdirectory at all.
    let home_dir = tempfile::TempDir::new().expect("create home tempdir");
    let work_dir = tempfile::TempDir::new().expect("create work tempdir");

    let _guard = HomeGuard::set(home_dir.path());

    // Must not panic.
    let registry = AgentRegistry::load_hierarchical(work_dir.path());

    for name in BUILTIN_NAMES {
        assert!(
            registry.get(name).is_some(),
            "built-in '{name}' must be present even when user agents dir is missing"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 7 – project has .git but no .hone/agents — silently skipped
// ---------------------------------------------------------------------------

/// A project with a `.git` marker but without a `.hone/agents/` directory
/// must not cause an error.  The registry must contain all built-ins.
#[test]
fn load_hierarchical_skips_missing_project_agents_dir() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    let home_dir = tempfile::TempDir::new().expect("create home tempdir");

    let project_dir = tempfile::TempDir::new().expect("create project tempdir");
    std::fs::create_dir(project_dir.path().join(".git")).expect("create .git dir");
    // Intentionally do NOT create .hone/agents.

    let _guard = HomeGuard::set(home_dir.path());

    // Must not panic or error.
    let registry = AgentRegistry::load_hierarchical(project_dir.path());

    for name in BUILTIN_NAMES {
        assert!(
            registry.get(name).is_some(),
            "built-in '{name}' must be present even when project agents dir is missing"
        );
    }
}

// ---------------------------------------------------------------------------
// Test 8 – malformed agent file is skipped; well-formed sibling still loads
// ---------------------------------------------------------------------------

/// A `.md` file that lacks valid YAML frontmatter must not abort loading.
/// Any well-formed sibling in the same directory must still be registered.
/// The malformed file must not produce a registration.
#[test]
fn load_hierarchical_continues_on_malformed_agent_file() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    let home_dir = tempfile::TempDir::new().expect("create home tempdir");
    let agents_dir = home_dir.path().join(".config/hone/agents");
    std::fs::create_dir_all(&agents_dir).expect("create user agents dir");

    // Malformed: no frontmatter delimiters at all.
    std::fs::write(agents_dir.join("bad.md"), "not valid frontmatter at all")
        .expect("write bad.md");

    // Well-formed.
    write_agent_md(&agents_dir, "good.md", "good", "a well-formed agent");

    let work_dir = tempfile::TempDir::new().expect("create work tempdir");

    let _guard = HomeGuard::set(home_dir.path());

    // Must not panic or propagate an error.
    let registry = AgentRegistry::load_hierarchical(work_dir.path());

    assert!(
        registry.get("good").is_some(),
        "'good' agent must be registered despite 'bad.md' being malformed"
    );

    assert!(
        registry.get("bad").is_none(),
        "'bad' must NOT be registered — it failed to parse"
    );
}

// ---------------------------------------------------------------------------
// Test 9 – non-.md files in agents dir are ignored
// ---------------------------------------------------------------------------

/// Files without the `.md` extension must be silently skipped.  They must not
/// cause an error, and the valid `.md` agent in the same directory must load.
#[test]
fn load_hierarchical_ignores_non_md_files() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    let home_dir = tempfile::TempDir::new().expect("create home tempdir");
    let agents_dir = home_dir.path().join(".config/hone/agents");
    std::fs::create_dir_all(&agents_dir).expect("create user agents dir");

    // A non-.md file with arbitrary content.
    std::fs::write(agents_dir.join("notes.txt"), "this is not an agent")
        .expect("write notes.txt");

    // A valid agent.
    write_agent_md(&agents_dir, "agent.md", "agent", "a real agent");

    let work_dir = tempfile::TempDir::new().expect("create work tempdir");

    let _guard = HomeGuard::set(home_dir.path());

    // Must not panic.
    let registry = AgentRegistry::load_hierarchical(work_dir.path());

    assert!(
        registry.get("agent").is_some(),
        "'agent' must be registered"
    );

    // Total = 12 built-ins + 1 user agent; notes.txt must not add anything.
    assert_eq!(
        registry.list().len(),
        BUILTIN_NAMES.len() + 1,
        "notes.txt must not be registered as an agent"
    );
}

// ---------------------------------------------------------------------------
// Test 10 – HOME unset falls back gracefully
// ---------------------------------------------------------------------------

/// When the `HOME` environment variable is not set (e.g. in a container),
/// `load_hierarchical` must not panic and must return a registry with all
/// built-ins.
#[test]
fn load_hierarchical_no_home_env_falls_back_gracefully() {
    let _lock = ENV_MUTEX.lock().expect("ENV_MUTEX poisoned");

    let work_dir = tempfile::TempDir::new().expect("create work tempdir");

    let _guard = HomeGuard::unset();

    // Must not panic.
    let registry = AgentRegistry::load_hierarchical(work_dir.path());

    for name in BUILTIN_NAMES {
        assert!(
            registry.get(name).is_some(),
            "built-in '{name}' must be present when HOME is unset"
        );
    }
}
