use std::sync::Arc;

use hone_core::{AgentConfig, AgentEnsemble};
use hone_proto::{ModelId, PricingEntry};
use hone_providers::MockProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn make_config(tokens: Vec<String>) -> AgentConfig {
    AgentConfig {
        provider: Arc::new(MockProvider::new(tokens)),
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
        },
        working_dir: std::path::PathBuf::from("."),
        memory_store: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ensemble_runs_all_candidates() {
    // Three candidates produce distinct responses.
    let responses = ["candidate one", "candidate two", "candidate three"];

    let candidate_factories: Vec<Box<dyn Fn() -> AgentConfig + Send + Sync>> = responses
        .iter()
        .map(|&r| {
            let r = r.to_owned();
            let factory: Box<dyn Fn() -> AgentConfig + Send + Sync> =
                Box::new(move || make_config(vec![r.clone()]));
            factory
        })
        .collect();

    // Ranker returns "2" → winner_index 1 (zero-based).
    let ranker_factory: Box<dyn Fn() -> AgentConfig + Send + Sync> =
        Box::new(|| make_config(vec!["2".to_owned()]));

    let ensemble = AgentEnsemble::new(candidate_factories, ranker_factory);
    let (tx, _rx) = mpsc::channel(256);

    let result = ensemble
        .run("What is the answer?", tx)
        .await
        .expect("ensemble should succeed");

    // All 3 candidates must have been collected.
    assert_eq!(result.all_candidates.len(), 3);

    // Ranker said "2" → 0-based index 1.
    assert_eq!(result.winner_index, 1);
    assert_eq!(result.winner.response, "candidate two");

    // Ranker response is preserved.
    assert_eq!(result.ranker_response, "2");
}

#[tokio::test]
async fn test_ensemble_falls_back_to_first_on_invalid_ranker_response() {
    let candidate_factories: Vec<Box<dyn Fn() -> AgentConfig + Send + Sync>> = ["alpha", "beta"]
        .iter()
        .map(|&r| {
            let r = r.to_owned();
            let factory: Box<dyn Fn() -> AgentConfig + Send + Sync> =
                Box::new(move || make_config(vec![r.clone()]));
            factory
        })
        .collect();

    // Ranker returns non-numeric text → fallback to index 0.
    let ranker_factory: Box<dyn Fn() -> AgentConfig + Send + Sync> =
        Box::new(|| make_config(vec!["blah blah".to_owned()]));

    let ensemble = AgentEnsemble::new(candidate_factories, ranker_factory);
    let (tx, _rx) = mpsc::channel(256);

    let result = ensemble
        .run("Tell me something", tx)
        .await
        .expect("ensemble should succeed even with garbled ranker output");

    assert_eq!(result.winner_index, 0);
    assert_eq!(result.winner.response, "alpha");
}

#[tokio::test]
async fn test_ensemble_handles_candidate_failure() {
    use std::pin::Pin;

    use async_trait::async_trait;
    use futures::{Stream, stream};
    use hone_proto::{Message, ModelInfo, ProviderEvent, ProviderHealth};
    use hone_providers::{CompletionRequest, ModelProvider, ProviderError};

    /// A provider that always returns an HTTP error.
    struct FailingProvider;

    #[async_trait]
    impl ModelProvider for FailingProvider {
        fn id(&self) -> &str {
            "failing"
        }
        fn display_name(&self) -> &str {
            "Failing Provider"
        }
        fn supported_models(&self) -> Vec<ModelInfo> {
            vec![]
        }
        async fn complete(
            &self,
            _request: CompletionRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
            // Emit an Error event so the agent returns Err.
            let events = vec![
                ProviderEvent::Error {
                    message: "simulated failure".to_owned(),
                },
                ProviderEvent::Done,
            ];
            Ok(Box::pin(stream::iter(events)))
        }
        fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
            1
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::Unavailable {
                reason: "intentional".to_owned(),
            }
        }
    }

    // Slot 0: good candidate.
    // Slot 1: failing candidate.
    // Slot 2: good candidate.
    let candidate_factories: Vec<Box<dyn Fn() -> AgentConfig + Send + Sync>> = vec![
        Box::new(|| make_config(vec!["good one".to_owned()])),
        Box::new(|| AgentConfig {
            provider: Arc::new(FailingProvider),
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
            },
            working_dir: std::path::PathBuf::from("."),
            memory_store: None,
        }),
        Box::new(|| make_config(vec!["good three".to_owned()])),
    ];

    // Ranker returns "1" → first surviving candidate.
    let ranker_factory: Box<dyn Fn() -> AgentConfig + Send + Sync> =
        Box::new(|| make_config(vec!["1".to_owned()]));

    let ensemble = AgentEnsemble::new(candidate_factories, ranker_factory);
    let (tx, _rx) = mpsc::channel(256);

    let result = ensemble
        .run("What happened?", tx)
        .await
        .expect("ensemble should succeed with 2 of 3 candidates");

    // Only the 2 non-failing candidates are present.
    assert_eq!(result.all_candidates.len(), 2);

    // Ranker said "1" → 0-based index 0.
    assert_eq!(result.winner_index, 0);
}
