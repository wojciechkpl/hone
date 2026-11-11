# `/plan` gate fixes: spec

Branch `fix/plan-gate`. Contract for the red tests (rows G1–G12).

## The problems

`/plan <prompt>` asks the orchestrator's lead model for a plan, shows it in an overlay, and offers Run (`r`/Enter), Edit (`e`) and Cancel (`c`/Esc). Today:

1. **The overlay's keys wait.** The key sets `pending_plan_action`, but the TUI loop reads it only inside its `Enter && !input.is_empty() && Idle|Error` block, so nothing happens until the next Enter with text typed. That Enter also submits the typed text as a turn, and Run then pushes a second user message that never runs.
2. **Edit loses the prompt.** `handle_key_plan_preview` clears `plan_preview` on `e`, and the loop reads the prompt from it afterwards.
3. **Run doesn't run the plan shown.** It puts the prompt back through `submit_input`, so the complexity classifier picks the tier again: a Simple or Medium verdict runs one agent and no workers; a Complex verdict asks the lead for a new plan, paying for it again and possibly getting a different one.
4. **The TUI freezes while planning.** The loop awaits `dispatch_plan_decompose` inside the key branch: nothing is drawn (not even "Planning: …"), and Esc and Ctrl+C don't work until the lead answers.
5. **`set_input_buffer` puts the cursor at the char count**, but `InputState::cursor_position` is a byte offset. With a non-ASCII prompt the cursor lands mid-text, or mid-character, and the next typed character panics in `String::insert`.

## The fix

```mermaid
%%{init: {"themeVariables": {"fontSize": "13px"}, "flowchart": {"nodeSpacing": 25, "rankSpacing": 30, "padding": 8}, "sequence": {"actorMargin": 30, "messageMargin": 25, "boxMargin": 6}, "gantt": {"barHeight": 16, "barGap": 3}}}%%
sequenceDiagram
  participant You
  participant App as App (hone-tui)
  participant Loop as TUI loop (bins/hone)
  participant Orch as Orchestrator
  You->>App: /plan refactor auth
  App->>App: "Planning: …", state WaitingForResponse
  Loop->>Orch: spawn decompose (background task)
  Note over Loop: keeps drawing; Esc cancels, late result dropped
  Orch-->>Loop: TaskPlan + lead cost
  Loop->>App: set_plan_preview → overlay, state Idle
  alt r / Enter
    App->>App: user message, state WaitingForResponse,<br/>pending_plan_run = preview
    Loop->>Orch: run_plan(plan shown) — no second decompose
  else e
    App->>App: input bar = prompt, cursor at end
  else c / Esc
    App->>App: overlay closed, plan dropped
  end
```

## Rows

| Row | Contract | Where tested |
|---|---|---|
| **G1** | The overlay's keys act at once. `e` and `c`/Esc are handled entirely by `App::handle_key_plan_preview`. `r`/Enter stages the run in the App (G4); the TUI loop drains it after **every** key event, not only inside the Enter-with-text block. | hone-tui (App); loop by reading |
| **G2** | **Edit** (`e`): overlay closed, preview cleared, input bar set to the planned prompt exactly as given to `/plan` (trimmed, without `/plan `), replacing what was there, cursor at the end. No message pushed, state unchanged. | hone-tui |
| **G3** | **Cancel** (`c`/Esc): overlay closed, preview cleared, input bar and state unchanged, nothing staged. Any other key: no-op, overlay stays open. | hone-tui |
| **G4** | **Run** (`r`/Enter): overlay closed; preview moved out (`plan_preview()` is `None`); a `User` message with the prompt is pushed; the App starts a turn exactly as a plain submitted message does (`AppState::WaitingForResponse`, `turn_start` set, per-turn fallback notices cleared, active agent `"default"`). `App::take_pending_plan_run() -> Option<PlanPreviewState>` returns the preview (prompt and full plan) once, then `None`. | hone-tui |
| **G5** | `PlanPreviewState` gains `pub plan: hone_core::TaskPlan`, equal to the plan passed to `from_decompose` (every `SubTask` field kept: `depends_on`, `context`, `scope`, `acceptance_tests`, `budget_usd`, `requires_capabilities`). Existing fields and the `from_decompose` signature are unchanged. | hone-tui |
| **G6** | `Orchestrator::run_plan(&mut self, plan: TaskPlan, events) -> OrchestratorResult` runs a given plan: **no decompose (lead) call**; sends `PlanCreated { goal, subtask_count }` before any worker event; writes the plan store before and after, as `run` does; executes the subtasks (dependency order as `execute_plan`); merges with the plan's goal. `run` becomes decompose followed by `run_plan`, with unchanged behaviour. | hone-core |
| **G7** | The TUI loop runs a staged plan with `spawn_orchestrated_turn(…, Some(plan))`, which calls `run_plan` instead of `run`, with the rest of the task (history write-back of prompt + merged answer, summary token, error event) unchanged. The run uses the settings of a Complex turn: a deferred `/model` change applied first, then the pinned model or the `[[routing]]` complex model or the session model; `/budget` and `/permit` drained into its config. Its abort handle is the current task's, so Esc cancels it. The adaptive classifier records nothing for it (`current_tier = None`: the classifier didn't pick the tier); `current_turn_prompt`, `current_turn_model` and `turn_fallback_model` are reset as at any turn start. | by reading (no test drives the TUI loop); `run_plan` in hone-core |
| **G8** | **Planning runs in the background.** `/plan <prompt>` when Idle or Error: pushes "Planning: <prompt>", sets `WaitingForResponse` and `turn_start`, stages `pending_plan_prompt`. The loop spawns the decompose (same model choice as today) and keeps drawing. Esc while planning cancels it like a turn (`cancel_turn`), and a result arriving after that is dropped. On success the loop calls `set_plan_preview`, which opens the overlay, sets `Idle` and clears `turn_start`. On failure it calls the new `App::plan_failed(msg)`, which pushes an `Error` message `"/plan decompose failed: <msg>"` and sets `Idle`. | hone-tui (App); loop by reading |
| **G9** | `/plan <prompt>` while `WaitingForResponse` (a turn or a plan is running) is refused: an `Error` message `"/plan: wait for the running turn to finish (Esc stops it)"`, nothing staged, state unchanged. `/plan` with an empty prompt keeps its usage error. | hone-tui |
| **G10** | `App::set_input_buffer` puts the cursor at the end of the buffer **in bytes** (`buffer.len()`). After Edit with a non-ASCII prompt (e.g. `zażółć gęślą jaźń`), typing a character appends it at the end and doesn't panic. | hone-tui |
| **G11** | `PlanAction`, `pending_plan_action` and `take_pending_plan_action` are removed: nothing reads them once the App acts on the keys. | compile |
| **G12** | Docs: `docs/tutorials/orchestration-gui.md` (Run runs the plan shown, without planning again; planning can be cancelled with Esc), the TUI half of `docs/architecture/orchestration-gui-flow.md`, `CHANGELOG.md`. | by reading |

## Out of scope (follow-ups)

- **Orchestrated workers have no tools.** `Orchestrator::build_lead_config` and `build_worker_config` give the lead and every worker an empty `ToolRegistry` (`crates/hone-core/src/orchestrator.rs:772`, `:830`), so a Complex turn or `/plan` Run can't read or edit files: workers answer in text only. (Their hard-coded `AgentMode::Build` / `ToolApprovalPolicy::Auto` is moot without tools.) Pre-existing; affects every orchestrated path.
- **Web UI Run plans again:** the `<DryRunCard>` Run sends `POST /orchestrate` without `dry_run`, which decomposes anew.
- **Planning's fallback notices** are still dropped (the decompose drains its own events).
- **Other drains in the Enter block:** a `!command` typed during a turn waits for the next Enter with text, like the plan actions did.
