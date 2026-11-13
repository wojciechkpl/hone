//! AS end-to-end: a real Agent + InMemoryStore memory + the real Anthropic
//! adapter against a local mock server. The request body's `system` must
//! contain the "Relevant Memories" block.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::sync::Arc;

use hone_core::{Agent, AgentConfig, AgentMode};
use hone_memory::{InMemoryStore, Memory, MemoryKind, MemoryStore};
use hone_proto::{ModelId, PricingEntry, ToolApprovalPolicy};
use hone_providers::AnthropicProvider;
use hone_sandbox::NullSandbox;
use hone_tools::ToolRegistry;
use httpmock::prelude::*;
use tokio::sync::mpsc;

const SSE: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"model\":\"claude-sonnet-4-5\",\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n\
event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"ok\"}}\n\n\
event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":1}}\n\n\
event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";

#[tokio::test]
async fn agent_turn_sends_relevant_memories_in_anthropic_system_field() {
    let server = MockServer::start_async().await;
    // Matches only when the top-level `system` array carries a Relevant Memories block.
    let mock = server
        .mock_async(|when, then| {
            when.method(POST).path("/v1/messages").matches(|req| {
                let Some(body) = req.body.as_ref() else {
                    return false;
                };
                let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else {
                    return false;
                };
                v["system"].as_array().is_some_and(|blocks| {
                    blocks.iter().any(|b| {
                        b["text"]
                            .as_str()
                            .is_some_and(|t| t.contains("Relevant Memories"))
                    })
                })
            });
            then.status(200)
                .header("content-type", "text/event-stream")
                .body(SSE);
        })
        .await;

    let query_text = "quokka habitat topic";
    let store: Arc<dyn MemoryStore> = {
        let s = InMemoryStore::new();
        s.record(Memory {
            id: "01935b0c-aabb-7000-8000-000000000002".to_owned(),
            content: format!("remember the {query_text} details"),
            tags: vec![],
            weight: 1.0,
            created_at: 0,
            decay_at: None,
            kind: MemoryKind::Project,
            description: Some("quokka note".to_owned()),
            embedding: None,
        })
        .await
        .unwrap();
        Arc::new(s)
    };

    let model = ModelId::new("anthropic:claude-sonnet-4-5");
    let config = AgentConfig {
        provider: Arc::new(AnthropicProvider::with_base_url("test-key", server.base_url())),
        tools: ToolRegistry::new(),
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "You are a helpful assistant.".to_owned(),
        model: model.clone(),
        max_tokens: None,
        temperature: None,
        budget_limit: None,
        pricing: PricingEntry {
            model,
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
        hooks: None,
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
        policy: None,
    };

    let mut agent = Agent::new(config);
    let (tx, mut rx) = mpsc::channel(64);
    let _ = agent.run_turn(query_text, tx).await;
    while rx.try_recv().is_ok() {}

    assert!(
        mock.hits_async().await >= 1,
        "no request reached the Anthropic mock with a Relevant Memories block in its `system` array"
    );
}
