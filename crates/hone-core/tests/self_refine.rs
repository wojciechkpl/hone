use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{Agent, AgentConfig, SelfRefineLoop};
use hone_proto::{Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth, ToolApprovalPolicy};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// SequencedProvider — cycles through a list of pre-configured responses.
// ---------------------------------------------------------------------------

struct SequencedProvider {
    sequences: Vec<Vec<ProviderEvent>>,
    call_count: Mutex<usize>,
}

impl SequencedProvider {
    fn new(sequences: Vec<Vec<ProviderEvent>>) -> Self {
        Self {
            sequences,
            call_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ModelProvider for SequencedProvider {
    fn id(&self) -> &str {
        "sequenced-mock"
    }

    fn display_name(&self) -> &str {
        "Sequenced Mock"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "Test Model".to_owned(),
            context_length: 4096,
            supports_tools: true,
            supports_streaming: true,
        }]
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let idx = {
            let mut count = self.call_count.lock().unwrap();
            let i = *count;
            *count += 1;
            i.min(self.sequences.len().saturating_sub(1))
        };
        let events = self.sequences[idx].clone();
        Ok(Box::pin(stream::iter(events)))
    }

    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        10
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn base_config(provider: Arc<dyn ModelProvider>) -> AgentConfig {
    AgentConfig {
        provider,
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        system_prompt: "You are a helpful coder.".to_owned(),
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
        working_dir: PathBuf::from("."),
        memory_store: None,
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
    }
}

fn token_seq(text: &str) -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::Token {
            text: text.to_owned(),
        },
        ProviderEvent::Done,
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// When the critique turn returns "APPROVED" the loop should stop after the
/// initial generation + one critique turn, reporting `rounds = 2`.
///
/// Call order: [0] generation → [1] critique ("APPROVED")
/// The loop exits without a refine turn.
#[tokio::test]
async fn test_self_refine_approves_immediately() {
    let provider = Arc::new(SequencedProvider::new(vec![
        token_seq("fn add(a: i32, b: i32) -> i32 { a + b }"),
        token_seq("APPROVED"),
    ]));

    let mut agent = Agent::new(base_config(provider));
    let (tx, _rx) = mpsc::channel(32);

    let refine = SelfRefineLoop::new(3);
    let result = refine
        .run(&mut agent, "write an add function", tx)
        .await
        .expect("refine should succeed");

    // 1 generation + 1 critique = 2 rounds
    assert_eq!(result.rounds, 2);
}

/// When the critique identifies problems the loop should execute a refine turn.
/// After one critique + one refine the next critique approves.
///
/// Call order: [0] generation → [1] critique (issues found) → [2] refine
///             → [3] critique ("APPROVED")
/// Total rounds = 4 (gen + crit + refine + crit).
///
/// With max_rounds=3 the loop terminates after:
///   round 1: generation
///   round 2: critique (not APPROVED)
///   round 3: refine
/// because max_rounds is consumed.  We use max_rounds=4 for the "happy path"
/// scenario.
#[tokio::test]
async fn test_self_refine_one_round() {
    let provider = Arc::new(SequencedProvider::new(vec![
        // Turn 1: initial generation
        token_seq("fn add(a: i32, b: i32) -> i32 { a + b }"),
        // Turn 2: critique — identifies an issue
        token_seq("The function does not handle overflow. Consider using checked_add."),
        // Turn 3: refine
        token_seq("fn add(a: i32, b: i32) -> Option<i32> { a.checked_add(b) }"),
        // Turn 4: critique — approves
        token_seq("APPROVED"),
    ]));

    let mut agent = Agent::new(base_config(provider));
    let (tx, _rx) = mpsc::channel(32);

    // max_rounds=4 allows: gen(1) + crit(2) + refine(3) + crit(4)
    let refine = SelfRefineLoop::new(4);
    let result = refine
        .run(&mut agent, "write an add function", tx)
        .await
        .expect("refine should succeed");

    // Completed: generation + critique + refine + critique = 4 rounds
    assert_eq!(result.rounds, 4);
    assert!(result.final_response.contains("checked_add"));
}
