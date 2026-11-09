//! P2.5 section 14.8: the tool cache never bypasses the read filter.
//!
//! `CachingToolWrapper` neither serves nor stores a result when `ctx.read_filter` is `Some`.
//!
//! WIRING NEEDED (this file is a test file; the `mod` line lives in production source, which
//! the test author may not edit): add to the end of `caching_tool.rs`
//!
//! ```ignore
//! #[cfg(test)]
//! #[path = "caching_tool_readfilter_tests.rs"]
//! mod readfilter_tests;
//! ```
//!
//! Until that line exists these tests are not compiled, so they cannot be run red.

use super::*;
use hone_cache::{InMemoryBackend, ToolResultCache};
use hone_tools::policy_gate::ReadAccess;
use std::sync::atomic::{AtomicU32, Ordering};

struct CountingGlob {
    calls: Arc<AtomicU32>,
}

#[async_trait]
impl Tool for CountingGlob {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "test"
    }
    fn schema(&self) -> Value {
        serde_json::json!({})
    }
    async fn call(&self, _i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(ToolOutput::new(serde_json::json!({"files": ["a.rs"]})))
    }
    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }
}

fn ctx(filtered: bool) -> ToolContext {
    ToolContext {
        sandbox: Arc::new(hone_sandbox::NullSandbox),
        sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
        working_dir: std::env::current_dir().unwrap(),
        read_filter: if filtered {
            Some(Arc::new(|_: &std::path::Path| ReadAccess::Allowed))
        } else {
            None
        },
    }
}

fn wrapper(calls: &Arc<AtomicU32>) -> CachingToolWrapper {
    let cache = Arc::new(ToolResultCache::new(Arc::new(InMemoryBackend::new())));
    CachingToolWrapper::new(
        Arc::new(CountingGlob {
            calls: Arc::clone(calls),
        }),
        cache,
    )
    .with_shadow_rate(0.0)
}

fn input() -> ToolInput {
    ToolInput::new(serde_json::json!({"pattern": "*.rs"}))
}

#[tokio::test]
async fn without_a_read_filter_the_second_call_is_a_cache_hit() {
    let calls = Arc::new(AtomicU32::new(0));
    let w = wrapper(&calls);
    let c = ctx(false);
    let first = w.call(input(), &c).await.unwrap();
    let second = w.call(input(), &c).await.unwrap();
    assert!(!first.cache_hit);
    assert!(second.cache_hit);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
}

#[tokio::test]
async fn with_a_read_filter_every_call_runs_the_tool_and_nothing_is_served_from_cache() {
    let calls = Arc::new(AtomicU32::new(0));
    let w = wrapper(&calls);
    let c = ctx(true);
    let first = w.call(input(), &c).await.unwrap();
    let second = w.call(input(), &c).await.unwrap();
    assert!(!first.cache_hit);
    assert!(!second.cache_hit);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn a_result_cached_without_a_filter_is_not_served_to_a_filtered_call() {
    let calls = Arc::new(AtomicU32::new(0));
    let w = wrapper(&calls);
    let _ = w.call(input(), &ctx(false)).await.unwrap(); // populates the cache
    let filtered = w.call(input(), &ctx(true)).await.unwrap();
    assert!(!filtered.cache_hit, "must not serve the unfiltered result");
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn a_filtered_result_is_not_stored_for_later_unfiltered_calls() {
    let calls = Arc::new(AtomicU32::new(0));
    let w = wrapper(&calls);
    let _ = w.call(input(), &ctx(true)).await.unwrap();
    let unfiltered = w.call(input(), &ctx(false)).await.unwrap();
    assert!(!unfiltered.cache_hit, "the filtered call must not have stored");
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}
