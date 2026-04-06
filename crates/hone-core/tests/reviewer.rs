use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{AgentConfig, ReviewSeverity, ReviewerAgent};
use hone_proto::{Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// SequencedProvider — copied from agent_turn tests so we can control per-call
// responses without reaching into private modules.
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
        system_prompt: "You are a coder.".to_owned(),
        model: ModelId::new("mock:test"),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model: ModelId::new("mock:test"),
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: None,
        },
        working_dir: PathBuf::from("."),
    }
}

fn single_response_events(text: &str) -> Vec<ProviderEvent> {
    vec![
        ProviderEvent::Token { text: text.to_owned() },
        ProviderEvent::Done,
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A model that returns a JSON array of issues should populate `ReviewResult`
/// with the correct count and severity values.
#[tokio::test]
async fn test_reviewer_parses_issues() {
    let json_response = r#"[
        {"severity": "critical", "description": "null pointer dereference", "file": "main.rs", "suggestion": "add null check"},
        {"severity": "minor", "description": "unused variable", "file": "lib.rs", "suggestion": "remove or use the variable"}
    ]"#;

    let provider = Arc::new(SequencedProvider::new(vec![
        single_response_events(json_response),
    ]));

    let mut reviewer = ReviewerAgent::new(base_config(provider));
    let (tx, _rx) = mpsc::channel(32);

    let result = reviewer
        .review("diff content here", tx)
        .await
        .expect("review should succeed");

    assert_eq!(result.issues.len(), 2);
    assert_eq!(result.issues[0].severity, ReviewSeverity::Critical);
    assert_eq!(result.issues[1].severity, ReviewSeverity::Minor);
    // Critical issue → not approved.
    assert!(!result.approved);
}

/// An empty JSON array means no issues; the code should be approved.
#[tokio::test]
async fn test_reviewer_approves_clean_code() {
    let provider = Arc::new(SequencedProvider::new(vec![
        single_response_events("[]"),
    ]));

    let mut reviewer = ReviewerAgent::new(base_config(provider));
    let (tx, _rx) = mpsc::channel(32);

    let result = reviewer
        .review("clean diff", tx)
        .await
        .expect("review should succeed");

    assert!(result.issues.is_empty());
    assert!(result.approved);
}

/// When the model returns invalid (non-JSON) output the reviewer should
/// fail-open: `approved = true` and `issues` is empty.
#[tokio::test]
async fn test_reviewer_handles_invalid_json() {
    let provider = Arc::new(SequencedProvider::new(vec![
        single_response_events("I cannot review this code right now, sorry."),
    ]));

    let mut reviewer = ReviewerAgent::new(base_config(provider));
    let (tx, _rx) = mpsc::channel(32);

    let result = reviewer
        .review("some context", tx)
        .await
        .expect("review should succeed even with unparseable output");

    // Fail-open: unparseable output must not block the pipeline.
    assert!(result.approved);
    assert!(result.issues.is_empty());
}
