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

## Review round 1 (rows G13–G18)

The reviewer found that background planning lets typed text hit the overlay (B1), that an orchestrated run's many `Done`s end the turn early in the TUI (S1), and that the plan run drained `/permit` without applying it (S2).

| Row | Contract | Where tested |
|---|---|---|
| **G13** | **The overlay can't be answered by accident.** In `App::handle_key_at(key, now)` with the overlay open, `r`, `e`, `c` and Enter act only when `now` is at least `MENU_KEY_GUARD` (400 ms) after the overlay opened (`set_plan_preview`) **and** after the previous keystroke, as the approval menu does. A key failing the guard is dropped: the overlay stays open and nothing reaches the input bar. Esc cancels at once. `handle_key_plan_preview` ignores `r`, `e`, `c` and Enter carrying any modifier (Ctrl+R, Alt+Enter, Shift+Enter are no-ops). Text pasted while the overlay is open is dropped. | hone-tui (`handle_key_at` clock seam) |
| **G14** | **An orchestrated turn ends once.** `spawn_orchestrated_turn` (Complex turns and plan runs) forwards the orchestrator's events except `Done` and `Error`, which belong to its inner lead and worker turns (a failed worker is already reported by `WorkerCompleted { status: "failed: …" }`). It then sends exactly one terminal event: on success the summary `Token`, then one `Done` whose `usage` and `cost_usd` are the sums of the swallowed `Done`s (`plan: None`); on failure one `Error { message: "orchestration failed: …" }` and no `Done`. So the TUI stays busy (spinner, Esc cancels, `/plan` refused) until the run really ends. | bins/hone `mod tests` (existing `spawn_orchestrated_turn` harness), with G7's no-decompose and history write-back for `Some(plan)` |
| **G15** | **`/plan` waits for the previous turn's task.** If `last_turn_task` hasn't finished (a medium turn or the automatic test fix keeps running after its `Done`), the loop doesn't plan and calls the new `App::refuse_plan()`: an `Error` message `"/plan: the previous turn is still finishing; try again when it's done"`, state `Idle`, `turn_start` cleared, nothing staged. | hone-tui (`refuse_plan`); loop by reading |
| **G16** | **A plan run gets the turn settings of a Complex turn, through one helper.** The model and governance steps shared by Complex turns, plan runs and planning move into one helper so they can't drift. Pending `/budget` and `/permit` are applied to the agent (`set_budget_limit`, `approve_tools`) **and** to the run's config, as every turn does, so a `/permit` drained by a plan run isn't lost. With `[[routing]]` off, the config's model is the agent's current model (`applied_model`), so `/model` reaches orchestrated runs and planning; with routing on, the Complex model as before. | by reading |
| **G17** | **Small fixes.** Run adds the prompt to Up-arrow history, as a submitted message does. A planning task that ends without sending a result (a panic) leads to `plan_failed("planning stopped unexpectedly")`, not an endless spinner. `set_input_buffer`'s doc comment no longer mentions Run. | hone-tui (history); loop by reading |
| **G18** | **Test gaps from review:** Esc during planning goes through `handle_key` and requests a cancel; `run_plan` runs a dependency after its prerequisite (call order). | hone-tui, hone-core |

## Review round 2 (rows G19–G22)

The reviewer approved round 2 with three low findings: with one terminal `Done`, earlier messages of an orchestrated run stay streaming; `BudgetProgress` overwrites the session cost, which the summed `Done` then adds to; and the G18 test can't tell dependency order from list order.

| Row | Contract | Where tested |
|---|---|---|
| **G19** | **No message is left streaming.** Before pushing the System message for `PlanCreated`, `WorkerStarted` or `WorkerCompleted`, the App ends a streaming message, as the `ModelFallback` handler does. `Done`, `Error` and `cancel_turn` clear `is_streaming` on every message, not only the last. After the event sequence `spawn_orchestrated_turn` sends (tokens, plan and worker events, worker tokens, summary token, one `Done`), no message is streaming. | hone-tui |
| **G20** | **`BudgetProgress` doesn't touch the session cost.** It keeps updating `budget_usd`, the mode label and the phase, but `session_cost_usd` changes only by `Done`'s `cost_usd`. A session at 1.0 that runs an orchestration reporting `BudgetProgress { spent_usd: 0.3 }` and then `Done { cost_usd: 0.5 }` ends at 1.5. | hone-tui |
| **G21** | **A refused `/plan` leaves no "Planning:" line.** `refuse_plan` removes the `"Planning: <prompt>"` System message when it is the last message, so the transcript shows only the refusal. | hone-tui |
| **G22** | **Test strength:** G18 lists the dependent subtask first, so only dependency order can pass; a run in which one worker fails and the run succeeds forwards the failed `WorkerCompleted`, no `Error`, and exactly one `Done`. | hone-core, bins/hone |

## Out of scope (follow-ups)

- **Orchestrated workers have no tools.** `Orchestrator::build_lead_config` and `build_worker_config` give the lead and every worker an empty `ToolRegistry` (`crates/hone-core/src/orchestrator.rs:772`, `:830`), so a Complex turn or `/plan` Run can't read or edit files: workers answer in text only. (Their hard-coded `AgentMode::Build` / `ToolApprovalPolicy::Auto` is moot without tools.) Pre-existing; affects every orchestrated path.
- **Web UI Run plans again:** the `<DryRunCard>` Run sends `POST /orchestrate` without `dry_run`, which decomposes anew.
- **Planning's fallback notices** are still dropped (the decompose drains its own events).
- **`/budget` doesn't cap a plan's workers:** it reaches only the orchestrator's lead config (merge call); workers use `worker_budget`, unset in the TUI. Wiring it to `Orchestrator::with_total_budget` is a follow-up.
- **Vim mode:** Esc never requests a cancel there (pre-existing), so "Esc stops it" doesn't hold in vim mode.
- **Planning's lead-call cost** isn't added to the session cost total.
- **Edit replaces the input bar's text,** including a follow-up typed while planning and restored when planning ended.
- **`/budget` and `/permit` persist:** the docs called them per-run, but `SetBudgetLimit` and `ApproveTools` stay on the agent for the session. Docs now say so; making them per-run is a follow-up.
- **Status label after an orchestrated run:** `BudgetProgress` sets the mode label to `orchestrator i/n`, and nothing restores the agent mode label after the run (pre-existing).
- **Other drains in the Enter block:** a `!command` typed during a turn waits for the next Enter with text, like the plan actions did.
