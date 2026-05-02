//! Red-phase integration test: `MemoryRecalled` event entries carry friendly
//! keys, NOT raw UUIDs.
//!
//! This test MUST fail today because:
//!   1. `hone_core::friendly_memory_key` does not exist yet.
//!   2. `fetch_relevant_memories` still uses `m.id.clone()` as the first tuple
//!      element instead of calling `friendly_memory_key`.
//!
//! Do not modify this test during the green phase — it is the contract.

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::{Agent, AgentConfig};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use hone_proto::{
    AgentEvent, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth, ToolApprovalPolicy,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Minimal mock provider: always returns a single text token then Done.
// ---------------------------------------------------------------------------

struct TrivialProvider;

#[async_trait]
impl ModelProvider for TrivialProvider {
    fn id(&self) -> &str {
        "trivial-mock"
    }

    fn display_name(&self) -> &str {
        "Trivial Mock"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "Test Model".to_owned(),
            context_length: 4096,
            supports_tools: false,
            supports_streaming: true,
        }]
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![
            ProviderEvent::Token {
                text: "ok".to_owned(),
            },
            ProviderEvent::Done,
        ])))
    }

    fn estimate_tokens(&self, _: &[hone_proto::Message]) -> u32 {
        10
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

// ---------------------------------------------------------------------------
// Test 9 — MemoryRecalled event entries use friendly keys, not raw UUIDs
// ---------------------------------------------------------------------------

/// Spin up an Agent with a MemoryStore containing one memory whose
/// `description` is `Some("known label")`.  Run a turn with user input that
/// matches the memory content so it is recalled.  Drain events and find
/// `AgentEvent::MemoryRecalled`.  Assert:
///   - The first entry's key equals `"known label"` (not any UUID).
///   - The key does NOT look like a UUIDv7 (8-4-4-4-12 hex pattern).
///
/// After the green phase the agent must call `friendly_memory_key` when
/// building `memory_entries` in `fetch_relevant_memories`, replacing the
/// current `m.id.clone()`.
#[tokio::test]
async fn memory_recalled_event_uses_friendly_key_not_uuid() {
    // The memory content contains the query text verbatim so the BM25 scorer
    // surfaces it (same strategy as `test_memories_injected_as_separate_message`
    // in agent_turn.rs).
    let query_text = "known label topic";

    let store: Arc<dyn hone_memory::MemoryStore> = {
        let s = InMemoryStore::new();
        s.record(Memory {
            // Use a real-looking UUIDv7 so the test proves it is NOT returned.
            id: "01935b0c-aabb-7000-8000-000000000001".to_owned(),
            content: format!("this memory is about {query_text}"),
            tags: vec![],
            weight: 1.0,
            created_at: 0,
            decay_at: None,
            kind: MemoryKind::Project,
            description: Some("known label".to_owned()),
        })
        .await
        .unwrap();
        Arc::new(s)
    };

    let config = AgentConfig {
        provider: Arc::new(TrivialProvider),
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
        working_dir: std::path::PathBuf::from("/tmp"),
        memory_store: Some(Arc::clone(&store)),
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
    };

    let mut agent = Agent::new(config);
    let (tx, mut rx) = mpsc::channel(32);

    agent
        .run_turn(query_text, tx)
        .await
        .expect("turn should succeed");

    // Collect all events.
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    // There must be at least one MemoryRecalled event.
    let recalled_event = events.iter().find_map(|e| {
        if let AgentEvent::MemoryRecalled { entries } = e {
            Some(entries.clone())
        } else {
            None
        }
    });

    let entries = recalled_event.expect(
        "expected AgentEvent::MemoryRecalled to be emitted; \
         check that the query text appears in the memory content",
    );

    assert!(
        !entries.is_empty(),
        "MemoryRecalled entries must not be empty"
    );

    let first_key = &entries[0].0;

    // The key must be the friendly description, not the raw UUID.
    assert_eq!(
        first_key, "known label",
        "MemoryRecalled first entry key must equal the memory description \
         'known label'; got: {first_key:?}. \
         This fails until fetch_relevant_memories calls friendly_memory_key."
    );

    // Belt-and-suspenders: the key must NOT look like a UUID (8-4-4-4-12).
    let uuid_re =
        regex::Regex::new(r"^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$")
            .unwrap();
    assert!(
        !uuid_re.is_match(first_key),
        "MemoryRecalled key must NOT be a raw UUID; got: {first_key:?}"
    );
}
