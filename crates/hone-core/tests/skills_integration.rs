//! Integration test: SkillRegistry wired into AgentConfig injects matched skill
//! bodies into the system-role messages seen by the provider.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;

use hone_core::{Agent, AgentConfig, RetryConfig, Skill, SkillRegistry};
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

fn make_config_with_skills(registry: SkillRegistry) -> AgentConfig {
    AgentConfig {
        provider: Arc::new(MockProvider::new(vec!["ok".to_owned()])),
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "You are a helpful assistant.".to_owned(),
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
        working_dir: std::path::PathBuf::from("."),
        memory_store: None,
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: Some(Arc::new(registry)),
        embedder: None,
    }
}

/// An always-true skill must appear in the system messages sent to the provider.
#[tokio::test]
async fn always_skill_injected_into_provider_messages() {
    let mut registry = SkillRegistry::new();
    registry.register(Skill {
        name: "tdd-always".to_string(),
        description: "TDD is always required".to_string(),
        triggers: vec![],
        tools: vec![],
        model: None,
        always: true,
        body: "USE TDD ALWAYS".to_string(),
    });

    let provider = Arc::new(MockProvider::new(vec!["ok".to_owned()]));
    let last_request = Arc::clone(&provider.last_request);

    let config = AgentConfig {
        provider: provider as Arc<dyn hone_providers::ModelProvider>,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "You are a helpful assistant.".to_owned(),
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
        working_dir: std::path::PathBuf::from("."),
        memory_store: None,
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: Some(Arc::new(registry)),
        embedder: None,
    };

    let mut agent = Agent::new(config);
    let (tx, _rx) = mpsc::channel(32);
    agent.run_turn("hello", tx).await.expect("turn should succeed");

    let req = last_request.lock().unwrap();
    let req = req.as_ref().expect("provider should have received a request");

    // At least one system-role message must contain the skill body.
    let found = req.messages.iter().any(|m| {
        matches!(m.role, hone_proto::Role::System)
            && m.content.as_text().contains("USE TDD ALWAYS")
    });
    assert!(found, "expected 'USE TDD ALWAYS' in a system message; messages: {:#?}", req.messages);
}

/// When skills=None the agent behaves exactly as before — no extra system
/// messages, provider still receives the base system prompt.
#[tokio::test]
async fn no_skills_no_extra_system_message() {
    let provider = Arc::new(MockProvider::new(vec!["ok".to_owned()]));
    let last_request = Arc::clone(&provider.last_request);

    let config = AgentConfig {
        provider: provider as Arc<dyn hone_providers::ModelProvider>,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "You are a helpful assistant.".to_owned(),
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
        working_dir: std::path::PathBuf::from("."),
        memory_store: None,
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None,
        skills: None,
        embedder: None,
    };

    let mut agent = Agent::new(config);
    let (tx, _rx) = mpsc::channel(32);
    agent.run_turn("hello", tx).await.expect("turn should succeed");

    let req = last_request.lock().unwrap();
    let req = req.as_ref().expect("provider should have received a request");

    // Exactly one system message (the base system prompt), no extra injected blocks.
    let system_count = req
        .messages
        .iter()
        .filter(|m| matches!(m.role, hone_proto::Role::System))
        .count();
    assert_eq!(system_count, 1, "expected exactly 1 system message when skills=None");
}

/// A skill with no triggers and always=false must NOT be injected (no match).
#[tokio::test]
async fn non_matching_skill_not_injected() {
    let mut registry = SkillRegistry::new();
    registry.register(Skill {
        name: "deploy-skill".to_string(),
        description: "Deploy to production".to_string(),
        triggers: vec!["deploy to production".to_string()],
        tools: vec![],
        model: None,
        always: false,
        body: "DEPLOY STEPS HERE".to_string(),
    });

    let config = make_config_with_skills(registry);
    // provider is embedded inside config; we can't get last_request here.
    // Instead just verify the turn completes and assert via a fresh provider.
    let provider = Arc::new(MockProvider::new(vec!["ok".to_owned()]));
    let last_request = Arc::clone(&provider.last_request);

    let config2 = AgentConfig {
        provider: provider as Arc<dyn hone_providers::ModelProvider>,
        ..config
    };

    let mut agent = Agent::new(config2);
    let (tx, _rx) = mpsc::channel(32);
    agent.run_turn("hello", tx).await.expect("turn should succeed");

    let req = last_request.lock().unwrap();
    let req = req.as_ref().expect("provider should have received a request");

    let skill_injected = req
        .messages
        .iter()
        .any(|m| m.content.as_text().contains("DEPLOY STEPS HERE"));
    assert!(!skill_injected, "non-matching skill must not appear in messages");
}
