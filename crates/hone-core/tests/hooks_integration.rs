/// Integration test: verify that a `PreToolUse` hook with `deny: true`
/// prevents the tool from being invoked and injects a synthetic tool-result
/// into the context.
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_core::hooks::{HookRegistry, HookSpec};
use hone_core::{Agent, AgentConfig, AgentMode};
use hone_proto::{
    AgentEvent, Message, ModelId, ModelInfo, PricingEntry, ProviderEvent, ProviderHealth,
    ToolApprovalPolicy, ToolCallRequest, ToolLayer,
};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_sandbox::NullSandbox;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput, ToolRegistry};
use serde_json::json;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// Mini mock provider: emits ONE tool call then a final text response.
// ---------------------------------------------------------------------------

struct TwoStepProvider {
    call_count: Mutex<usize>,
}

impl TwoStepProvider {
    fn new() -> Self {
        Self {
            call_count: Mutex::new(0),
        }
    }
}

#[async_trait]
impl ModelProvider for TwoStepProvider {
    fn id(&self) -> &str {
        "two-step-mock"
    }

    fn display_name(&self) -> &str {
        "Two-step Mock"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![ModelInfo {
            id: ModelId::new("mock:test"),
            display_name: "Mock".to_owned(),
            context_length: 4096,
            supports_tools: true,
            supports_streaming: true,
        }]
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        let call = {
            let mut n = self.call_count.lock().unwrap();
            let v = *n;
            *n += 1;
            v
        };

        let events: Vec<ProviderEvent> = if call == 0 {
            // First LLM call: emit a tool call to "shell".
            vec![
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "call-001".to_owned(),
                    name: "shell".to_owned(),
                    arguments: json!({ "command": "rm -rf /" }),
                }),
                ProviderEvent::Done,
            ]
        } else {
            // Second LLM call (after tool result injected): emit final text.
            vec![
                ProviderEvent::Token {
                    text: "done".to_owned(),
                },
                ProviderEvent::Done,
            ]
        };
        Ok(Box::pin(stream::iter(events)))
    }

    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        1
    }

    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

// ---------------------------------------------------------------------------
// A shell tool that MUST NOT be called — panics if invoked.
// ---------------------------------------------------------------------------

struct PanicShellTool;

#[async_trait]
impl Tool for PanicShellTool {
    fn name(&self) -> &str {
        "shell"
    }
    fn description(&self) -> &str {
        "Shell — should never be called when hook denies."
    }
    fn schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }
    async fn call(
        &self,
        _input: ToolInput,
        _ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        panic!("shell tool was invoked despite hook deny — test failed");
    }
    fn is_read_only(&self) -> bool {
        false
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

/// Build a `HookRegistry` with one `PreToolUse` hook that unconditionally
/// denies any "shell" tool call.
fn deny_shell_registry() -> HookRegistry {
    let mut registry = HookRegistry::new();
    registry.register(HookSpec {
        name: "deny-shell".to_owned(),
        event: "PreToolUse".to_owned(),
        // Discard stdin, emit deny JSON.
        command: r#"cat > /dev/null && echo '{"deny":true,"message":"no shell allowed","mutated_input":null}'"#
            .to_owned(),
        timeout_ms: 5000,
        tool_name_pattern: Some("shell".to_owned()),
    });
    registry
}

#[tokio::test]
async fn pre_tool_use_hook_deny_prevents_shell_invocation() {
    let registry = deny_shell_registry();

    let mut tools = ToolRegistry::builder();
    tools = tools.register(Box::new(PanicShellTool));
    let tools = tools.build();

    let config = AgentConfig {
        provider: Arc::new(TwoStepProvider::new()),
        tools,
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "You are helpful.".to_owned(),
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
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: Some(Arc::new(registry)),
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
    };

    let mut agent = Agent::new(config);
    let (tx, mut rx) = mpsc::channel::<AgentEvent>(64);

    // Run one turn — should not panic.
    let result = agent
        .run_turn("run a shell command please", tx)
        .await
        .expect("turn should complete without error");

    // Drain events.
    let mut events: Vec<AgentEvent> = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        events.push(ev);
    }

    // The turn should have completed (returning "done" from the second LLM call).
    assert_eq!(
        result.response, "done",
        "expected final response to be the second LLM token"
    );

    // The context must contain a tool-result message with "no shell allowed".
    let messages = agent.messages();
    let has_deny_result = messages.iter().any(|m| {
        m.content
            .as_text()
            .contains("no shell allowed")
    });
    assert!(
        has_deny_result,
        "expected context to contain a tool-result with 'no shell allowed'; messages: {messages:?}"
    );
}

/// Verify that hooks=None is a true no-op: the turn completes normally and
/// a real (non-panicking) tool is invoked.
#[tokio::test]
async fn no_hooks_no_overhead() {
    struct PassThrough;
    #[async_trait]
    impl Tool for PassThrough {
        fn name(&self) -> &str {
            "echo_tool"
        }
        fn description(&self) -> &str {
            "echo"
        }
        fn schema(&self) -> serde_json::Value {
            json!({ "type": "object" })
        }
        async fn call(
            &self,
            input: ToolInput,
            _ctx: &ToolContext,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::new(input.0))
        }
        fn is_read_only(&self) -> bool {
            false
        }
        fn layer(&self) -> ToolLayer {
            ToolLayer::Native
        }
    }

    struct EchoProvider {
        call_count: Mutex<usize>,
    }
    impl EchoProvider {
        fn new() -> Self {
            Self {
                call_count: Mutex::new(0),
            }
        }
    }
    #[async_trait]
    impl ModelProvider for EchoProvider {
        fn id(&self) -> &str {
            "echo-mock"
        }
        fn display_name(&self) -> &str {
            "Echo Mock"
        }
        fn supported_models(&self) -> Vec<ModelInfo> {
            vec![ModelInfo {
                id: ModelId::new("mock:test"),
                display_name: "Mock".to_owned(),
                context_length: 4096,
                supports_tools: true,
                supports_streaming: true,
            }]
        }
        async fn complete(
            &self,
            _request: CompletionRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
            let call = {
                let mut n = self.call_count.lock().unwrap();
                let v = *n;
                *n += 1;
                v
            };
            let events: Vec<ProviderEvent> = if call == 0 {
                vec![
                    ProviderEvent::ToolCall(ToolCallRequest {
                        id: "c1".to_owned(),
                        name: "echo_tool".to_owned(),
                        arguments: json!({"hello": "world"}),
                    }),
                    ProviderEvent::Done,
                ]
            } else {
                vec![
                    ProviderEvent::Token {
                        text: "ok".to_owned(),
                    },
                    ProviderEvent::Done,
                ]
            };
            Ok(Box::pin(stream::iter(events)))
        }
        fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
            1
        }
        async fn health(&self) -> ProviderHealth {
            ProviderHealth::Healthy
        }
    }

    let mut tools = ToolRegistry::builder();
    tools = tools.register(Box::new(PassThrough));
    let tools = tools.build();

    let config = AgentConfig {
        provider: Arc::new(EchoProvider::new()),
        tools,
        sandbox: Arc::new(NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        system_prompt: "You are helpful.".to_owned(),
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
        retry_config: hone_core::RetryConfig::default(),
        index: None,
        inject_context: false,
        skeleton_strategy: Default::default(),
        egress_log: None,
        approval_policy: ToolApprovalPolicy::Auto,
        step_rx: None,
        hooks: None, // no hooks — must be a perfect no-op
        skills: None,
        embedder: None,
        mode: AgentMode::Build,
    };

    let mut agent = Agent::new(config);
    let (tx, _rx) = mpsc::channel::<AgentEvent>(64);
    let result = agent
        .run_turn("do the echo tool", tx)
        .await
        .expect("turn must complete");
    assert_eq!(result.response, "ok");
    assert_eq!(result.tool_calls_count, 1, "tool must have been invoked");
}
