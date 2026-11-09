//! P2.3 single-implementation and ensemble tests
//! (`docs/plans/policy-p2.3-spec.md` sections 2 and 8).

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_config::Decision;
use hone_config::policy::{CompiledPolicy, PolicyContext, PolicyRule, Subject};
use hone_core::{AgentConfig, AgentEnsemble, AgentMode, RetryConfig};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    StepAction, ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{
    SubjectContext, Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry, ToolSubjects,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;

// -- single implementation ------------------------------------------------------

type DecideFn = for<'a, 'b, 'c, 'd> fn(
    &'a hone_tools::policy_gate::PolicyConfig,
    &'b HashSet<String>,
    Option<&'c dyn Tool>,
    &'d ToolCallRequest,
) -> hone_tools::policy_gate::GateDecision;

#[test]
fn hone_core_decide_is_the_hone_tools_decide() {
    // Coercing both paths to one fn-pointer type (built from hone_tools types) fails to
    // compile if hone_core had its own PolicyConfig/GateDecision.
    let core: DecideFn = hone_core::decide;
    let tools: DecideFn = hone_tools::policy_gate::decide;
    assert_eq!(core as usize, tools as usize);
}

#[test]
fn hone_core_reexports_the_same_types() {
    assert_eq!(
        std::any::type_name::<hone_core::PolicyConfig>(),
        std::any::type_name::<hone_tools::policy_gate::PolicyConfig>()
    );
    assert_eq!(
        std::any::type_name::<hone_core::GateDecision>(),
        std::any::type_name::<hone_tools::policy_gate::GateDecision>()
    );
}

// -- ensemble candidates are headless -------------------------------------------

struct AskTool {
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for AskTool {
    fn name(&self) -> &str {
        "t"
    }
    fn description(&self) -> &str {
        "fake"
    }
    fn schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn call(&self, _i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(ToolOutput::new(json!({"ok": true})))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
    fn subjects(&self, _i: &ToolInput, _s: &SubjectContext) -> ToolSubjects {
        ToolSubjects {
            subjects: vec![Subject::Command("deploy prod".into())],
            unparsed: false,
            ..Default::default()
        }
    }
}

struct Scripted {
    sequences: Vec<Vec<ProviderEvent>>,
    n: Mutex<usize>,
}

#[async_trait]
impl ModelProvider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }
    fn display_name(&self) -> &str {
        "Scripted"
    }
    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "Test".to_owned(),
            context_length: 4096,
            supports_tools: true,
            supports_streaming: true,
        }]
    }
    async fn complete(
        &self,
        _r: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let idx = {
            let mut n = self.n.lock().unwrap();
            let i = *n;
            *n += 1;
            i.min(self.sequences.len() - 1)
        };
        Ok(Box::pin(stream::iter(self.sequences[idx].clone())))
    }
    fn estimate_tokens(&self, _m: &[Message]) -> u32 {
        10
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

#[tokio::test]
async fn ensemble_candidates_fail_closed_instead_of_prompting_on_a_shared_channel() {
    let workdir = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    let context = PolicyContext::for_platform(workdir.clone(), None);
    let compiled = CompiledPolicy::compile(
        &context,
        &[],
        &[PolicyRule::command("deploy*", Decision::Ask).with_id("ask-deploy")],
        &[],
    )
    .unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let registry = ToolRegistry::builder()
        .register(Box::new(AskTool {
            calls: calls.clone(),
        }))
        .build();

    let first = vec![
        ProviderEvent::ToolCall(ToolCallRequest {
            id: "c1".to_owned(),
            name: "t".to_owned(),
            arguments: json!({}),
        }),
        ProviderEvent::Done,
    ];
    let second = vec![
        ProviderEvent::Token {
            text: "done".to_owned(),
        },
        ProviderEvent::Done,
    ];
    let provider = Arc::new(Scripted {
        sequences: vec![first, second],
        n: Mutex::new(0),
    });

    // The prototype is interactive: an open, empty step channel. A candidate that
    // inherited it would block forever on its approval prompt.
    let (_tx, rx) = mpsc::channel::<StepAction>(4);
    let proto = AgentConfig {
        provider,
        tools: registry,
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "test".to_owned(),
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
        working_dir: workdir,
        memory_store: None,
        retry_config: RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: Some(Arc::new(tokio::sync::Mutex::new(rx))),
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: Some(hone_core::PolicyConfig {
            compiled: Arc::new(compiled),
            context,
            pre_approved: vec![],
            builtins: false,
        }),
    };

    let ensemble = AgentEnsemble::new(proto, vec![0.0]);
    let (tx, mut events) = mpsc::channel(256);
    let outcome = tokio::time::timeout(Duration::from_secs(10), ensemble.run("go", tx)).await;
    assert!(
        outcome.is_ok(),
        "ensemble candidate hung on an approval prompt"
    );

    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "an unapproved ask must not dispatch"
    );
    let mut prompts = 0;
    while let Ok(e) = events.try_recv() {
        if matches!(
            e,
            AgentEvent::ApprovalRequired { .. } | AgentEvent::ToolCallPaused { .. }
        ) {
            prompts += 1;
        }
    }
    assert_eq!(prompts, 0, "candidates are headless and must never prompt");
}
