//! Provenance writer — bridges the agent's `AgentEvent` stream to a
//! persistent, tamper-evident `ProvenanceLog`.
//!
//! Wired in `execution::run_handler` and `orchestration::orchestrate_handler`
//! when `AppState::provenance_dir` is set. The handler builds a
//! [`ProvenanceTee`], appends the user prompt up front, then calls
//! `tee.observe(&event)` on every event flowing through the SSE
//! pipeline. The chain file is `<provenance_dir>/<session_id>.jsonl`,
//! identical in shape to what `ProvenanceLog::open` produces, so the
//! existing `GET /provenance/{id}/verify` endpoint can re-verify it.
//!
//! Token-stream aggregation: tokens flow as `AgentEvent::Token { text }`
//! one piece at a time. We accumulate them in-memory until the next
//! `Done` (or `Error`), then emit a single `ProvenanceEvent::ModelCompletion`
//! with the joined text. This matches the design doc shape (one
//! completion per turn, not one per token).

use std::path::Path;
use std::sync::Arc;

use hone_proto::AgentEvent;
use hone_provenance::{ProvenanceEvent, ProvenanceLog, ProvenanceLogError};
use tokio::sync::Mutex;

/// Map a single [`AgentEvent`] into zero or more [`ProvenanceEvent`]s.
///
/// State machine:
/// - `Token { text }` is buffered into `text_buf`; nothing emitted.
/// - `Done` flushes `text_buf` as `ModelCompletion`, then emits
///   `SessionEnd`.
/// - `Error { message }` flushes any buffered text first, then emits
///   `SessionEnd { reason }`.
/// - `ToolCallStart { name, args }` → `ToolCall`.
/// - `ToolCallEnd { result, is_error }` → `ToolResult`.
/// - Other variants are dropped — they're orchestrator/UX-side and
///   would clutter the provenance log without adding audit value.
pub struct EventMapper {
    text_buf: String,
    model: String,
}

impl EventMapper {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            text_buf: String::new(),
            model: model.into(),
        }
    }

    pub fn map(&mut self, event: &AgentEvent) -> Vec<ProvenanceEvent> {
        match event {
            AgentEvent::Token { text } => {
                self.text_buf.push_str(text);
                Vec::new()
            }
            AgentEvent::Done { .. } => {
                let mut out = Vec::with_capacity(2);
                if !self.text_buf.is_empty() {
                    out.push(ProvenanceEvent::ModelCompletion {
                        model: self.model.clone(),
                        output: std::mem::take(&mut self.text_buf),
                    });
                }
                out.push(ProvenanceEvent::SessionEnd {
                    reason: "done".into(),
                });
                out
            }
            AgentEvent::Error { message } => {
                let mut out = Vec::with_capacity(2);
                if !self.text_buf.is_empty() {
                    out.push(ProvenanceEvent::ModelCompletion {
                        model: self.model.clone(),
                        output: std::mem::take(&mut self.text_buf),
                    });
                }
                out.push(ProvenanceEvent::SessionEnd {
                    reason: format!("error: {message}"),
                });
                out
            }
            AgentEvent::ToolCallStart { name, args, .. } => {
                vec![ProvenanceEvent::ToolCall {
                    name: name.clone(),
                    arguments: args.clone(),
                }]
            }
            AgentEvent::ToolCallEnd {
                result, is_error, ..
            } => {
                vec![ProvenanceEvent::ToolResult {
                    name: String::new(),
                    result: result.clone(),
                    is_error: *is_error,
                }]
            }
            _ => Vec::new(),
        }
    }
}

/// Append a single user prompt as the chain's leading entry. Called by
/// the handler once before the first `tee.observe` call.
pub fn append_user_prompt(
    log: &mut ProvenanceLog,
    prompt: &str,
) -> Result<u64, ProvenanceLogError> {
    log.append(ProvenanceEvent::UserPrompt {
        text: prompt.to_string(),
    })
}

/// Open or create the per-session chain file under `dir`.
pub fn open_session_log(dir: &Path, session_id: &str) -> Result<ProvenanceLog, ProvenanceLogError> {
    let path = dir.join(format!("{session_id}.jsonl"));
    ProvenanceLog::open(&path)
}

/// Inline tee that the handler calls from inside the SSE forwarding
/// pipeline. Each call to [`Self::observe`] maps the event and appends
/// every produced entry to the underlying log.
///
/// Holding the lock per-event is acceptable because:
/// - Append is a small disk write (one JSONL line) — sub-millisecond on
///   any reasonable disk.
/// - The lock is uncontended in practice: the SSE pipeline is the
///   single observer; nothing else touches the log during a run.
/// - We never await across the lock — the append is synchronous.
pub struct ProvenanceTee {
    log: Arc<Mutex<ProvenanceLog>>,
    mapper: EventMapper,
}

impl ProvenanceTee {
    pub fn new(log: Arc<Mutex<ProvenanceLog>>, model: impl Into<String>) -> Self {
        Self {
            log,
            mapper: EventMapper::new(model),
        }
    }

    /// Observe one event. Maps it through [`EventMapper`] and appends
    /// the resulting entries to the chain. Errors are logged via
    /// `tracing::warn!` rather than returned — the SSE pipeline must
    /// not abort because a sidecar log write hiccupped.
    pub async fn observe(&mut self, event: &AgentEvent) {
        let mapped = self.mapper.map(event);
        if mapped.is_empty() {
            return;
        }
        let mut guard = self.log.lock().await;
        for pe in mapped {
            if let Err(err) = guard.append(pe) {
                tracing::warn!("provenance append failed: {err}");
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn mapper_buffers_tokens_until_done_then_emits_completion_and_session_end() {
        let mut m = EventMapper::new("claude");
        assert!(m.map(&AgentEvent::Token { text: "hel".into() }).is_empty());
        assert!(m.map(&AgentEvent::Token { text: "lo".into() }).is_empty());
        let out = m.map(&AgentEvent::Done {
            usage: hone_proto::TokenUsage::default(),
            cost_usd: 0.0,
            plan: None,
        });
        assert_eq!(out.len(), 2);
        assert!(matches!(
            out[0],
            ProvenanceEvent::ModelCompletion { ref output, .. } if output == "hello"
        ));
        assert!(matches!(out[1], ProvenanceEvent::SessionEnd { .. }));
    }

    #[test]
    fn mapper_done_with_no_tokens_emits_only_session_end() {
        let mut m = EventMapper::new("claude");
        let out = m.map(&AgentEvent::Done {
            usage: hone_proto::TokenUsage::default(),
            cost_usd: 0.0,
            plan: None,
        });
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0], ProvenanceEvent::SessionEnd { .. }));
    }

    #[test]
    fn mapper_error_flushes_buffer_then_emits_session_end_with_reason() {
        let mut m = EventMapper::new("claude");
        m.map(&AgentEvent::Token {
            text: "partial".into(),
        });
        let out = m.map(&AgentEvent::Error {
            message: "rate limit".into(),
        });
        assert_eq!(out.len(), 2);
        assert!(
            matches!(out[0], ProvenanceEvent::ModelCompletion { ref output, .. } if output == "partial")
        );
        match &out[1] {
            ProvenanceEvent::SessionEnd { reason } => {
                assert!(reason.contains("rate limit"));
            }
            other => panic!("expected SessionEnd, got {other:?}"),
        }
    }

    #[test]
    fn mapper_tool_call_start_emits_tool_call() {
        let mut m = EventMapper::new("m");
        let args = serde_json::json!({"path": "/tmp/x"});
        let out = m.map(&AgentEvent::ToolCallStart {
            id: "c1".into(),
            name: "read_file".into(),
            args: args.clone(),
        });
        assert_eq!(out.len(), 1);
        match &out[0] {
            ProvenanceEvent::ToolCall { name, arguments } => {
                assert_eq!(name, "read_file");
                assert_eq!(arguments, &args);
            }
            other => panic!("expected ToolCall, got {other:?}"),
        }
    }

    #[test]
    fn mapper_tool_call_end_emits_tool_result_with_is_error_flag() {
        let mut m = EventMapper::new("m");
        let out = m.map(&AgentEvent::ToolCallEnd {
            id: "c1".into(),
            result: serde_json::json!("ok"),
            is_error: false,
            duration_ms: 0,
            cache_hit: false,
        });
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            ProvenanceEvent::ToolResult {
                is_error: false,
                ..
            }
        ));

        let mut m2 = EventMapper::new("m");
        let out = m2.map(&AgentEvent::ToolCallEnd {
            id: "c2".into(),
            result: serde_json::json!("permission denied"),
            is_error: true,
            duration_ms: 0,
            cache_hit: false,
        });
        assert!(matches!(
            out[0],
            ProvenanceEvent::ToolResult { is_error: true, .. }
        ));
    }

    #[test]
    fn mapper_drops_unrelated_events() {
        let mut m = EventMapper::new("m");
        for ev in [
            AgentEvent::ApprovalRequired {
                action: "x".into(),
                description: "y".into(),
            },
            AgentEvent::PlanCreated {
                goal: "z".into(),
                subtask_count: 1,
            },
            AgentEvent::WorkerStarted {
                task_id: "t".into(),
                agent: "a".into(),
                description: "d".into(),
            },
            AgentEvent::ModelInfo {
                context_window: 8000,
            },
        ] {
            assert!(m.map(&ev).is_empty(), "{ev:?} should map to nothing");
        }
    }

    #[tokio::test]
    async fn tee_end_to_end_produces_a_verifiable_chain() {
        use tempfile::TempDir;

        let tmp = TempDir::new().expect("tmp");
        let log = open_session_log(tmp.path(), "session-1").expect("open");
        let log = Arc::new(Mutex::new(log));

        // Append the user-prompt entry up front (the way handlers do).
        {
            let mut g = log.lock().await;
            append_user_prompt(&mut g, "do a thing").expect("prompt");
        }

        let mut tee = ProvenanceTee::new(Arc::clone(&log), "claude");
        // Drive a tiny session: token, token, tool_call, tool_result, done.
        for ev in [
            AgentEvent::Token { text: "hi ".into() },
            AgentEvent::Token {
                text: "there".into(),
            },
            AgentEvent::ToolCallStart {
                id: "c1".into(),
                name: "read_file".into(),
                args: serde_json::json!({"path": "/tmp/x"}),
            },
            AgentEvent::ToolCallEnd {
                id: "c1".into(),
                result: serde_json::json!("file contents"),
                is_error: false,
                duration_ms: 0,
                cache_hit: false,
            },
            AgentEvent::Done {
                usage: hone_proto::TokenUsage::default(),
                cost_usd: 0.0,
                plan: None,
            },
        ] {
            tee.observe(&ev).await;
        }

        let g = log.lock().await;
        let entries = g.entries();
        // Expected: prompt, tool_call, tool_result, model_completion, session_end.
        assert_eq!(
            entries.len(),
            5,
            "got {} entries: {entries:?}",
            entries.len()
        );
        assert!(matches!(
            entries[0].event,
            ProvenanceEvent::UserPrompt { .. }
        ));
        assert!(matches!(entries[1].event, ProvenanceEvent::ToolCall { .. }));
        assert!(matches!(
            entries[2].event,
            ProvenanceEvent::ToolResult { .. }
        ));
        assert!(matches!(
            entries[3].event,
            ProvenanceEvent::ModelCompletion { .. }
        ));
        assert!(matches!(
            entries[4].event,
            ProvenanceEvent::SessionEnd { .. }
        ));
        // The chain itself must verify (load-time + manual).
        g.verify().expect("chain verifies");

        // Extra assurance: a fresh `ProvenanceLog::open` of the same path
        // also verifies — proving the on-disk JSONL is well-formed and
        // that `verify` doesn't depend on in-memory state.
        let path = tmp.path().join("session-1.jsonl");
        let reopened = ProvenanceLog::open(&path).expect("reopen verifies");
        assert_eq!(reopened.entries().len(), 5);
    }
}
