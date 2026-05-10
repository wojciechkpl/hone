# Orchestration GUI Flow Architecture

This document visualizes the end-to-end `/plan` command flow across Hone's TUI and web UI surfaces. Both surfaces reuse the same backend `/orchestrate?dry_run=true` endpoint; the difference lies in how they dispatch the decomposition, render the preview, and handle user actions.

The TUI path runs the orchestrator in-process (`Orchestrator::decompose`), while the web UI streams SSE events from the daemon. Both paths surface the same state shape: goal, cost estimates, and subtask list.

---

## TUI Flow: Local Decomposition

```mermaid
sequenceDiagram
    actor User
    participant TUI as hone (TUI)<br/>bins/hone/src/main.rs
    participant App as App State<br/>hone_tui/src/app.rs
    participant Dispatch as dispatch_plan_decompose<br/>bins/hone/src/main.rs:1173
    participant Orch as Orchestrator<br/>hone_core/src/orchestrator.rs
    participant LLM as Lead Agent<br/>(decompose call)
    participant Render as render_plan_preview<br/>hone_tui/src/render/plan_preview.rs

    User->>TUI: Type /plan <prompt>
    TUI->>App: handle_command(Command::Plan)
    App->>App: set_pending_plan_prompt(prompt)
    
    Note over TUI: Main event loop continues
    
    TUI->>TUI: Check app.take_pending_plan_prompt()
    TUI->>Dispatch: dispatch_plan_decompose(config, cwd, prompt)
    Dispatch->>Orch: Orchestrator::new(lead_config, registry, 4)
    Dispatch->>Orch: decompose(prompt, events_tx)
    Orch->>LLM: Run lead agent once (plan-mode system prompt)
    LLM-->>Orch: Returns TaskPlan + lead_cost + usage
    Dispatch-->>TUI: Ok((plan, lead_cost, usage))
    
    TUI->>App: PlanPreviewState::from_decompose(prompt, plan, lead_cost)
    App->>App: set_plan_preview(state)
    TUI->>Render: Render overlay on next frame
    
    Note over Render: Display goal, paid/projected costs, subtasks (up to 20)
    Note over Render: Footer: [r]un / [e]dit / [c]ancel
    
    User->>TUI: Press 'r' or Enter
    TUI->>App: handle_key_plan_preview(KeyCode::Char('r'))
    App->>App: set_pending_plan_action(PlanAction::Run)
    
    Note over TUI: Next main loop iteration
    
    TUI->>App: take_pending_plan_action()
    TUI->>App: set_input_buffer(prompt)
    TUI->>App: submit_input()
    
    Note over TUI: Normal turn dispatch (Simple/Medium/Complex tier)
```

**Key functions and files:**
- `bins/hone/src/main.rs:handle_command` (line ~2800) — intercepts `/plan` slash command
- `bins/hone/src/main.rs:dispatch_plan_decompose` (line 1173) — in-process orchestrator call
- `hone_tui/src/app.rs` — state holders: `pending_plan_prompt`, `set_plan_preview`
- `hone_core/src/orchestrator.rs:Orchestrator::decompose` — decomposition-only path
- `hone_tui/src/render/plan_preview.rs:render_plan_preview` (line 20) — overlay rendering
- `hone_tui/src/plan_preview.rs` — `PlanPreviewState` and `PlanAction` types

---

## Web UI Flow: SSE Streaming Decomposition

```mermaid
sequenceDiagram
    actor User
    participant Chat as ChatPanel<br/>apps/webui/src/components/ChatPanel.tsx
    participant Intercept as /plan interceptor<br/>ChatPanel.tsx:239
    participant Orch as sendOrchestrate<br/>ChatPanel.tsx
    participant Daemon as orchestrate_handler<br/>crates/hone-server/src/orchestration.rs:187
    participant LLM as Lead Agent<br/>(decompose call)
    participant SSE as SSE Stream
    participant Worker as WorkerProgressPanel<br/>apps/webui/src/components/WorkerProgressPanel.tsx
    participant DryRun as DryRunCard<br/>WorkerProgressPanel.tsx:78

    User->>Chat: Type /plan <prompt> in input
    Chat->>Intercept: detectSlashCommand('/plan <prompt>')
    Intercept->>Orch: sendOrchestrate(prompt, { dryRun: true })
    Orch->>Daemon: POST /orchestrate?dry_run=true with prompt
    
    Note over Daemon: Query param: DryRunQuery { dry_run: true }
    
    Daemon->>Daemon: resolve_working_dir(req.working_dir, workspace_root)
    Daemon->>Daemon: build_lead_agent_config(provider, model, working_dir)
    Daemon->>Orch: Orchestrator::new(lead_config, registry, max_workers)
    
    Note over Daemon: Branch: is_dry_run = true
    
    Daemon->>Orch: decompose(prompt, events_tx)
    Orch->>LLM: Run lead agent (one LLM call only)
    LLM-->>Orch: Returns TaskPlan + lead_cost + usage
    
    Note over Daemon: Emit model_info, plan_created, budget_progress, done
    
    Daemon->>SSE: Send AgentEvent::PlanCreated { goal, subtask_count }
    Daemon->>SSE: Send AgentEvent::BudgetProgress { spent_usd, budget_usd, workers_*  }
    Daemon->>SSE: Send AgentEvent::Done { usage, cost_usd, plan: Some(serde_json) }
    
    SSE-->>Chat: Stream events over EventSource
    Chat->>Chat: dispatch(event) on each event
    
    Note over Chat: Drain pending_plan_action from signal
    Chat->>Chat: setPendingPlanAction → setPendingPlanActionSignal
    Chat->>Worker: Update dryRunPreview signal
    
    Worker->>DryRun: Render <DryRunCard preview={dryRun()} />
    
    Note over DryRun: Display goal, paid/projected costs, subtasks
    Note over DryRun: Buttons: [Run] [Edit] [Cancel]
    
    User->>DryRun: Click [Run]
    DryRun->>DryRun: onRun() → setPendingPlanAction({ kind: 'run', preview })
    DryRun->>DryRun: clearDryRunPreview()
    
    Note over Chat: drains pending_plan_action signal in createEffect
    Chat->>Chat: sendOrchestrate(prompt) — WITHOUT dryRun flag
    
    Note over Chat: Normal orchestration path executes workers
```

**Key functions and files:**
- `apps/webui/src/components/ChatPanel.tsx:239` — slash-command interception
- `crates/hone-server/src/orchestration.rs:187` (line 187) — `orchestrate_handler` endpoint
- `crates/hone-server/src/orchestration.rs:251-322` — dry-run branch logic
- `crates/hone-server/src/orchestration.rs:DryRunQuery` (line 76) — query parameter shape
- `apps/webui/src/components/WorkerProgressPanel.tsx:78` — `<DryRunCard>` component
- `hone_core/src/lib.rs:89` — `DEFAULT_WORKER_COST_USD = 0.05` constant

---

## Governance Overrides: `/budget` and `/permit`

Both surfaces feed per-run governance flags into the turn dispatch:

```mermaid
sequenceDiagram
    actor User
    participant CLI as hone (TUI)
    participant App as App State
    participant Cmd as handle_command<br/>hone_tui/src/user_commands.rs
    participant Gov as PendingGovernance<br/>hone_tui/src/governance.rs

    User->>CLI: Type /budget 0.50
    CLI->>Cmd: parse Command::Budget(0.50)
    Cmd->>App: set_pending_governance(PendingGovernance { budget_usd: Some(0.50), permits: [] })
    
    User->>CLI: Type /permit shell
    CLI->>Cmd: parse Command::Permit("shell")
    Cmd->>App: append to pending_governance.permits
    
    User->>CLI: Type /plan ...
    Note over CLI: ... plan preview shown ...
    
    User->>CLI: Press 'r' (run)
    Note over CLI: Main loop drains pending_governance before dispatching
    
    CLI->>App: take_pending_governance()
    App->>App: agent.set_budget_limit(Some(0.50))
    App->>App: agent.set_approval_policy(...).approve_always("shell")
    
    Note over App: Governance drained — next turn has defaults
```

**Key types and functions:**
- `hone_tui/src/governance.rs:PendingGovernance` (line 6) — state struct
- `bins/hone/src/main.rs:2980` — drain point in main loop
- `hone_core/src/agent.rs` (AgentHandle) — `set_budget_limit`, `set_approval_policy` setters
- `hone_proto::ToolApprovalPolicy::approve_always` — per-tool allowlist API

---

## Cost Estimate Formula

Both TUI and web UI use the same formula:

```
projected_cost_usd = lead_cost_usd + subtask_count × DEFAULT_WORKER_COST_USD
```

Where `DEFAULT_WORKER_COST_USD = 0.05` (defined at `hone_core/src/lib.rs:89`).

The lead cost is the real, measured cost from the decomposition LLM call. The worker estimates are heuristic (not measured) and based on the constant above. Users can override per-run with `/budget <usd>` (TUI) or `POST /orchestrate` with `total_budget_usd` (web UI).

---

## See Also

- `docs/tutorials/orchestration-gui.md` — User-facing guide for `/plan`, `/budget`, `/permit`, `/workflows`
- `docs/plans/orchestration-gui-tdd-2026-05.md` — Implementation roadmap and slice definitions
- `docs/research/gui-for-non-cli-users-2026-05.md` — Audience analysis and UX research
- `docs/research/tui-orchestration-integration-2026-05.md` — TUI-specific integration patterns
