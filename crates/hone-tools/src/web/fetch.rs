use std::sync::OnceLock;

use async_trait::async_trait;
use hone_proto::ToolLayer;
use regex::Regex;
use serde_json::{Value, json};

use crate::{
    error::ToolError,
    traits::{Tool, ToolContext, ToolInput, ToolOutput},
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FetchResult {
    pub url: String,
    pub title: Option<String>,
    pub content_markdown: String,
    pub status: u16,
}

#[cfg(any(test, feature = "test-stub"))]
#[derive(Debug, Clone)]
pub struct CannedFetch {
    pub status: u16,
    pub html: String,
    pub url: String,
}

pub struct WebFetchTool {
    http: reqwest::Client,
    #[cfg(any(test, feature = "test-stub"))]
    canned: Option<CannedFetch>,
}

// Compiled regexes — each initialised once on first use.

fn re_title() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap_or_else(|_| unreachable!()))
}

fn re_noise() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?is)<(script|style|nav|footer|aside|header)[^>]*>.*?</(script|style|nav|footer|aside|header)>",
        )
        .unwrap_or_else(|_| unreachable!())
    })
}

fn re_link() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?is)<a\s[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
            .unwrap_or_else(|_| unreachable!())
    })
}

fn re_code() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?is)<code[^>]*>(.*?)</code>").unwrap_or_else(|_| unreachable!())
    })
}

fn re_heading() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?is)<h([1-6])[^>]*>(.*?)</h[1-6]>").unwrap_or_else(|_| unreachable!())
    })
}

fn re_paragraph() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?is)<p[^>]*>(.*?)</p>").unwrap_or_else(|_| unreachable!())
    })
}

fn re_list_item() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?is)<li[^>]*>(.*?)</li>").unwrap_or_else(|_| unreachable!())
    })
}

fn re_any_tag() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?s)<[^>]+>").unwrap_or_else(|_| unreachable!()))
}

fn re_excess_blanks() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\n{3,}").unwrap_or_else(|_| unreachable!()))
}

impl WebFetchTool {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(concat!("hone/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            http,
            #[cfg(any(test, feature = "test-stub"))]
            canned: None,
        }
    }

    #[cfg(any(test, feature = "test-stub"))]
    pub fn with_canned(canned: CannedFetch) -> Self {
        Self {
            http: reqwest::Client::new(),
            canned: Some(canned),
        }
    }

    /// Convert HTML to Markdown using a minimal readability-style extractor.
    ///
    /// Returns `(title, markdown_body)`:
    /// - `title` is the contents of the first `<title>` tag, or `None` if absent.
    /// - body strips `<script>`, `<style>`, `<nav>`, `<footer>`, `<aside>`, and
    ///   `<header>`; converts headings, paragraphs, links, list items, and inline
    ///   code to their Markdown equivalents; collapses remaining whitespace.
    pub fn html_to_markdown(html: &str) -> (Option<String>, String) {
        // Extract title
        let title = re_title()
            .captures(html)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().trim().to_string())
            .filter(|s| !s.is_empty());

        // Strip block-level noise tags (script, style, nav, footer, aside, header)
        let cleaned = re_noise().replace_all(html, "");

        // Convert <a href="...">TEXT</a> first (before stripping tags)
        let cleaned = re_link().replace_all(&cleaned, |caps: &regex::Captures| {
            let href = caps.get(1).map_or("", |m| m.as_str());
            let text = caps.get(2).map_or("", |m| m.as_str());
            let text_clean = re_any_tag().replace_all(text, "");
            format!("[{text_clean}]({href})")
        });

        // Convert <code>X</code> to `X`
        let cleaned = re_code().replace_all(&cleaned, |caps: &regex::Captures| {
            let inner = caps.get(1).map_or("", |m| m.as_str());
            format!("`{inner}`")
        });

        // Convert headings h1..h6
        let cleaned = re_heading().replace_all(&cleaned, |caps: &regex::Captures| {
            let level: usize = caps[1].parse().unwrap_or(1);
            let hashes = "#".repeat(level);
            let inner = caps.get(2).map_or("", |m| m.as_str());
            let text = re_any_tag().replace_all(inner, "");
            let text = text.trim();
            format!("{hashes} {text}\n\n")
        });

        // Convert <p>...</p>
        let cleaned = re_paragraph().replace_all(&cleaned, |caps: &regex::Captures| {
            let inner = caps.get(1).map_or("", |m| m.as_str());
            let text = re_any_tag().replace_all(inner, "");
            let text = text.trim().to_string();
            if text.is_empty() {
                String::new()
            } else {
                format!("{text}\n\n")
            }
        });

        // Convert <li>...</li>
        let cleaned = re_list_item().replace_all(&cleaned, |caps: &regex::Captures| {
            let inner = caps.get(1).map_or("", |m| m.as_str());
            let text = re_any_tag().replace_all(inner, "");
            let text = text.trim().to_string();
            if text.is_empty() {
                String::new()
            } else {
                format!("- {text}\n")
            }
        });

        // Strip all remaining HTML tags
        let cleaned = re_any_tag().replace_all(&cleaned, "");

        // Decode common HTML entities
        let cleaned = cleaned
            .replace("&amp;", "&")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&#39;", "'")
            .replace("&nbsp;", " ");

        // Collapse 3+ consecutive blank lines to 2
        let cleaned = re_excess_blanks().replace_all(&cleaned, "\n\n");

        let body = cleaned.trim().to_string();

        (title, body)
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch a URL and return the page content as Markdown."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch."
                }
            },
            "required": ["url"]
        })
    }

    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let url = input
            .get_str("url")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing required field 'url'".to_string(),
            })?
            .to_string();

        #[cfg(any(test, feature = "test-stub"))]
        if let Some(ref canned) = self.canned {
            let (title, content_markdown) = Self::html_to_markdown(&canned.html);
            let result = FetchResult {
                url: canned.url.clone(),
                title,
                content_markdown,
                status: canned.status,
            };
            return Ok(ToolOutput::new(json!(result)));
        }

        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| ToolError::Other(e.to_string()))?;

        let status = response.status().as_u16();
        let html = response
            .text()
            .await
            .map_err(|e| ToolError::Other(e.to_string()))?;

        let (title, content_markdown) = Self::html_to_markdown(&html);
        let result = FetchResult {
            url,
            title,
            content_markdown,
            status,
        };
        Ok(ToolOutput::new(json!(result)))
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
    use std::sync::Arc;

    use super::*;

    fn make_ctx() -> ToolContext {
        ToolContext {
            sandbox: Arc::new(hone_sandbox::NullSandbox),
            sandbox_policy: Arc::new(hone_sandbox::SandboxPolicy::permissive()),
            working_dir: PathBuf::from("/tmp"),
        }
    }

    fn canned_call_output(html: &str) -> Value {
        // Synchronous wrapper: uses the canned seam so no network is hit.
        // We drive this via tokio in each individual test.
        panic!("use #[tokio::test] and call canned_call_output_async instead")
    }

    async fn canned_call_output_async(html: &str) -> Value {
        let tool = WebFetchTool::with_canned(CannedFetch {
            status: 200,
            html: html.to_string(),
            url: "https://example.com/test".to_string(),
        });
        let input = ToolInput::new(json!({"url": "https://example.com/test"}));
        let ctx = make_ctx();
        tool.call(input, &ctx)
            .await
            .expect("canned fetch must succeed")
            .value
    }

    // -----------------------------------------------------------------------
    // Status passthrough
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn with_canned_returns_canned_status() {
        // Given
        let tool = WebFetchTool::with_canned(CannedFetch {
            status: 200,
            html: "<html><body><p>Hello</p></body></html>".to_string(),
            url: "https://example.com".to_string(),
        });
        let input = ToolInput::new(json!({"url": "https://example.com"}));
        let ctx = make_ctx();

        // When
        let output = tool.call(input, &ctx).await.expect("must succeed");

        // Then
        assert_eq!(output.value["status"], 200);
    }

    // -----------------------------------------------------------------------
    // Title extraction
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn with_canned_html_extracts_title() {
        // Given
        let html = "<html><head><title>Hi</title></head><body><p>Hello</p></body></html>";

        // When
        let out = canned_call_output_async(html).await;

        // Then
        assert_eq!(
            out["title"],
            Value::String("Hi".to_string()),
            "title must be extracted from <title> tag"
        );
    }

    // -----------------------------------------------------------------------
    // Body extraction
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn with_canned_html_extracts_text_into_markdown() {
        // Given
        let html = "<html><body><h1>Foo</h1><p>Bar.</p></body></html>";

        // When
        let out = canned_call_output_async(html).await;
        let md = out["content_markdown"]
            .as_str()
            .expect("content_markdown must be a string");

        // Then
        assert!(
            md.contains("# Foo"),
            "h1 should become '# Foo' in markdown, got: {md}"
        );
        assert!(
            md.contains("Bar."),
            "paragraph text should appear in markdown, got: {md}"
        );
    }

    #[tokio::test]
    async fn with_canned_html_strips_script_and_style() {
        // Given
        let html =
            "<html><body><script>alert(1)</script><p>Visible</p><style>x{color:red}</style></body></html>";

        // When
        let out = canned_call_output_async(html).await;
        let md = out["content_markdown"]
            .as_str()
            .expect("content_markdown must be a string");

        // Then
        assert!(
            md.contains("Visible"),
            "visible text should be in markdown, got: {md}"
        );
        assert!(
            !md.contains("alert(1)"),
            "script content must be stripped, got: {md}"
        );
        assert!(
            !md.contains("color:red"),
            "style content must be stripped, got: {md}"
        );
    }

    #[tokio::test]
    async fn with_canned_html_links_become_markdown_links() {
        // Given
        let html =
            r#"<html><body><p><a href="https://example.com">link</a></p></body></html>"#;

        // When
        let out = canned_call_output_async(html).await;
        let md = out["content_markdown"]
            .as_str()
            .expect("content_markdown must be a string");

        // Then
        assert!(
            md.contains("[link](https://example.com)"),
            "anchor should become markdown link, got: {md}"
        );
    }

    #[tokio::test]
    async fn with_canned_html_lists_become_markdown() {
        // Given
        let html = "<html><body><ul><li>a</li><li>b</li></ul></body></html>";

        // When
        let out = canned_call_output_async(html).await;
        let md = out["content_markdown"]
            .as_str()
            .expect("content_markdown must be a string");

        // Then
        assert!(
            md.contains("- a"),
            "list item should become '- a', got: {md}"
        );
        assert!(
            md.contains("- b"),
            "list item should become '- b', got: {md}"
        );
    }

    // -----------------------------------------------------------------------
    // html_to_markdown unit tests (tests the public method directly)
    // -----------------------------------------------------------------------

    #[test]
    fn html_to_markdown_no_title_returns_none() {
        // Given: HTML without a <title> tag
        let html = "<html><body><p>No title here</p></body></html>";

        // When
        let (title, _body) = WebFetchTool::html_to_markdown(html);

        // Then
        assert!(
            title.is_none(),
            "expected None for title when <title> tag is absent"
        );
    }

    // -----------------------------------------------------------------------
    // Tool trait surface
    // -----------------------------------------------------------------------

    #[test]
    fn tool_name_is_web_fetch() {
        let tool = WebFetchTool::new();
        assert_eq!(tool.name(), "web_fetch");
    }

    #[test]
    fn is_read_only_returns_true() {
        let tool = WebFetchTool::new();
        assert!(tool.is_read_only());
    }

    #[tokio::test]
    async fn call_with_invalid_args_returns_tool_error() {
        // Given: input missing 'url'
        let tool = WebFetchTool::new();
        let input = ToolInput::new(json!({}));
        let ctx = make_ctx();

        // When
        let result = tool.call(input, &ctx).await;

        // Then
        assert!(result.is_err(), "missing 'url' must return an error");
    }
}
