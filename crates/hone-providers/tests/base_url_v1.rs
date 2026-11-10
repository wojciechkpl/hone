#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
//! B1-B3 (`docs/plans/base-url-v1-spec.md`): `with_base_url` accepts a base that already ends in
//! `/v1` (the OpenAI SDK / OpenRouter / vLLM convention) and a trailing slash, without ever
//! producing `/v1/v1/...`. Only a whole `/v1` path segment is removed.
//!
//! A wiremock server accepts every POST; the tests assert on the path the request arrived at.

use std::time::Duration;

use futures::StreamExt;
use hone_proto::Message;
use hone_providers::{AnthropicProvider, CompletionRequest, ModelProvider, OpenAiProvider};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: [DONE]\n\n"),
        )
        .mount(&s)
        .await;
    s
}

async fn paths(s: &MockServer) -> Vec<String> {
    s.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.url.path().to_owned())
        .collect()
}

fn request(model: &str) -> CompletionRequest {
    CompletionRequest::new(model, vec![Message::user("hi")])
}

async fn drain(p: &dyn ModelProvider, model: &str) {
    let stream = p.complete(request(model)).await.expect("request accepted");
    let _: Vec<_> = tokio::time::timeout(Duration::from_secs(20), stream.collect())
        .await
        .expect("stream did not finish");
}

// -- B1: OpenAI ------------------------------------------------------------------------------------

const OPENAI_CASES: [(&str, &str); 7] = [
    ("", "/v1/chat/completions"),
    ("/", "/v1/chat/completions"),
    ("//", "/v1/chat/completions"),
    ("/v1", "/v1/chat/completions"),
    ("/v1/", "/v1/chat/completions"),
    ("/api/v1", "/api/v1/chat/completions"),
    ("/api", "/api/v1/chat/completions"),
];

#[tokio::test]
async fn test_b1_openai_complete_never_doubles_v1() {
    for (suffix, expected) in OPENAI_CASES {
        let s = server().await;
        let p = OpenAiProvider::with_base_url("fixture-key", format!("{}{suffix}", s.uri()));

        drain(&p, "openai:gpt-4o").await;

        assert_eq!(paths(&s).await, vec![expected.to_owned()], "base suffix {suffix:?}");
    }
}

#[tokio::test]
async fn test_b1_openai_health_never_doubles_v1() {
    for (suffix, expected) in OPENAI_CASES {
        let s = server().await;
        let p = OpenAiProvider::with_base_url("fixture-key", format!("{}{suffix}", s.uri()));

        let _ = tokio::time::timeout(Duration::from_secs(20), p.health())
            .await
            .expect("health hung");

        assert_eq!(paths(&s).await, vec![expected.to_owned()], "base suffix {suffix:?}");
    }
}

// -- B2: only a whole segment is removed ------------------------------------------------------------

#[tokio::test]
async fn test_b2_only_a_whole_v1_segment_is_removed() {
    for (suffix, expected) in [
        ("/apiv1", "/apiv1/v1/chat/completions"),
        ("/v1beta", "/v1beta/v1/chat/completions"),
        ("/foo/v10", "/foo/v10/v1/chat/completions"),
    ] {
        let s = server().await;
        let p = OpenAiProvider::with_base_url("fixture-key", format!("{}{suffix}", s.uri()));

        drain(&p, "openai:gpt-4o").await;

        assert_eq!(paths(&s).await, vec![expected.to_owned()], "base suffix {suffix:?}");
    }
}

// -- B3: Anthropic -------------------------------------------------------------------------------------

#[tokio::test]
async fn test_b3_anthropic_complete_never_doubles_v1() {
    for (suffix, expected) in [
        ("", "/v1/messages"),
        ("/", "/v1/messages"),
        ("/v1", "/v1/messages"),
        ("/v1/", "/v1/messages"),
        ("/proxy/v1", "/proxy/v1/messages"),
        ("/apiv1", "/apiv1/v1/messages"),
    ] {
        let s = server().await;
        let p = AnthropicProvider::with_base_url("fixture-key", format!("{}{suffix}", s.uri()));

        drain(&p, "anthropic:claude-sonnet-4-5").await;

        assert_eq!(paths(&s).await, vec![expected.to_owned()], "base suffix {suffix:?}");
    }
}

#[tokio::test]
async fn test_b3_anthropic_health_never_doubles_v1() {
    for (suffix, expected) in [("/v1", "/v1/messages"), ("", "/v1/messages")] {
        let s = server().await;
        let p = AnthropicProvider::with_base_url("fixture-key", format!("{}{suffix}", s.uri()));

        let _ = tokio::time::timeout(Duration::from_secs(20), p.health())
            .await
            .expect("health hung");

        assert_eq!(paths(&s).await, vec![expected.to_owned()], "base suffix {suffix:?}");
    }
}

// -- B5: surrounding whitespace is trimmed before normalising -------------------------------------------

/// `(text before the server URI, text after it, expected normalised path prefix)`.
const WHITESPACE_CASES: [(&str, &str, &str); 7] = [
    ("", " ", ""),
    ("", "/v1 ", ""),
    ("  ", "/v1", ""),
    ("", "/v1/\t", ""),
    ("", "/v1/\n", ""),
    (" \t", "/api/v1/ \n", "/api"),
    ("", "/ ", ""),
];

#[tokio::test]
async fn test_b5_openai_trims_surrounding_whitespace() {
    for (before, after, prefix) in WHITESPACE_CASES {
        let s = server().await;
        let p = OpenAiProvider::with_base_url("fixture-key", format!("{before}{}{after}", s.uri()));

        drain(&p, "openai:gpt-4o").await;
        assert_eq!(
            paths(&s).await,
            vec![format!("{prefix}/v1/chat/completions")],
            "complete, {before:?} + uri + {after:?}"
        );

        let s = server().await;
        let p = OpenAiProvider::with_base_url("fixture-key", format!("{before}{}{after}", s.uri()));
        let _ = tokio::time::timeout(Duration::from_secs(20), p.health())
            .await
            .expect("health hung");
        assert_eq!(
            paths(&s).await,
            vec![format!("{prefix}/v1/chat/completions")],
            "health, {before:?} + uri + {after:?}"
        );
    }
}

#[tokio::test]
async fn test_b5_anthropic_trims_surrounding_whitespace() {
    for (before, after, prefix) in WHITESPACE_CASES {
        let s = server().await;
        let p = AnthropicProvider::with_base_url("fixture-key", format!("{before}{}{after}", s.uri()));

        drain(&p, "anthropic:claude-sonnet-4-5").await;
        assert_eq!(
            paths(&s).await,
            vec![format!("{prefix}/v1/messages")],
            "complete, {before:?} + uri + {after:?}"
        );

        let s = server().await;
        let p = AnthropicProvider::with_base_url("fixture-key", format!("{before}{}{after}", s.uri()));
        let _ = tokio::time::timeout(Duration::from_secs(20), p.health())
            .await
            .expect("health hung");
        assert_eq!(
            paths(&s).await,
            vec![format!("{prefix}/v1/messages")],
            "health, {before:?} + uri + {after:?}"
        );
    }
}
