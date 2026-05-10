# Orchestration GUI: `/plan`, `/budget`, `/permit`, `/workflows`

A short tutorial covering the four slash commands that make orchestrated
runs safe and repeatable for non-CLI users — the same surfaces are
wired in both the TUI (`hone`) and the web UI (`honed` at `/ui`).

## TL;DR

| Command | What it does | Where |
|---|---|---|
| `/plan <prompt>` | Preview the orchestrator's decomposed plan + cost estimate **before** any worker runs. Pick Run / Edit / Cancel. | TUI + web UI |
| `/budget <usd>` | Cap the next run's cost ceiling. Drains after one turn. | TUI |
| `/permit <tool>` | Pre-approve a tool for the next run (skip the AskRisky pause for it). Drains after one turn. | TUI |
| `/workflows` | Open the **Workflows drawer**: list saved YAML recipes, fill in `{{var}}` template fields, click Run. | web UI |

All four are intentionally **per-run** — they apply to the very next
turn and then drain. Nothing persists to your config.

---

## 1. The plan-preview gate (`/plan`)

The orchestrator's *decompose* step is one LLM call: it turns your
prompt into a `TaskPlan` (a goal plus N subtasks, each routed to a
specialist agent). `/plan` runs only that step and shows you the
result before paying for any worker.

```
> /plan refactor the auth module to extract session state

  ╭──────────────────── Plan Preview ────────────────────╮
  │   goal  Refactor the auth module                     │
  │   cost  paid $0.0120  ·  projected ~$0.1620         │
  │                                                      │
  │   subtasks (3)                                       │
  │   · rust-expert     [1] Extract session state struct │
  │   · code-reviewer   [2] Audit for race conditions    │
  │   · test-architect  [3] Add integration tests        │
  │                                                      │
  │   r/Enter run    e edit    c/Esc cancel              │
  ╰──────────────────────────────────────────────────────╯
```

Three keys:

- **r / Enter** — run the plan. The prompt replays through the normal
  turn dispatch; each subtask spawns the right specialist.
- **e** — restore the prompt to the input bar. Tweak it and submit
  again (or `/plan` again).
- **c / Esc** — cancel. Costs you only the lead-LLM call you already
  paid.

The cost line shows two figures:

- **paid** — the real lead-LLM cost from the decompose call.
- **projected** — `paid + N × DEFAULT_WORKER_COST_USD`, where
  `DEFAULT_WORKER_COST_USD = $0.05` lives in `hone-core`. It's a
  heuristic, not a guarantee — set a hard ceiling with `/budget` if
  you need one (see § 2).

The **same** preview is available in the web UI at `/ui` — type
`/plan <prompt>` in the chat input and a `<DryRunCard>` overlay
appears with the same Run / Edit / Cancel buttons. Both surfaces
back onto `POST /orchestrate?dry_run=true`.

---

## 2. Per-run cost ceiling (`/budget`)

`/budget <usd>` sets a hard cap on the next turn's spend. The cap
mutates the running `AgentHandle`'s `budget_limit` field so the
runtime budget check (`Cost::check_budget`) trips with
`AgentError::BudgetExceeded` before any further LLM call.

```
> /budget 0.20
> /plan refactor x
... overlay shows projected cost of $0.18 — under budget, safe to Run.
> r
```

Validation rules:

- Positive only — `/budget 0`, `/budget -1` are rejected.
- Finite only — `/budget inf`, `/budget nan` are rejected.
- Integers and decimals both work — `/budget 5` is `Some(5.0)`.

`/budget` drains after one turn. To set a permanent ceiling, edit
your config TOML directly.

---

## 3. Per-run tool pre-approval (`/permit`)

`/permit <tool>` adds a tool name to the next run's auto-allowlist so
the AskRisky approval pause doesn't trigger for it. Useful when you
already know a recipe needs `shell` or `edit_file` and don't want to
press `y` ten times.

```
> /permit shell
> /permit edit_file
> /plan apply the migration plan from PR #42
... overlay shows projected cost; Run.
... shell + edit_file fire without pausing.
```

Notes:

- Idempotent — `/permit shell` twice is the same as once.
- Tool names can be dotted (`/permit shell.run`).
- No-op when the active policy is `Auto` (everything's already
  allowed) or `AlwaysAsk` (no allowlist, every tool prompts).

---

## 4. Saved workflows (`/workflows` — web UI)

Recipes are YAML files under three layers, in priority order:

1. `<workspace_root>/recipes/`
2. `~/.config/hone/recipes/`
3. `<project_root>/.hone/recipes/` (project overrides)

The web UI's **Workflows drawer** (topbar `workflows` button or
`/workflows` slash command) lists them as cards:

```
─────────────────────────────────────────
 [refactor-extract]  user · 3 steps
   Extract a function and add a regression test
─────────────────────────────────────────
 [tdd-feature]       builtin · 4 steps
   Red → Green → Refactor with named test author
─────────────────────────────────────────
 [security-audit]    project · 2 steps
   Run cargo audit + secret scan, summarise results
─────────────────────────────────────────
                                  [+ New]
```

Click a card to load detail. Each unique `{{var}}` placeholder in the
prompts becomes an input field. Click **Run** to compose the prompt
(every step concatenated with `## stepname` headers, vars
substituted) and dispatch it through the normal chat flow.

### Authoring a recipe from the browser

The drawer's **+ New** form is single-step authoring for v1: name,
description, one step, optional template vars.

It posts to `POST /recipes` (added in slice 5). The handler validates:

- Name matches `[a-zA-Z0-9_-]+` (no `/`, no `..`, no whitespace —
  path-traversal hardened).
- Steps non-empty; each step's name and prompt non-empty.
- Returns `409 Conflict` if a recipe of the same name already exists
  in the user dir.

The file lands at `~/.config/hone/recipes/<name>.yaml`.

For multi-step recipes, edit the YAML directly — the schema supports
arbitrary step counts, conditional `when:` predicates, and a `test_cmd`.
See [recipes.md](recipes.md) for the full grammar.

---

## Putting it all together

A typical "I'm not sure what this is going to do" workflow:

```
# Cap blast radius
> /budget 0.30
> /permit shell

# Preview before paying
> /plan add an end-to-end test for the new dry_run flag

# Overlay shows: 4 subtasks, projected ~$0.20. Looks fine.
> r

# Orchestration runs. Lands four worker results into one merged response.
# /budget + /permit drained — next turn is back to defaults.
```

If the same prompt comes up often, save it as a workflow:

```
# Open the web UI at http://127.0.0.1:3117/ui
# Click `workflows` → `+ New`
# Name: e2e-test, Step prompt: "add an end-to-end test for {{feature}}"
# Save.
# Next time: click the e2e-test card, fill `feature`, click Run.
```

---

## See also

- [Getting Started § 8.1](../getting-started.md#81-plan-preview-gate-plan)
  — same material in the main install doc.
- [Recipes](recipes.md) — full YAML schema, conditional steps, `test_cmd`.
- [`docs/research/gui-for-non-cli-users-2026-05.md`](../research/gui-for-non-cli-users-2026-05.md)
  — the audience analysis that motivated these surfaces.
- [`docs/plans/orchestration-gui-tdd-2026-05.md`](../plans/orchestration-gui-tdd-2026-05.md)
  — the slice-by-slice TDD implementation plan.
