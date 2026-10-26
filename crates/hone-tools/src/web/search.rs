use async_trait::async_trait;
use hone_proto::ToolLayer;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub enum SearchBackend {
    Disabled,
    Tavily {
        api_key: String,
        http: reqwest::Client,
    },
    Brave {
        api_key: String,
        http: reqwest::Client,
    },
    Exa {
        api_key: String,
        http: reqwest::Client,
    },
    #[cfg(any(test, feature = "test-stub"))]
    Stub(Vec<SearchResult>),
}

pub struct WebSearchTool {
    backend: SearchBackend,
}

impl WebSearchTool {
    pub fn from_env() -> Self {
        let provider = std::env::var("HONE_WEBSEARCH_PROVIDER")
            .unwrap_or_default()
            .to_lowercase();
        let key = std::env::var("HONE_WEBSEARCH_KEY")
            .unwrap_or_default();
        let key = key.trim().to_string();

        match provider.as_str() {
            "tavily" | "brave" | "exa" if !key.is_empty() => {
                let http = reqwest::Client::new();
                let backend = match provider.as_str() {
                    "tavily" => SearchBackend::Tavily { api_key: key, http },
                    "brave" => SearchBackend::Brave { api_key: key, http },
                    "exa" => SearchBackend::Exa { api_key: key, http },
                    _ => unreachable!(),
                };
                Self { backend }
            }
            _ => Self {
                backend: SearchBackend::Disabled,
            },
        }
    }

    pub fn disabled() -> Self {
        Self {
            backend: SearchBackend::Disabled,
        }
    }

    #[cfg(any(test, feature = "test-stub"))]
    pub fn stub(results: Vec<SearchResult>) -> Self {
        Self {
            backend: SearchBackend::Stub(results),
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for a query and return ranked results."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of results to return. Default: 5.",
                    "minimum": 1
                }
            },
            "required": ["query"]
        })
    }

    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let query = input
            .get_str("query")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'query'".to_string(),
            })?;
        let _max_results = input.get_u64("max_results").unwrap_or(5) as usize;
        #[cfg(any(test, feature = "test-stub"))]
        let max_results = _max_results;

        match &self.backend {
            SearchBackend::Disabled => Err(ToolError::Other(
                "Web search is disabled. Set HONE_WEBSEARCH_PROVIDER (tavily|brave|exa) \
                 and HONE_WEBSEARCH_KEY to enable it."
                    .to_string(),
            )),

            #[cfg(any(test, feature = "test-stub"))]
            SearchBackend::Stub(results) => {
                let truncated: Vec<&SearchResult> = results.iter().take(max_results).collect();
                Ok(ToolOutput::new(json!(truncated)))
            }

            SearchBackend::Tavily { .. } | SearchBackend::Brave { .. } | SearchBackend::Exa { .. } => {
                let _ = query;
                Err(ToolError::Other(
                    "not yet implemented for live providers".to_string(),
                ))
            }
        }
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }

    fn is_read_only(&self) -> bool {
        true
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex, PoisonError};

    use super::*;

    /// Tests run in parallel threads of one process, so every test that sets
    /// or removes `HONE_WEBSEARCH_*` must hold this lock for its whole body.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    fn make_ctx() -> ToolContext {
        ToolContext {
            sandbox: Arc::new(hone_sandbox::NullSandbox),
            sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
            working_dir: PathBuf::from("/tmp"),
        }
    }

    // -----------------------------------------------------------------------
    // from_env tests
    // -----------------------------------------------------------------------

    #[test]
    fn from_env_with_no_provider_is_disabled() {
        let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        // Given: HONE_WEBSEARCH_PROVIDER is unset
        // SAFETY: ENV_MUTEX is held, so no other test touches the env
        unsafe {
            std::env::remove_var("HONE_WEBSEARCH_PROVIDER");
            std::env::remove_var("HONE_WEBSEARCH_KEY");
        }

        // When
        let tool = WebSearchTool::from_env();

        // Then: backend must be Disabled
        assert!(
            matches!(tool.backend, SearchBackend::Disabled),
            "expected Disabled backend when HONE_WEBSEARCH_PROVIDER is absent"
        );
    }

    #[test]
    fn from_env_unknown_provider_is_disabled() {
        let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        // Given
        // SAFETY: ENV_MUTEX is held, so no other test touches the env
        unsafe {
            std::env::set_var("HONE_WEBSEARCH_PROVIDER", "garbage");
            std::env::set_var("HONE_WEBSEARCH_KEY", "somekey");
        }

        // When
        let tool = WebSearchTool::from_env();

        // Then
        assert!(
            matches!(tool.backend, SearchBackend::Disabled),
            "expected Disabled backend for unknown provider"
        );

        // SAFETY: cleanup
        unsafe {
            std::env::remove_var("HONE_WEBSEARCH_PROVIDER");
            std::env::remove_var("HONE_WEBSEARCH_KEY");
        }
    }

    #[test]
    fn from_env_tavily_no_key_is_disabled() {
        let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        // Given: provider set but key absent
        // SAFETY: ENV_MUTEX is held, so no other test touches the env
        unsafe {
            std::env::set_var("HONE_WEBSEARCH_PROVIDER", "tavily");
            std::env::remove_var("HONE_WEBSEARCH_KEY");
        }

        // When
        let tool = WebSearchTool::from_env();

        // Then
        assert!(
            matches!(tool.backend, SearchBackend::Disabled),
            "expected Disabled backend when api key is absent"
        );

        // SAFETY: cleanup
        unsafe {
            std::env::remove_var("HONE_WEBSEARCH_PROVIDER");
        }
    }

    #[test]
    fn from_env_tavily_with_key_constructs_tavily_backend() {
        let _env = ENV_MUTEX.lock().unwrap_or_else(PoisonError::into_inner);
        // Given
        // SAFETY: ENV_MUTEX is held, so no other test touches the env
        unsafe {
            std::env::set_var("HONE_WEBSEARCH_PROVIDER", "tavily");
            std::env::set_var("HONE_WEBSEARCH_KEY", "tvly-testkey123");
        }

        // When
        let tool = WebSearchTool::from_env();

        // Then
        assert!(
            matches!(tool.backend, SearchBackend::Tavily { .. }),
            "expected Tavily backend when provider=tavily and key is present"
        );

        // SAFETY: cleanup
        unsafe {
            std::env::remove_var("HONE_WEBSEARCH_PROVIDER");
            std::env::remove_var("HONE_WEBSEARCH_KEY");
        }
    }

    // -----------------------------------------------------------------------
    // disabled() tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn disabled_call_returns_error_explaining_setup() {
        // Given
        let tool = WebSearchTool::disabled();
        let input = ToolInput::new(json!({"query": "rust"}));
        let ctx = make_ctx();

        // When
        let result = tool.call(input, &ctx).await;

        // Then: must be an error whose message mentions both env vars
        let err = result.expect_err("disabled tool must return Err");
        let msg = err.to_string();
        assert!(
            msg.contains("HONE_WEBSEARCH_PROVIDER"),
            "error should mention HONE_WEBSEARCH_PROVIDER, got: {msg}"
        );
        assert!(
            msg.contains("HONE_WEBSEARCH_KEY"),
            "error should mention HONE_WEBSEARCH_KEY, got: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // stub() tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn stub_call_returns_canned_results() {
        // Given
        let results = vec![SearchResult {
            title: "Rust lang".to_string(),
            url: "https://www.rust-lang.org".to_string(),
            snippet: "A language empowering everyone.".to_string(),
        }];
        let tool = WebSearchTool::stub(results.clone());
        let input = ToolInput::new(json!({"query": "rust"}));
        let ctx = make_ctx();

        // When
        let output = tool.call(input, &ctx).await.expect("stub must succeed");
        let arr = output.value.as_array().expect("output must be a JSON array");

        // Then
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["title"], "Rust lang");
        assert_eq!(arr[0]["url"], "https://www.rust-lang.org");
        assert_eq!(arr[0]["snippet"], "A language empowering everyone.");
    }

    #[tokio::test]
    async fn stub_call_respects_max_results_truncation() {
        // Given: 10 stubbed results, max_results = 3
        let results: Vec<SearchResult> = (0..10)
            .map(|i| SearchResult {
                title: format!("Result {i}"),
                url: format!("https://example.com/{i}"),
                snippet: format!("Snippet {i}"),
            })
            .collect();
        let tool = WebSearchTool::stub(results);
        let input = ToolInput::new(json!({"query": "test", "max_results": 3}));
        let ctx = make_ctx();

        // When
        let output = tool.call(input, &ctx).await.expect("stub must succeed");
        let arr = output.value.as_array().expect("output must be a JSON array");

        // Then
        assert_eq!(arr.len(), 3, "should be truncated to max_results=3");
    }

    #[tokio::test]
    async fn stub_call_default_max_results_is_5_when_omitted() {
        // Given: 10 stubbed results, no max_results specified
        let results: Vec<SearchResult> = (0..10)
            .map(|i| SearchResult {
                title: format!("Result {i}"),
                url: format!("https://example.com/{i}"),
                snippet: format!("Snippet {i}"),
            })
            .collect();
        let tool = WebSearchTool::stub(results);
        let input = ToolInput::new(json!({"query": "test"}));
        let ctx = make_ctx();

        // When
        let output = tool.call(input, &ctx).await.expect("stub must succeed");
        let arr = output.value.as_array().expect("output must be a JSON array");

        // Then
        assert_eq!(arr.len(), 5, "default max_results should be 5");
    }

    #[test]
    fn stub_is_read_only_returns_true() {
        let tool = WebSearchTool::stub(vec![]);
        assert!(tool.is_read_only());
    }

    #[test]
    fn stub_tool_name_is_web_search() {
        let tool = WebSearchTool::stub(vec![]);
        assert_eq!(tool.name(), "web_search");
    }

    #[tokio::test]
    async fn stub_call_with_invalid_args_returns_tool_error() {
        // Given: input missing 'query'
        let tool = WebSearchTool::stub(vec![]);
        let input = ToolInput::new(json!({"max_results": 5}));
        let ctx = make_ctx();

        // When
        let result = tool.call(input, &ctx).await;

        // Then
        assert!(
            result.is_err(),
            "missing 'query' field must return an error"
        );
    }
}
