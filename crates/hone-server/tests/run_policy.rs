//! P2.3 spec section 4 and 5: `POST /run` loads the permission policy for the
//! run's working directory, honours `approve`, and refuses providers that run
//! their own tools under a non-empty policy.
//!
//! The model is a scripted provider injected with `AppState::with_mock_provider`;
//! tools are the real `/run` tool set, so "not dispatched" is observed through
//! the filesystem (the file the tool would have written).
//!
//! Home-level policy (`~/.config/hone/permissions.toml`) of the machine running
//! the tests is outside the harness' control; these tests only use the
//! project layer.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use futures::{Stream, stream};
use hone_proto::{Message, ModelId, ModelInfo, ProviderEvent, ProviderHealth, ToolCallRequest};
use hone_providers::{CompletionRequest, ModelProvider, ProviderError};
use hone_server::{AppState, router};
use serde_json::json;
use tower::ServiceExt;

// -- scripted provider -------------------------------------------------------------

struct Scripted {
    first: Vec<ProviderEvent>,
    n: Mutex<usize>,
    own_tools: bool,
}

impl Scripted {
    fn writes(path: &str, own_tools: bool) -> Arc<Self> {
        Arc::new(Self {
            first: vec![
                ProviderEvent::ToolCall(ToolCallRequest {
                    id: "w1".to_owned(),
                    name: "write_file".to_owned(),
                    arguments: json!({ "path": path, "content": "data" }),
                }),
                ProviderEvent::Done,
            ],
            n: Mutex::new(0),
            own_tools,
        })
    }
}

#[async_trait]
impl ModelProvider for Scripted {
    fn id(&self) -> &str {
        "scripted"
    }
    fn display_name(&self) -> &str {
        "Scripted"
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
        let i = {
            let mut n = self.n.lock().unwrap();
            let i = *n;
            *n += 1;
            i
        };
        let events = if i == 0 {
            self.first.clone()
        } else {
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
        10
    }
    async fn health(&self) -> ProviderHealth {
        ProviderHealth::Healthy
    }
    // Compile-time dependency on the new trait method (red until added).
    fn runs_own_tools(&self) -> bool {
        self.own_tools
    }
}

// -- harness ----------------------------------------------------------------------

struct Env {
    _tmp: tempfile::TempDir,
    root: PathBuf,
}

fn env_with_policy(policy_toml: Option<&str>) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    if let Some(body) = policy_toml {
        std::fs::create_dir_all(root.join(".hone")).unwrap();
        std::fs::write(root.join(".hone/permissions.toml"), body).unwrap();
    }
    Env { _tmp: tmp, root }
}

const TOKEN: &str = "test-token";

/// Unauthenticated server (no `auth_token` configured).
async fn post_run(
    env: &Env,
    provider: Arc<Scripted>,
    extra: serde_json::Value,
) -> (StatusCode, String) {
    post_run_as(env, provider, extra, None).await
}

/// Server with `auth_token` configured; the request carries the bearer token.
async fn post_run_authed(
    env: &Env,
    provider: Arc<Scripted>,
    extra: serde_json::Value,
) -> (StatusCode, String) {
    post_run_as(env, provider, extra, Some(TOKEN)).await
}

async fn post_run_as(
    env: &Env,
    provider: Arc<Scripted>,
    extra: serde_json::Value,
    token: Option<&str>,
) -> (StatusCode, String) {
    let state = AppState::new(token.map(str::to_owned))
        .with_provider_key(None)
        .with_workspace_root(env.root.clone())
        .with_mock_provider(provider);
    let mut body = json!({ "prompt": "write the file" });
    if let Some(obj) = extra.as_object() {
        for (k, v) in obj {
            body[k] = v.clone();
        }
    }
    let mut builder = Request::builder()
        .method("POST")
        .uri("/run")
        .header("content-type", "application/json");
    if let Some(t) = token {
        builder = builder.header("authorization", format!("Bearer {t}"));
    }
    let req = builder.body(Body::from(body.to_string())).unwrap();
    let resp = router(state).oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = tokio::time::timeout(
        std::time::Duration::from_secs(20),
        axum::body::to_bytes(resp.into_body(), usize::MAX),
    )
    .await
    .expect("response body did not finish")
    .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

const DENY_BLOCKED: &str = r#"
version = 2
[[rules]]
id = "no-blocked"
path = "blocked.txt"
op = "write"
decision = "deny"
"#;

const ASK_ASKED: &str = r#"
version = 2
[[rules]]
id = "ask-write"
path = "asked.txt"
op = "write"
decision = "ask"
"#;

// -- tests ------------------------------------------------------------------------

#[tokio::test]
async fn test_run_when_no_policy_files_should_dispatch_as_before() {
    let env = env_with_policy(None);
    let (status, body) = post_run(&env, Scripted::writes("plain.txt", false), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(env.root.join("plain.txt").exists(), "tool must run: {body}");
}

#[tokio::test]
async fn test_run_when_project_policy_denies_should_not_dispatch_tool() {
    let env = env_with_policy(Some(DENY_BLOCKED));
    let (status, body) = post_run(&env, Scripted::writes("blocked.txt", false), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!env.root.join("blocked.txt").exists(), "denied tool ran");
    assert!(body.contains("denied by policy"), "{body}");
    assert!(body.contains("no-blocked"), "{body}");
}

#[tokio::test]
async fn test_run_when_policy_present_but_call_not_matched_should_dispatch() {
    let env = env_with_policy(Some(DENY_BLOCKED));
    let (status, body) = post_run(&env, Scripted::writes("fine.txt", false), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(env.root.join("fine.txt").exists(), "{body}");
}

#[tokio::test]
async fn test_run_when_ask_rule_and_no_approve_should_fail_closed_headless() {
    let env = env_with_policy(Some(ASK_ASKED));
    let (status, body) = post_run(&env, Scripted::writes("asked.txt", false), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(
        !env.root.join("asked.txt").exists(),
        "ask must not run headless"
    );
    assert!(body.contains("requires approval"), "{body}");
    assert!(body.contains("ask-write"), "{body}");
}

#[tokio::test]
async fn test_run_when_approve_names_the_ask_rule_should_let_the_call_through() {
    let env = env_with_policy(Some(ASK_ASKED));
    let (status, body) = post_run_authed(
        &env,
        Scripted::writes("asked.txt", false),
        json!({ "approve": ["ask-write"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(env.root.join("asked.txt").exists(), "{body}");
}

#[tokio::test]
async fn test_run_when_approve_names_a_deny_rule_should_still_deny() {
    let env = env_with_policy(Some(DENY_BLOCKED));
    let (status, body) = post_run_authed(
        &env,
        Scripted::writes("blocked.txt", false),
        json!({ "approve": ["no-blocked"] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!env.root.join("blocked.txt").exists());
    assert!(body.contains("denied by policy"), "{body}");
}

#[tokio::test]
async fn test_run_when_approve_id_unknown_should_return_400() {
    let env = env_with_policy(Some(ASK_ASKED));
    let (status, body) = post_run_authed(
        &env,
        Scripted::writes("asked.txt", false),
        json!({ "approve": ["no-such-rule"] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("no-such-rule"), "{body}");
    assert!(!env.root.join("asked.txt").exists());
}

#[tokio::test]
async fn test_run_when_approve_given_but_no_policy_should_return_400() {
    let env = env_with_policy(None);
    let (status, body) = post_run_authed(
        &env,
        Scripted::writes("x.txt", false),
        json!({ "approve": ["ask-write"] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(!env.root.join("x.txt").exists());
}

#[tokio::test]
async fn test_run_when_approve_is_empty_list_should_behave_like_absent() {
    let env = env_with_policy(None);
    let (status, body) = post_run(
        &env,
        Scripted::writes("x.txt", false),
        json!({ "approve": [] }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(env.root.join("x.txt").exists());
}

#[tokio::test]
async fn test_run_when_policy_file_is_broken_should_return_400_without_file_text() {
    let env = env_with_policy(Some("secret_marker_ZXQ = = [[ not toml"));
    let (status, body) = post_run(&env, Scripted::writes("x.txt", false), json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        !body.contains("secret_marker_ZXQ"),
        "policy file text leaked into the response: {body}"
    );
    assert!(
        !env.root.join("x.txt").exists(),
        "no run on a broken policy"
    );
}

#[tokio::test]
async fn test_run_when_provider_runs_own_tools_and_policy_nonempty_should_return_400() {
    let env = env_with_policy(Some(DENY_BLOCKED));
    let (status, body) = post_run(&env, Scripted::writes("x.txt", true), json!({})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(body.contains("runs its own tools"), "{body}");
    assert!(body.contains("HONE_ALLOW_UNGATED_PROVIDER=1"), "{body}");
    assert!(!env.root.join("x.txt").exists());
}

#[tokio::test]
async fn test_run_when_provider_runs_own_tools_and_no_policy_should_not_refuse() {
    let env = env_with_policy(None);
    let (status, body) = post_run(&env, Scripted::writes("x.txt", true), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn test_run_when_provider_runs_own_tools_and_policy_empty_should_not_refuse() {
    let env = env_with_policy(Some("version = 2\n"));
    let (status, body) = post_run(&env, Scripted::writes("x.txt", true), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

// -- `approve` requires an authenticated server -------------------------------------

#[tokio::test]
async fn test_run_when_approve_given_and_server_has_no_auth_token_should_return_400() {
    let env = env_with_policy(Some(ASK_ASKED));
    let (status, body) = post_run(
        &env,
        Scripted::writes("asked.txt", false),
        json!({ "approve": ["ask-write"] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("approve requires an authenticated server"),
        "{body}"
    );
    assert!(
        !env.root.join("asked.txt").exists(),
        "an unauthenticated caller must not self-approve an ask rule"
    );
}

#[tokio::test]
async fn test_run_when_approve_given_without_auth_and_no_policy_should_still_return_400() {
    // The auth check must not depend on a policy being loaded.
    let env = env_with_policy(None);
    let (status, body) = post_run(
        &env,
        Scripted::writes("x.txt", false),
        json!({ "approve": ["whatever"] }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body.contains("approve requires an authenticated server"),
        "{body}"
    );
}

#[tokio::test]
async fn test_run_when_approve_absent_and_no_auth_token_should_be_unchanged() {
    let env = env_with_policy(Some(ASK_ASKED));
    let (status, body) = post_run(&env, Scripted::writes("asked.txt", false), json!({})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("requires approval"), "{body}");
}

#[tokio::test]
async fn test_run_when_approve_null_and_no_auth_token_should_be_unchanged() {
    let env = env_with_policy(None);
    let (status, body) = post_run(
        &env,
        Scripted::writes("x.txt", false),
        json!({ "approve": null }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(env.root.join("x.txt").exists());
}
