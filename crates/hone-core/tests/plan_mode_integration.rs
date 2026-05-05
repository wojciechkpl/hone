use std::sync::Arc;

use hone_core::{AgentConfig, AgentHandle, AgentMode, PlanModeStore};
use hone_proto::{AgentEvent, ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tempfile::TempDir;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_plan_config(working_dir: std::path::PathBuf) -> AgentConfig {
    let provider = Arc::new(MockProvider::new(vec![
        "This is my investigation plan: step 1, step 2, step 3.".to_string(),
    ]));
    AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "You are a planning assistant.".to_string(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir,
        memory_store: None,
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Plan,
    }
}

// ---------------------------------------------------------------------------
// Test: agent in Plan mode saves a plan file after one turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_mode_agent_saves_plan_after_turn() {
    let dir = TempDir::new().expect("temp dir");
    let working_dir = dir.path().to_path_buf();

    let config = make_plan_config(working_dir.clone());
    let agent = AgentHandle::spawn(config);

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);

    // Run one turn.
    let _result = agent
        .run_turn("investigate the auth module", tx)
        .await
        .expect("turn must succeed");

    // Drain events and find PlanSaved.
    let mut saved_path: Option<std::path::PathBuf> = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::PlanSaved { path } = event {
            saved_path = Some(path);
        }
    }

    // A PlanSaved event must have been emitted.
    let path = saved_path.expect("PlanSaved event must be emitted in Plan mode");

    // The file must exist on disk.
    assert!(path.exists(), "plan file must exist at {:?}", path);

    // The file must be under .hone/plans relative to working_dir.
    let plans_dir = working_dir.join(".hone/plans");
    assert!(
        path.starts_with(&plans_dir),
        "plan file {:?} must be under {:?}",
        path,
        plans_dir
    );

    // The file must be loadable and contain the expected content.
    let loaded = PlanModeStore::load(&path)
        .await
        .expect("plan file must be loadable");

    assert_eq!(loaded.user_prompt, "investigate the auth module");
    assert!(
        !loaded.plan_body.is_empty(),
        "plan_body must not be empty"
    );
    assert!(
        !loaded.slug.is_empty(),
        "slug must not be empty"
    );
}

// ---------------------------------------------------------------------------
// Test: SetMode actor command transitions tool registry correctly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_mode_command_is_idempotent_for_same_mode() {
    let dir = TempDir::new().expect("temp dir");
    let working_dir = dir.path().to_path_buf();

    let config = make_plan_config(working_dir);
    let agent = AgentHandle::spawn(config);

    // Sending SetMode with the same mode twice should not panic or error.
    agent.set_mode(AgentMode::Plan).await;
    agent.set_mode(AgentMode::Plan).await;

    // Cycle to Build and back.
    agent.set_mode(AgentMode::Build).await;
    agent.set_mode(AgentMode::Plan).await;

    // Verify the agent still functions by running a turn.
    let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
    let result = agent.run_turn("simple question", tx).await;
    assert!(result.is_ok(), "turn must succeed after mode switches");
}

// ---------------------------------------------------------------------------
// Test: Build mode does NOT emit PlanSaved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn build_mode_agent_does_not_save_plan() {
    let dir = TempDir::new().expect("temp dir");
    let working_dir = dir.path().to_path_buf();

    // Start in Build mode (default).
    let provider = Arc::new(MockProvider::new(vec![
        "Here is my response.".to_string(),
    ]));
    let config = AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "You are a helpful assistant.".to_string(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
            cache_creation_per_million: None,
        },
        working_dir: working_dir.clone(),
        memory_store: None,
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
    };

    let agent = AgentHandle::spawn(config);
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
    let _result = agent.run_turn("do something", tx).await.expect("turn must succeed");

    // Drain events — PlanSaved must not appear.
    let mut got_plan_saved = false;
    while let Ok(event) = rx.try_recv() {
        if matches!(event, AgentEvent::PlanSaved { .. }) {
            got_plan_saved = true;
        }
    }
    assert!(
        !got_plan_saved,
        "Build mode must not emit PlanSaved events"
    );

    // Also verify no .hone/plans directory was created.
    let plans_dir = working_dir.join(".hone/plans");
    assert!(
        !plans_dir.exists(),
        ".hone/plans dir must not exist in Build mode"
    );
}

// ---------------------------------------------------------------------------
// Test: MockProvider emits ProviderEvent::Done which triggers plan saving
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_mode_filename_uses_prompt_slug() {
    let dir = TempDir::new().expect("temp dir");
    let working_dir = dir.path().to_path_buf();

    let config = make_plan_config(working_dir.clone());
    let agent = AgentHandle::spawn(config);

    let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);
    let _result = agent
        .run_turn("Fix the auth bug quickly", tx)
        .await
        .expect("turn must succeed");

    let mut saved_path: Option<std::path::PathBuf> = None;
    while let Ok(event) = rx.try_recv() {
        if let AgentEvent::PlanSaved { path } = event {
            saved_path = Some(path);
        }
    }

    let path = saved_path.expect("PlanSaved event must be emitted");
    let filename = path.file_name().unwrap().to_str().unwrap();

    // Filename must contain the slug derived from the prompt.
    assert!(
        filename.contains("fix-the-auth-bug-quickly"),
        "filename '{}' must contain slug 'fix-the-auth-bug-quickly'",
        filename
    );
}
