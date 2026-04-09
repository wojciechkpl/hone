use hone_proto::AgentEvent;

/// Encode an `AgentEvent` as a Server-Sent Events frame.
///
/// Format:
/// ```text
/// id: {id}
/// event: {type}
/// data: {json}
///
/// ```
pub fn encode_event(event: &AgentEvent, event_id: u64) -> String {
    let event_type = match event {
        AgentEvent::Token { .. } => "token",
        AgentEvent::ToolCallStart { .. } => "tool_call_start",
        AgentEvent::ToolCallEnd { .. } => "tool_call_end",
        AgentEvent::Done { .. } => "done",
        AgentEvent::Error { .. } => "error",
        AgentEvent::ApprovalRequired { .. } => "approval_required",
        AgentEvent::PlanCreated { .. } => "plan_created",
        AgentEvent::WorkerStarted { .. } => "worker_started",
        AgentEvent::WorkerCompleted { .. } => "worker_completed",
        AgentEvent::BudgetProgress { .. } => "budget_progress",
    };

    let data = serde_json::to_string(event).unwrap_or_else(|_| "{}".to_owned());

    format!("id: {event_id}\nevent: {event_type}\ndata: {data}\n\n")
}
