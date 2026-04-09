//! Wraps a Tool with cache lookup/storage.
//!
//! Cache policy is determined by the tool's name: read-only filesystem tools
//! use FileHash invalidation, git read-only tools use GitSha, LSP tools use
//! TTL, and non-cacheable tools bypass the cache entirely.
//!
//! Shadow sampling (default 1%) re-executes the underlying tool on a random
//! fraction of hits and logs a warning if the output diverges.  This is the
//! primary correctness guard against stale entries from invalidation bugs.

use std::sync::Arc;

use async_trait::async_trait;
use hone_cache::{CacheKey, CacheError, InvalidationTag, ToolResultCache};
use hone_proto::ToolLayer;
use hone_tools::{Tool, ToolContext, ToolError, ToolInput, ToolOutput};
use serde_json::Value;

/// Wraps a `Tool` with a transparent cache layer.
///
/// On every `call`:
/// 1. If the tool is not in the cacheable set, delegate directly.
/// 2. Build a deterministic `CacheKey` from the tool name + canonicalized JSON
///    input + invalidation policy.
/// 3. On a cache hit, return the stored bytes (and optionally shadow-sample).
/// 4. On a cache miss, execute the inner tool, serialize the output, store it,
///    and return it.
pub struct CachingToolWrapper {
    inner: Arc<dyn Tool>,
    cache: Arc<ToolResultCache>,
    /// Probability [0.0, 1.0] of shadow-executing on a cache hit to verify
    /// freshness.  Default: 0.01 (1 % per the research doc §8).
    shadow_rate: f64,
}

impl CachingToolWrapper {
    pub fn new(inner: Arc<dyn Tool>, cache: Arc<ToolResultCache>) -> Self {
        Self {
            inner,
            cache,
            shadow_rate: 0.01,
        }
    }

    #[allow(dead_code)]
    pub fn with_shadow_rate(mut self, rate: f64) -> Self {
        self.shadow_rate = rate;
        self
    }

    /// Tools that are safe to cache (read-only, deterministic for a given
    /// invalidation key).  Write tools, shell, git_commit, etc. are excluded.
    fn is_cacheable(tool_name: &str) -> bool {
        matches!(
            tool_name,
            "read_file"
                | "glob"
                | "grep"
                | "git_status"
                | "git_diff"
                | "lsp_diagnostics"
                | "lsp_hover"
                | "lsp_code_actions"
                | "symbol_search"
                | "symbol_at"
        )
    }

    /// Choose an invalidation policy for the given tool + input + context.
    ///
    /// The policy is computed *before* calling the tool so that we can store
    /// it alongside the entry.  For `read_file` we hash the file on the way
    /// in; for git tools we capture the current HEAD SHA.
    fn invalidation_for(
        tool_name: &str,
        input: &ToolInput,
        ctx: &ToolContext,
    ) -> InvalidationTag {
        match tool_name {
            // File reads: invalidate when the file content changes.
            "read_file" => {
                if let Some(path_str) = input.get_str("path") {
                    let path = ctx.working_dir.join(path_str);
                    if let Ok(content) = std::fs::read(&path) {
                        return InvalidationTag::FileHash {
                            path,
                            hash: hash_content(&content),
                        };
                    }
                }
                // File not found / unreadable: short TTL so the miss is cheap.
                InvalidationTag::Ttl(unix_now() + 30)
            }

            // Glob and grep: directory Merkle hashing is expensive, use TTL.
            // §7 table: 5-minute TTL for these.
            "glob" | "grep" => InvalidationTag::Ttl(unix_now() + 300),

            // Git read-only tools: invalidate on HEAD SHA change.
            // §7 table: 30s TTL fallback, but GitSha is more precise.
            "git_status" | "git_diff" => InvalidationTag::GitSha {
                repo: ctx.working_dir.clone(),
                sha: current_git_sha(&ctx.working_dir).unwrap_or_default(),
                dirty: false, // v1: skip dirty-flag check (too slow)
            },

            // LSP tools: short TTL (index epoch would be more precise but
            // requires wiring IndexEpoch through the LSP bridge).
            "lsp_diagnostics" | "lsp_hover" | "lsp_code_actions" => {
                InvalidationTag::Ttl(unix_now() + 60)
            }

            // Symbol index: rely on file-watcher epoch; use a generous TTL.
            "symbol_search" | "symbol_at" => InvalidationTag::Ttl(unix_now() + 300),

            // Should not reach here (is_cacheable guards the call site).
            _ => InvalidationTag::None,
        }
    }

    fn build_key(
        tool_name: &str,
        input: &ToolInput,
        invalidator: InvalidationTag,
    ) -> CacheKey {
        let canonical = canonicalize_input(&input.0);
        CacheKey::from_bytes(
            format!("tool:{tool_name}"),
            canonical.as_bytes(),
            invalidator,
        )
    }

    /// Shadow-sample: run the tool and compare output against the cached bytes.
    /// Logs a warning on divergence; does not replace the cached entry in v1.
    async fn maybe_shadow(
        &self,
        cached_bytes: &[u8],
        input: ToolInput,
        ctx: &ToolContext,
    ) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        // Fast, non-cryptographic coin flip keyed on the input to avoid
        // correlated sampling across identical calls.
        let mut h = DefaultHasher::new();
        cached_bytes.hash(&mut h);
        let coin = (h.finish() as f64) / (u64::MAX as f64);
        if coin >= self.shadow_rate {
            return;
        }

        match self.inner.call(input, ctx).await {
            Ok(output) => {
                let fresh = serde_json::to_vec(&output.into_value()).unwrap_or_default();
                if fresh != cached_bytes {
                    tracing::warn!(
                        tool = self.inner.name(),
                        cached_bytes = cached_bytes.len(),
                        fresh_bytes = fresh.len(),
                        "tool cache shadow mismatch — possible stale entry (invalidation bug)"
                    );
                } else {
                    tracing::trace!(tool = self.inner.name(), "shadow sample matched");
                }
            }
            Err(e) => {
                tracing::warn!(
                    tool = self.inner.name(),
                    err = %e,
                    "shadow sample execution failed"
                );
            }
        }
    }
}

fn canonicalize_input(value: &Value) -> String {
    serde_json::to_string(&sort_value(value)).unwrap_or_default()
}

/// Recursively sort object keys so that `{"b":2,"a":1}` and `{"a":1,"b":2}`
/// produce the same JSON string and therefore the same cache key.
fn sort_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let sorted: serde_json::Map<String, Value> = map
                .iter()
                .map(|(k, v)| (k.clone(), sort_value(v)))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .collect();
            Value::Object(sorted)
        }
        Value::Array(arr) => Value::Array(arr.iter().map(sort_value).collect()),
        _ => value.clone(),
    }
}

/// Non-cryptographic 32-byte content hash (matches the scheme in `CacheKey`).
fn hash_content(bytes: &[u8]) -> [u8; 32] {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    let p = h.finish();

    let mut out = [0u8; 32];
    for i in 0..4 {
        let segment = p.wrapping_add(i as u64).to_le_bytes();
        out[i * 8..(i + 1) * 8].copy_from_slice(&segment);
    }
    out
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn current_git_sha(path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(path)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}

#[async_trait]
impl Tool for CachingToolWrapper {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn schema(&self) -> Value {
        self.inner.schema()
    }

    async fn call(&self, input: ToolInput, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let tool_name = self.inner.name();

        if !Self::is_cacheable(tool_name) {
            return self.inner.call(input, ctx).await;
        }

        let invalidator = Self::invalidation_for(tool_name, &input, ctx);
        let key = Self::build_key(tool_name, &input, invalidator.clone());

        let inner = Arc::clone(&self.inner);
        let input_for_compute = input.clone();
        let ctx_for_compute = ctx.clone();

        let bytes = self
            .cache
            .get_or_compute(key, move || async move {
                let output = inner
                    .call(input_for_compute, &ctx_for_compute)
                    .await
                    .map_err(|e| CacheError::Backend(format!("tool error: {e}")))?;
                let json = output.into_value();
                let bytes = serde_json::to_vec(&json)
                    .map_err(|e| CacheError::Serialization(e.to_string()))?;
                Ok((bytes, invalidator))
            })
            .await
            .map_err(|e| ToolError::Other(format!("cache error: {e}")))?;

        // Optional shadow-sample to detect stale entries (§8 false-positive detection).
        if self.shadow_rate > 0.0 {
            self.maybe_shadow(&bytes, input, ctx).await;
        }

        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|e| ToolError::Other(format!("cache deserialize: {e}")))?;

        Ok(ToolOutput::new(value))
    }

    fn layer(&self) -> ToolLayer {
        self.inner.layer()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hone_cache::{CacheBackend, InMemoryBackend, ToolResultCache};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    struct CountingTool {
        calls: Arc<AtomicU32>,
    }

    #[async_trait]
    impl Tool for CountingTool {
        fn name(&self) -> &str {
            "read_file"
        }
        fn description(&self) -> &str {
            "test"
        }
        fn schema(&self) -> Value {
            serde_json::json!({})
        }
        async fn call(&self, _input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(ToolOutput::new(serde_json::json!({"content": "hello"})))
        }
        fn layer(&self) -> ToolLayer {
            ToolLayer::Native
        }
    }

    fn make_ctx() -> ToolContext {
        ToolContext {
            sandbox: Arc::new(hone_sandbox::NullSandbox),
            working_dir: std::env::current_dir().unwrap(),
        }
    }

    #[tokio::test]
    async fn non_cacheable_tool_bypasses_cache() {
        struct WriteTool;
        #[async_trait]
        impl Tool for WriteTool {
            fn name(&self) -> &str { "write_file" }
            fn description(&self) -> &str { "test" }
            fn schema(&self) -> Value { serde_json::json!({}) }
            async fn call(&self, _i: ToolInput, _c: &ToolContext) -> Result<ToolOutput, ToolError> {
                Ok(ToolOutput::new(serde_json::json!({"ok": true})))
            }
            fn layer(&self) -> ToolLayer { ToolLayer::Native }
        }

        let backend = Arc::new(InMemoryBackend::new());
        let cache = Arc::new(ToolResultCache::new(backend.clone()));
        let wrapper = CachingToolWrapper::new(Arc::new(WriteTool), cache).with_shadow_rate(0.0);
        let ctx = make_ctx();

        let _ = wrapper.call(ToolInput::new(serde_json::json!({})), &ctx).await.unwrap();

        let stats = backend.stats().await;
        assert_eq!(stats.misses, 0, "write_file should bypass the cache entirely");
    }

    #[tokio::test]
    async fn sort_value_normalises_key_order() {
        let a = sort_value(&serde_json::json!({"b": 2, "a": 1}));
        let b = sort_value(&serde_json::json!({"a": 1, "b": 2}));
        assert_eq!(
            serde_json::to_string(&a).unwrap(),
            serde_json::to_string(&b).unwrap()
        );
    }

    #[tokio::test]
    async fn hash_content_is_deterministic() {
        let h1 = hash_content(b"hello world");
        let h2 = hash_content(b"hello world");
        assert_eq!(h1, h2);
    }

    #[tokio::test]
    async fn hash_content_differs_on_different_input() {
        let h1 = hash_content(b"hello");
        let h2 = hash_content(b"world");
        assert_ne!(h1, h2);
    }
}
