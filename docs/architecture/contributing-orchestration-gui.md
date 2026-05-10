# Contributing to Hone's Orchestration GUI Surfaces

## 1. What this guide covers

This guide documents the architectural patterns and design decisions behind Hone's orchestration GUI work (PRs #55–#65, shipped 2026-05-10). It targets a contributor with Rust and SolidJS experience who wants to extend the `/plan`, `/budget`, `/permit`, or `/workflows` surfaces, or add a similar new slash command. You will learn five core patterns to mirror and nine key architectural trade-offs that shaped the implementation. For workspace-wide architecture (storage, multi-agent scheduling, security sandboxing), see `docs/architecture.md`.

---

## 2. The five core patterns

### Pattern 1: Actor + AgentCommand setter/getter pair

**What:** Mutations to long-lived `Agent` state (e.g. per-run budget override) flow through an async actor using a message-passing enum, with public getter methods on `AgentHandle`.

**Where:** 
- `crates/hone-core/src/agent.rs:2190-2198` — `AgentHandle::set_budget_limit()` (setter) and `AgentHandle::budget_limit()` (getter)
- `crates/hone-core/src/agent.rs:16-30` — `AgentCommand` enum cases
- `crates/hone-core/src/agent.rs:2019-2022` — actor loop arm handling `SetBudgetLimit`

**When to use:** Any new mutation to an in-flight `Agent` that must be thread-safe and survive across multiple turns. Budget overrides, policy changes, context injection, and cost accounting all use this pattern.

**Example:** 
```rust
// Caller (TUI main loop or daemon handler)
agent.set_budget_limit(Some(0.50)).await;

// Implementation (agent.rs)
pub async fn set_budget_limit(&self, limit: Option<f64>) {
    let (reply, rx) = oneshot::channel();
    let _ = self.tx.send(AgentCommand::SetBudgetLimit { limit, reply });
    let _ = rx.await;
}

pub async fn budget_limit(&self) -> Option<f64> {
    let (reply, rx) = oneshot::channel();
    let _ = self.tx.send(AgentCommand::BudgetLimit { reply });
    rx.await.unwrap_or(None)
}
```

---

### Pattern 2: Pending-on-App + drain in event loop

**What:** TUI commands that need async dispatch (network, subprocess, or expensive computation) set a `pending_*: Option<T>` field on `App`, then the main event loop drains that field and spawns the work outside the sync command handler.

**Where:**
- `crates/hone-tui/src/app.rs:220` — `pending_plan_prompt: Option<String>` field
- `crates/hone-tui/src/app.rs:1293-1294` — `take_pending_plan_prompt()` getter
- `bins/hone/src/main.rs:2866-2885` — drain point in main loop calling `dispatch_plan_decompose()`

**When to use:** A slash command (e.g. `/plan`, `/budget`) must block the UI for 100ms–2s while waiting for an LLM call or network round-trip. Set a pending field synchronously in `handle_command`, then drain it asynchronously in the next event loop iteration.

**Example from PR #63:**
```rust
// crates/hone-tui/src/app.rs — handle_command arm
Command::Plan(prompt) => {
    self.pending_plan_prompt = Some(prompt.to_string());
}

// bins/hone/src/main.rs — main loop
if let Some(prompt) = app.take_pending_plan_prompt() {
    match dispatch_plan_decompose(&config, &cwd, &prompt).await {
        Ok((plan, lead_cost, usage)) => {
            app.set_plan_preview(PlanPreviewState::from_decompose(
                prompt, plan, lead_cost,
            ));
        }
        Err(e) => app.push_status(&format!("Plan failed: {}", e)),
    }
}
```

**Avoid:** Calling `.await` or blocking I/O inside `handle_command` itself — that freezes the render loop and breaks TUI responsiveness.

---

### Pattern 3: Bridge signals (web UI)

**What:** SolidJS `createSignal` used as a global event bus to fire actions from deeply-nested components (e.g. a `<DryRunCard>` button) that are consumed by ancestor components (e.g. `<ChatPanel>`).

**Where:**
- `apps/webui/src/lib/orchestrator.ts:130-143` — `setPendingPlanAction()` / `takePendingPlanAction()` signal pair
- `apps/webui/src/components/WorkerProgressPanel.tsx:78-95` — `<DryRunCard>` component dispatching plan actions
- `apps/webui/src/components/ChatPanel.tsx:363-380` — `<ChatPanel>` draining actions in a `createEffect`

**When to use:** A child component needs to fire an action that a distant ancestor must handle, and prop-drilling the handler would traverse >2 levels. Bridge signals avoid the prop-drilling tax and keep the call site readable.

**Example from PR #57:**
```typescript
// apps/webui/src/lib/orchestrator.ts — define bridge
const [pendingPlanAction, setPendingPlanActionInternal] = 
    createSignal<PlanAction | null>(null);

export function setPendingPlanAction(action: PlanAction | null): void {
    setPendingPlanActionInternal(action);
}

export function takePendingPlanAction(): PlanAction | null {
    const v = pendingPlanAction();
    if (v) setPendingPlanActionInternal(null);
    return v;
}

// apps/webui/src/components/DryRunCard.tsx — dispatch
const onRun = () => {
    setPendingPlanAction({ kind: 'run', preview: dryRun() });
    clearDryRunPreview();
};

// apps/webui/src/components/ChatPanel.tsx — consume
createEffect(() => {
    const action = takePendingPlanAction();
    if (action?.kind === 'run') {
        sendOrchestrate(action.preview.goal, { dryRun: false });
    }
});
```

---

### Pattern 4: Test injection seam

**What:** Provide constructor or builder methods that redirect thread-unsafe globals (like `dirs::config_dir()`) to test-controlled paths, enabling unit tests without mutating real `~/.config/hone/`.

**Where:**
- `crates/hone-tui/src/app.rs:AppState` — `with_recipes_user_dir(PathBuf)` builder method
- `docs/plans/orchestration-gui-tdd-2026-05.md:73-79` — "Critical: redirecting `dirs::config_dir` in tests" design decision

**When to use:** Any handler that touches `dirs::config_dir()`, filesystem, network, or other globals that are not thread-safe or repeatable. The pattern is: add a `with_*` builder method that accepts a test-supplied override, store it as an `Option<T>` field, and check the override first before calling the global function.

**Example:**
```rust
// Handler code
let user_recipes_dir = self.recipes_user_dir.clone()
    .or_else(|| dirs::config_dir().map(|p| p.join("hone/recipes")))
    .unwrap_or_default();

// Test setup
#[test]
fn test_load_recipes() {
    let tmpdir = TempDir::new().unwrap();
    let mut app = AppState::default()
        .with_recipes_user_dir(tmpdir.path().to_path_buf());
    // Now app.load_recipes() uses tmpdir, not real ~/.config/hone
}
```

---

### Pattern 5: Cost-formula reuse via shared constant

**What:** A heuristic that both TUI and web UI clients must agree on (e.g. per-worker cost estimate) lives in a single Rust constant in `hone_core`, and is duplicated as a TypeScript constant in the web UI. This is acceptable because it is a single scalar, not a struct.

**Where:**
- `crates/hone-core/src/lib.rs:89` — `pub const DEFAULT_WORKER_COST_USD: f64 = 0.05;`
- `apps/webui/src/lib/orchestrator.ts` — mirrored as `const DEFAULT_WORKER_COST_USD = 0.05;` (searched by PR #55–#61)

**When to use:** Any heuristic or magic number that both clients need to compute identically (cost estimates, color thresholds, token budgets). For complex shared logic (plan-tree building, approval state machines), place it in `hone-core` or `hone-proto` and call it from both clients via JSON. See § 3.4 for why a shared `hone-orchestration-view` crate is not the answer.

**Example:**
```rust
// TUI computation (bins/hone/src/main.rs)
let projected_cost = lead_cost_usd + subtask_count as f64 * DEFAULT_WORKER_COST_USD;

// Web UI computation (apps/webui/src/components/WorkerProgressPanel.tsx)
const projectedCost = () => 
    leadCost() + workerCount() * DEFAULT_WORKER_COST_USD;
```

---

## 3. Architectural decisions (ADR-style)

### Decision 1: `AgentEvent::Done.plan` is `Option<serde_json::Value>`, not `Option<TaskPlan>`

**Decision:** The `plan` field in the `Done` event is untyped JSON, not a Rust struct.

**Why:** `hone-proto` depends only on `serde` and `serde_json` to remain a thin wire protocol. Adding a dependency on `hone-core` (which already depends on `hone-proto`) would invert the dependency direction and couple the protocol layer to core business logic. Untyped JSON is the right serialization boundary.

**Alternatives considered:**
- (a) Duplicate a `PlanSummary` struct in `hone-proto` — adds maintenance burden and schema sync risk.
- (b) Add `hone-core` as a dependency of `hone-proto` — breaks the dependency inversion principle; would create a circular dependency.

**When to revisit:** If a third client (Vim plugin, Slackbot, etc.) needs typed access to `TaskPlan` and duplication costs exceed the dependency-inversion gain, extract a minimal `TaskPlan` wrapper in `hone-proto` and make `hone-core::TaskPlan` a re-export of it.

---

### Decision 2: `?dry_run=true` halts after `PlanCreated`, not after `ToolCallStart`

**Decision:** The `/orchestrate?dry_run=true` endpoint emits `model_info` + `plan_created` + `budget_progress` + `done`, then halts. It does NOT emit `worker_started`, `tool_call_start`, or cost-per-step breakdowns.

**Why:** The `decompose()` call (which runs the lead LLM once) is already a sunk cost we pay upfront. Halting immediately after gives accurate subtask count and decomposition for zero extra cost. Halting later (after tool-call simulation) would cost ~3× more (multiple LLM calls per worker) for a per-step cost estimate that is only marginally more accurate on small plans (≤ 4 workers).

**Alternatives considered:**
- Simulate tool calls with the LLM to get per-step cost estimates — too expensive for a preview, only justifies itself in a 10+ worker plan.
- Skip decomposition entirely and return an empty plan — loses the value of the preview (users see nothing until execution).

**When to revisit:** If users report that projected cost is consistently wrong by >2× on real workloads, measure the cost-accuracy gain from per-step simulation and weigh it against the 3× cost overhead.

---

### Decision 3: `?dry_run=true` query param, not separate `POST /plan` route

**Decision:** Dry-run is a query parameter on the existing `POST /orchestrate` endpoint, not a new route.

**Why:** Reuses the entire SSE pipeline (event framing, provenance tee, error handling, auth, rate limiting, observability). A separate `POST /plan` route would require duplicating ~80 LoC of handler boilerplate and test infrastructure. The shared handler scales.

**Alternatives considered:**
- `POST /plan` endpoint — cleaner URI semantics, but duplicates implementation and test infrastructure.
- New route with shared helper function — still requires separate handler registration and tests; marginal savings.

**When to revisit:** If dry-run grows divergent semantics (e.g. different authentication, different rate limits, different event stream shape), the shared handler will become a tangle of `if is_dry_run` branches. At that point, extract `POST /plan` as a separate route with a shared `decompose_only()` helper.

---

### Decision 4: No shared `hone-orchestration-view` crate between TUI and web UI

**Decision:** Do not extract a shared presentation layer. Share at the protocol layer (`hone_proto::AgentEvent` JSON) and render independently.

**Why:** SolidJS signals (event-driven, reactive) and Ratatui immediate-mode rendering are fundamentally different paradigms. A shared view crate that compiles to both would be a thin façade with no real savings — you cannot reuse React components in Ratatui and vice versa. Both clients already consume the same `AgentEvent` protocol; that is the right level of sharing.

**What to share instead:**
- Pure **formatters** like `format_cost(usd: f64) -> String` belong in `hone-core` (~5 LoC). Both clients call it to ensure rounding parity.
- The **plan-tree builder** (worker → parent_task_id → tree structure) belongs in `hone-core` where the orchestrator lives, and clients consume the output as JSON.
- The **approval state machine** (`StepAction` enum) is already shared in `hone-proto`.

**Alternatives considered:**
- Shared WASM crate consumed by both — compilation overhead, still requires rendering translation layer on TUI side, no net savings.

**When to revisit:** If a third client lands (iOS shell, Slack bot, etc.), the duplication tax flips and a `hone-orchestration-view` crate becomes justified. At two clients, it is YAGNI.

---

### Decision 5: Per-run governance only, no persistence

**Decision:** `/budget` and `/permit` overrides affect only the current turn. They are not persisted to `~/.config/hone/config.toml` or team policy.

**Why:** Defers the (real, but separate) question of how persistent governance interacts with team policies, project-level configs, and role-based access control. Per-run overrides are a clear, testable contract: they apply once and reset. This unblocks the feature for v1.

**Alternatives considered:**
- Persist to `config.toml` — requires designing config merging (which overrides? which scope wins?) and is a 2-week project.
- Persist to a new `.hone/persistent-governance.toml` — splits governance state across two files; confusing.

**When to revisit:** When users ask "I want this budget to apply to every run of this workflow, not just one." At that point, design a `workflow.permissions` section in the saved recipe YAML and a `--apply-workflow-permissions` flag.

---

### Decision 6: `apply_to_budget` and `apply_to_policy` are separate helpers, not one `apply(config: &mut AgentConfig)`

**Decision:** Governance overrides are applied via focused helper methods, not a single `apply()` that takes a full config struct.

**Why:** **Testability.** Constructing a full `AgentConfig` in a unit test requires ~20 fields, most irrelevant to the test. Separate helpers let tests construct just the `Option<f64>` budget or `ToolApprovalPolicy` being tested. Example: `test_budget_override_caps_spend` constructs only `opt: Option<f64>`, not an entire agent config.

**Where:** `crates/hone-tui/src/governance.rs:15-30` — `apply_to_budget()` and `apply_to_policy()` methods.

**Alternatives considered:**
- Single `apply(config)` method — forces tests to construct 20 unused fields or use a builder (which then needs its own tests).

**When to revisit:** If the helper count grows past 3 (currently 2) and the call site at `bins/hone/src/main.rs` gets noisy with many independent `apply_*()` calls, unify them into a single `apply_all()` method.

---

### Decision 7: TUI dispatches `/plan` to `Orchestrator::decompose` in-process, web UI POSTs to `?dry_run=true`

**Decision:** TUI calls `dispatch_plan_decompose()` which instantiates a local `Orchestrator` and runs `decompose()` synchronously. Web UI POSTs to `/orchestrate?dry_run=true` on the daemon.

**Why:**
- **TUI:** Is a single-process binary that already owns an `Orchestrator` instance (for the normal agent loop). Reusing it in-process is zero-cost. Making it round-trip through the daemon would add a hard dependency on `honed` running.
- **Web UI:** Must use the daemon — there is no alternative. Browser cannot instantiate Rust objects or run LLM calls directly.

**Where:** 
- `bins/hone/src/main.rs:1173` — `dispatch_plan_decompose()` implementation.
- `crates/hone-server/src/orchestration.rs:251-322` — dry-run branch of `orchestrate_handler()`.

**When to revisit:** If the TUI gains a "remote daemon" mode (where it connects to a `honed` instance rather than owning an agent loop), consolidate both paths to use the daemon route.

---

### Decision 8: TDD with separate test-architect and implementer agents

**Decision:** The test-architect agent writes failing tests first (red phase). A different implementer agent makes them green (green phase). A third code-reviewer agent reviews the implementation.

**Why:** Prevents the implementer from unconsciously writing tests that match their implementation rather than testing the actual contract. When the same agent writes tests and code, it will bend the tests to match the implementation, defeating the purpose of tests as a specification. Separation forces independent thinking.

**Validation:** Code-reviewer caught 4 real bugs across PRs #55, #56, #58, #62:
- Zeroed `TokenUsage` in mock setup (test was not checking token accuracy).
- NaN/infinity parser (implementation silently accepted invalid input; test was too permissive).
- Duplicate re-export causing clippy warnings (test did not verify export uniqueness).
- `&Option<f64>` getter type error (test assumed the wrong type signature).

**Process invariant:** Do not skip this even for "small" features. It is documented in `CLAUDE.md` and is a team standard.

---

### Decision 9: Frontend has no vitest yet

**Decision:** Web UI test coverage is currently `tsc --noEmit` (type checking) + `vite build` (bundling), with no unit tests. Logic-heavy modules are hand-tested.

**Why:** Bootstrapping vitest for a single PR would inflate slice scope and delay shipping by 3–5 days. The 1–2 logic-heavy functions in the orchestration work (e.g. `format_cost()`) are trivial enough that type checking + manual testing is sufficient for now.

**When to revisit:** When a complex module lands (e.g. multi-step recipe builder with validation, cost-trend calculation engine, or policy conflict resolution) that genuinely needs unit-level coverage and is hard to test manually. Estimate: when frontend logic exceeds 500 lines in a single file.

---

## 4. Where to add a new slash command (recipe)

Using `/plan` as the canonical example, here is the 5-step flow to add a new surface:

### Step 1: Add variant to `Command` enum and `COMMAND_NAMES`

**File:** `crates/hone-tui/src/commands.rs`

```rust
pub enum Command {
    Plan(String),
    Budget(f64),
    Permit(String),
    Workflows,
    MyNewCommand(Args),  // <-- Add here
}

const COMMAND_NAMES: &[&str] = &[
    "plan", "budget", "permit", "workflows", 
    "mynew",  // <-- Add here
];
```

**Citation:** `/plan` variant added in PR #63, `/budget` and `/permit` in PR #58. PR#59 added `/workflows`.

---

### Step 2: Wire `App::handle_command` to set a pending field

**File:** `crates/hone-tui/src/app.rs`

```rust
pub(crate) pending_my_new_thing: Option<MyNewState>,

// In handle_command arm:
Command::MyNewCommand(args) => {
    self.pending_my_new_thing = Some(MyNewState::from_args(args));
}

pub fn take_pending_my_new_thing(&mut self) -> Option<MyNewState> {
    self.pending_my_new_thing.take()
}
```

**Citation:** `/plan` wired at `app.rs:1836` (PR #63), `/budget` and `/permit` at `app.rs` governance handling (PR #58, #62).

---

### Step 3: Drain `take_pending_*()` in bins/hone main loop

**File:** `bins/hone/src/main.rs` (event loop, right after `app.handle_key(key)`)

```rust
if let Some(state) = app.take_pending_my_new_thing() {
    match handle_my_new_thing(&config, state).await {
        Ok(result) => {
            app.set_preview_state(result);
        }
        Err(e) => app.push_status(&format!("Error: {}", e)),
    }
}
```

**Citation:** Plan decomposition drained at `bins/hone/src/main.rs:2866-2885` (PR #63). Governance drained at `bins/hone/src/main.rs:2980-2989` (PR #62).

---

### Step 4: Mirror in web UI (SolidJS)

**File:** `apps/webui/src/components/ChatPanel.tsx` (or new panel if it's a full page)

```typescript
const onSlashCommand = (cmd: string, args: string) => {
    if (cmd === 'mynew') {
        // Intercept before generic sendOrchestrate
        handleMyNewCommand(args);
        return; // Prevent default chat send
    }
    // ... other commands ...
};

// Define in lib/orchestrator.ts if it needs a bridge signal:
const [myNewState, setMyNewState] = createSignal(null);

export function setPendingMyNewState(s) { setMyNewState(s); }
export function takePendingMyNewState() {
    const v = myNewState();
    if (v) setMyNewState(null);
    return v;
}
```

**Citation:** `/plan` interception at `apps/webui/src/components/ChatPanel.tsx:363-380` (PR #61). `/workflows` added full drawer panel at `apps/webui/src/components/WorkflowsPanel.tsx` (PR #59).

---

### Step 5: Document in getting-started and tutorials

**Files:**
- `docs/getting-started.md` § "Commands in interactive mode" — add `/mynew <args>` description.
- `docs/tutorials/getting-started.md` — add walkthrough example (if user-facing).
- `docs/tutorials/orchestration-gui.md` — add if it is an orchestration-related surface.

**Citation:** `/plan`, `/budget`, `/permit` added to `docs/getting-started.md:§8` and documented in `docs/tutorials/orchestration-gui.md` (PR #64).

---

## 5. Common pitfalls

- **Stash mishaps during multi-slice work** — Slice 1 had a stash double-pop incident that contaminated the working tree with 119 unrelated files from concurrent work. Lesson: prefer worktrees over stashes when juggling parallel slices. If you must stash, `git stash list` before popping to confirm you're on the right one.

- **Implementer scope creep** — Sub-slice 4.5a's first attempt removed `mode` from `AgentConfig` across 9 files when only 4 setters were asked for. Lesson: implementer's PR description must be checked against the test-architect's specification before commit. If the PR touches files outside the test scope, query why.

- **Test files have inner-attribute order constraints** — `///` doc comments before `#![allow(...)]` is illegal Rust syntax. Use `//!` for file-level docs above attributes. Slices 2, 4, and 4.5a all hit this. Lesson: run `cargo build` on test files before committing.

- **Auto-closed PRs when base branch is deleted** — Slice 1 merge deleted `feat/orchestrate-dry-run`, auto-closing #56 and #57; reopened as #60 and #61 by re-pushing the same branches against `main`. Lesson: do not stack PRs (`git rebase -i --onto=other-branch`) unless you are prepared to retarget on base merge or merge the base branch into the dependent slice before closing it.

- **`cargo fmt -p PACKAGE` reformats whole packages** — Can surface a 700-file diff if the package has pre-existing format debt. Lesson: use `rustfmt --edition 2024 --config max_width=100,use_field_init_shorthand=true <file>` on only the files you touched. Alternatively, run `cargo fmt --all` once at the start of the sprint to clear baseline debt, then format touched files only thereafter.

- **Signal cleanup in SolidJS** — Creating a signal inside a component without cleanup can cause memory leaks if the signal is global and the component unmounts. Lesson: define bridge signals at module scope, not in component bodies. See Pattern 3 for correct placement.

- **Dry-run query param case sensitivity** — `?dry_run=true` is the lowercase standard used across the codebase. Using `?dryRun=true` or `?dry-run=true` will not be parsed. Lesson: grep the codebase for the exact parameter name before using it.

---

## 6. See also

- `docs/architecture/orchestration-gui-flow.md` — Sequence diagrams for TUI (in-process) and web UI (SSE) paths, governance override flow, cost formula.
- `docs/plans/orchestration-gui-tdd-2026-05.md` — The 5-slice implementation roadmap, TDD agent assignments, verification gates.
- `docs/research/gui-for-non-cli-users-2026-05.md` — Audience analysis (Maya persona), six high-leverage surfaces, vocabulary mapping.
- `docs/research/tui-orchestration-integration-2026-05.md` — TUI portability assessment, per-surface design trade-offs, why no shared view crate.
- `docs/tutorials/orchestration-gui.md` — User-facing tutorial for `/plan`, `/budget`, `/permit`, `/workflows`.
- `docs/changelog.md` (2026-05-10 entry) — Complete feature list and file citations.
