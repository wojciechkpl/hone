//! P2.5: `AppState::new` reads `HONE_POLICY_BUILTINS` once; `with_mock_provider` turns it off
//! (`docs/plans/policy-p2.5-spec.md` section 2).
//!
//! This file holds a single test because it mutates the process environment.
//! API assumption: `AppState::new` reads the variable (via `builtins_enabled`) into the
//! public field `policy_builtins`, so honed picks it up with no extra wiring.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_server::AppState;

struct Nop;

#[async_trait]
impl ModelProvider for Nop {
    fn id(&self) -> &str {
        "nop"
    }
    fn display_name(&self) -> &str {
        "Nop"
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
        _request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = ProviderEvent> + Send>>, ProviderError> {
        Ok(Box::pin(stream::iter(vec![ProviderEvent::Done])))
    }
    fn estimate_tokens(&self, _messages: &[Message]) -> u32 {
        1
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
}

#[test]
fn new_reads_the_env_once_and_with_mock_provider_turns_it_off() {
    // SAFETY: this is the only test in this binary, so nothing else touches the environment.
    unsafe {
        std::env::remove_var("HONE_POLICY_BUILTINS");
    }
    assert!(!AppState::new(None).policy_builtins, "unset means off");

    unsafe {
        std::env::set_var("HONE_POLICY_BUILTINS", "1");
    }
    assert!(AppState::new(None).policy_builtins, "exactly 1 means on");
    assert!(
        !AppState::new(None)
            .with_mock_provider(Arc::new(Nop))
            .policy_builtins,
        "with_mock_provider must turn built-ins off"
    );

    for v in ["0", "true", "yes", ""] {
        unsafe {
            std::env::set_var("HONE_POLICY_BUILTINS", v);
        }
        assert!(!AppState::new(None).policy_builtins, "{v:?}");
    }
    unsafe {
        std::env::remove_var("HONE_POLICY_BUILTINS");
    }
}
