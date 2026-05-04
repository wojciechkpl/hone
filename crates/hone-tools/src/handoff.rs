//! `handoff` tool — workers signal "give this task to another agent".
//!
//! Records a [`HandoffRequest`] into a shared [`HandoffBuffer`] that the
//! orchestrator drains at end-of-turn (orchestrator integration is a separate
//! follow-up). The tool itself performs no filesystem or process side-effects;
//! `is_read_only()` returns `true`.

use async_trait::async_trait;
use hone_proto::handoff::{HandoffBuffer, HandoffRequest};
use hone_proto::ToolLayer;
use serde_json::{json, Value};

use crate::error::ToolError;
use crate::traits::{Tool, ToolContext, ToolInput, ToolOutput};

pub struct HandoffTool {
    buffer: HandoffBuffer,
    valid_targets: Option<Vec<String>>,
    origin_agent: Option<String>,
}

impl HandoffTool {
    pub fn new(buffer: HandoffBuffer) -> Self {
        Self {
            buffer,
            valid_targets: None,
            origin_agent: None,
        }
    }

    pub fn with_valid_targets(mut self, targets: Vec<String>) -> Self {
        self.valid_targets = Some(targets);
        self
    }

    pub fn with_origin_agent(mut self, name: impl Into<String>) -> Self {
        self.origin_agent = Some(name.into());
        self
    }
}

#[async_trait]
impl Tool for HandoffTool {
    fn name(&self) -> &str {
        "handoff"
    }

    fn description(&self) -> &str {
        "Hand off the current task to another agent. Use this when the work is \
         outside your expertise or needs review by a specialist."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "target_agent": {
                    "type": "string",
                    "description": "Name of the agent to hand off to (e.g. 'code-reviewer')."
                },
                "payload": {
                    "type": "string",
                    "description": "What the target agent should work on. Be specific."
                },
                "reason": {
                    "type": "string",
                    "description": "Brief justification for the handoff."
                }
            },
            "required": ["target_agent", "payload", "reason"]
        })
    }

    fn layer(&self) -> ToolLayer {
        ToolLayer::Native
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn call(&self, input: ToolInput, _ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let target_agent = input
            .get_str("target_agent")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing field 'target_agent'".into(),
            })?
            .to_string();
        let payload = input
            .get_str("payload")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing field 'payload'".into(),
            })?
            .to_string();
        let reason = input
            .get_str("reason")
            .ok_or_else(|| ToolError::InvalidInput {
                reason: "missing field 'reason'".into(),
            })?
            .to_string();

        if target_agent.is_empty() {
            return Err(ToolError::InvalidInput {
                reason: "target_agent must not be empty".into(),
            });
        }

        if let Some(targets) = &self.valid_targets {
            if !targets.iter().any(|t| t == &target_agent) {
                return Err(ToolError::InvalidInput {
                    reason: format!(
                        "target_agent '{target_agent}' not in valid targets: [{}]",
                        targets.join(", ")
                    ),
                });
            }
        }

        let request = HandoffRequest {
            target_agent: target_agent.clone(),
            payload,
            reason,
            origin_agent: self.origin_agent.clone(),
        };
        self.buffer.record(request);

        Ok(ToolOutput::new(json!({
            "handed_off": true,
            "target_agent": target_agent,
            "queued": self.buffer.len(),
        })))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use futures::future::join_all;
    use hone_proto::handoff::HandoffBuffer;
    use hone_sandbox::NullSandbox;
    use serde_json::json;

    use super::*;

    fn ctx() -> ToolContext {
        ToolContext {
            sandbox: Arc::new(NullSandbox),
            working_dir: PathBuf::from("/tmp"),
        }
    }

    fn good_input() -> ToolInput {
        ToolInput::new(json!({
            "target_agent": "code-reviewer",
            "payload": "review src/auth.rs",
            "reason": "needs second look"
        }))
    }

    #[tokio::test]
    async fn tool_name_is_handoff() {
        let tool = HandoffTool::new(HandoffBuffer::new());
        assert_eq!(tool.name(), "handoff");
    }

    #[tokio::test]
    async fn tool_is_read_only_returns_true() {
        let tool = HandoffTool::new(HandoffBuffer::new());
        assert!(tool.is_read_only());
    }

    #[tokio::test]
    async fn tool_schema_requires_three_fields() {
        let tool = HandoffTool::new(HandoffBuffer::new());
        let schema = tool.schema();
        let required = schema["required"].as_array().expect("required is array");
        let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(names.contains(&"target_agent"));
        assert!(names.contains(&"payload"));
        assert!(names.contains(&"reason"));
    }

    #[tokio::test]
    async fn tool_call_records_request_into_buffer() {
        let buf = HandoffBuffer::new();
        let tool = HandoffTool::new(buf.clone());
        tool.call(good_input(), &ctx()).await.unwrap();
        assert_eq!(buf.len(), 1);
        let drained = buf.drain();
        assert_eq!(drained[0].target_agent, "code-reviewer");
        assert_eq!(drained[0].payload, "review src/auth.rs");
        assert_eq!(drained[0].reason, "needs second look");
        assert_eq!(drained[0].origin_agent, None);
    }

    #[tokio::test]
    async fn tool_call_returns_handed_off_true_in_output() {
        let tool = HandoffTool::new(HandoffBuffer::new());
        let out = tool.call(good_input(), &ctx()).await.unwrap();
        let v = out.into_value();
        assert_eq!(v["handed_off"], json!(true));
        assert_eq!(v["target_agent"], json!("code-reviewer"));
        assert_eq!(v["queued"], json!(1));
    }

    #[tokio::test]
    async fn tool_call_with_origin_agent_sets_origin() {
        let buf = HandoffBuffer::new();
        let tool = HandoffTool::new(buf.clone()).with_origin_agent("debugger");
        tool.call(good_input(), &ctx()).await.unwrap();
        let drained = buf.drain();
        assert_eq!(drained[0].origin_agent, Some("debugger".into()));
    }

    #[tokio::test]
    async fn tool_call_missing_target_agent_returns_error() {
        let tool = HandoffTool::new(HandoffBuffer::new());
        let bad = ToolInput::new(json!({"payload": "p", "reason": "r"}));
        assert!(tool.call(bad, &ctx()).await.is_err());
    }

    #[tokio::test]
    async fn tool_call_missing_payload_returns_error() {
        let tool = HandoffTool::new(HandoffBuffer::new());
        let bad = ToolInput::new(json!({"target_agent": "a", "reason": "r"}));
        assert!(tool.call(bad, &ctx()).await.is_err());
    }

    #[tokio::test]
    async fn tool_call_missing_reason_returns_error() {
        let tool = HandoffTool::new(HandoffBuffer::new());
        let bad = ToolInput::new(json!({"target_agent": "a", "payload": "p"}));
        assert!(tool.call(bad, &ctx()).await.is_err());
    }

    #[tokio::test]
    async fn tool_call_empty_target_agent_returns_error() {
        let tool = HandoffTool::new(HandoffBuffer::new());
        let bad = ToolInput::new(json!({"target_agent": "", "payload": "p", "reason": "r"}));
        assert!(tool.call(bad, &ctx()).await.is_err());
    }

    #[tokio::test]
    async fn tool_call_target_not_in_valid_targets_returns_error_listing_valid() {
        let tool = HandoffTool::new(HandoffBuffer::new())
            .with_valid_targets(vec!["code-reviewer".into(), "security-auditor".into()]);
        let bad = ToolInput::new(json!({
            "target_agent": "ghost",
            "payload": "p",
            "reason": "r"
        }));
        let err = tool.call(bad, &ctx()).await.unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("code-reviewer"), "msg: {msg}");
        assert!(msg.contains("security-auditor"), "msg: {msg}");
    }

    #[tokio::test]
    async fn tool_call_target_in_valid_targets_succeeds() {
        let tool = HandoffTool::new(HandoffBuffer::new())
            .with_valid_targets(vec!["code-reviewer".into()]);
        tool.call(good_input(), &ctx()).await.unwrap();
    }

    #[tokio::test]
    async fn tool_call_with_empty_valid_targets_rejects_every_call() {
        let tool = HandoffTool::new(HandoffBuffer::new()).with_valid_targets(vec![]);
        assert!(tool.call(good_input(), &ctx()).await.is_err());
    }

    #[tokio::test]
    async fn tool_call_records_only_on_success() {
        let buf = HandoffBuffer::new();
        let tool = HandoffTool::new(buf.clone());
        let bad = ToolInput::new(json!({"payload": "p", "reason": "r"}));
        let _ = tool.call(bad.clone(), &ctx()).await;
        let _ = tool.call(bad, &ctx()).await;
        tool.call(good_input(), &ctx()).await.unwrap();
        assert_eq!(buf.len(), 1);
    }

    #[tokio::test]
    async fn tool_call_concurrent_records_are_serialized() {
        let buf = HandoffBuffer::new();
        let tool = Arc::new(HandoffTool::new(buf.clone()));
        let ctx = ctx();
        let futures = (0..10).map(|i| {
            let tool = Arc::clone(&tool);
            let ctx = ctx.clone();
            async move {
                let input = ToolInput::new(json!({
                    "target_agent": "code-reviewer",
                    "payload": format!("payload-{i}"),
                    "reason": "concurrent"
                }));
                tool.call(input, &ctx).await.unwrap();
            }
        });
        join_all(futures).await;
        assert_eq!(buf.drain().len(), 10);
    }
}
