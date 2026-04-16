# Phase 1 / P0 Implementation Notes

These notes capture design decisions made during the Phase 1 implementation
where the spec was ambiguous or a trade-off was required.

---

## TurnLedger (Step 1)

### Decision: Extend AgentEvent instead of parsing conversation log

The design spec says "ToolCallStart and ToolResult events carry the tool name
and arguments". The existing `AgentEvent::ToolCallStart` did not carry `args`
and `ToolCallEnd` did not carry `duration_ms`. We extended both structs with
`#[serde(default)]` fields so the change is backward-compatible with any
existing JSON serialization (old logs simply get `args: null` and
`duration_ms: 0`).

Alternative considered: parse args from the egress log / audit.jsonl. Rejected
because it requires `--audit` to be active and introduces a file I/O dependency
in the hot path.

### Decision: FileOp deduplication is last-write-wins per path

When a file is both read and then written in the same turn, the ledger shows
the write operation (the more significant one). This matches the design spec
("last-operation wins the badge").

### Decision: Tool-to-FileOp mapping is heuristic, not exact

`tool_op()` classifies by tool name substring. This covers all known native
tools. Unknown tools (e.g. custom MCP tools) return `None` and are tracked by
name only in `tools_invoked`, not in `files_touched`.

---

## Agent Rail (Step 2)

### Decision: `AgentRailState.scroll` is not persisted across turns

The spec says "Remembers state per session" for the rail toggle. The scroll
offset within the rail is reset each time a new turn completes (when the
content changes). The visibility toggle (`agent_rail_visible`) persists for
the session.

### Decision: Rail is always 18 columns

The spec says 16-col minimum at Standard tier, scaling to 24 at Full tier.
For simplicity, we use a fixed `RAIL_WIDTH = 18` that works across both tiers.
Width scaling is deferred to the follow-up pass.

### Decision: `render_agent_rail` uses a cloned `AgentRailState` from `app.rail_scroll()`

A proper stateful widget would require `App` to own an `AgentRailState` and
pass a mutable reference to `render_stateful_widget`. For now we construct a
temporary state from `app.rail_scroll()` each render tick. Full scroll support
(j/k in the rail) requires wiring a focus mode, which is deferred.

---

## Accessibility Flags (Step 3)

### Decision: `ascii_only` implies `no_color` box-drawing

When `ascii_only = true`, `box_chars()` returns ASCII. When `no_color = true`,
the same ASCII set is returned. This means color themes with `ascii_only` still
show color in text but use ASCII borders — consistent with the spec.

### Decision: `spinner_glyph(tick)` API on Theme

The spec says "spinners always show a static glyph" under `reduced_motion`.
We put this logic on `Theme` so every widget has a single call site. Existing
animation code in `animation.rs` can call `theme.spinner_glyph(animation.tick)`
on the next pass.

---

## Session Reports (Step 4)

### Decision: Non-TUI mode reports are deferred

The spec calls for reports after `run_recipe`, `run_orchestrate`,
`run_ensemble`, and `run_one_shot`. These code paths don't have an `App` with
`TurnLedger` data — implementing full ledger tracking there would require
threading a ledger collector through the event loops of all four paths.

**Decision**: For this pass, only the interactive TUI path generates a full
report on `/quit`. Non-TUI paths produce no report file. The design doc
explicitly allows partial reports ("a subset of the full report"). This is
marked with `partial: true` in the report header.

### Decision: Timestamp format is UNIX epoch seconds

The design spec shows ISO8601 timestamps in filenames. To avoid pulling in
`chrono` (not in the workspace deps), we use UNIX epoch seconds as the
timestamp in the filename for now. The report header uses the epoch seconds
string. This gives sortable, collision-resistant names. A follow-up can replace
this with proper RFC3339 formatting once `chrono` or `time` is added to the CLI
crate deps.

### Decision: `tokens_out` and `cache_read_tokens` not separately tracked

`StatusInfo` only accumulates `token_count` (total). Splitting into in/out/cache
requires per-turn `TokenUsage` accumulation. The report sets `tokens_out = 0`
and `cache_read_tokens = 0` until a follow-up adds that tracking.

---

## `hone reports` CLI (Step 5)

### Decision: Hand-rolled LCS diff instead of `similar` crate

The spec mentions "use the `similar` crate or a handwritten line-diff". The
`similar` crate is not in the workspace deps. For reports (< 200 lines), an
O(n²) LCS is fast enough. Added the hand-rolled version to keep zero new deps.

### Decision: Diff output is simplified (no context windowing)

The `line_diff` function emits a raw LCS-derived op list without the full 3-line
context window logic of unified diff. For report diffs (structured markdown
tables), showing all changes is clearer than a windowed hunk. Context windowing
is left as an enhancement.

---

## Rotation (Step 6)

Retention is implemented in `report::enforce_retention`. It deletes the oldest
`.md` files (and their `.md.json` sidecars) when the count exceeds the limit.

**Decision**: Retention is based on file modification time sorted oldest-first.
On fast machines, multiple reports written in the same second will have
identical `mtime`. In that edge case, alphabetical order breaks ties (the
platform `read_dir` order is stable within a second on most filesystems).

The `--keep-all` flag is on the `reports list` subcommand as requested. The
retention enforcement on write always applies (no CLI flag to disable it per
the spec: "retention is automatic").

---

## Out-of-scope items (explicitly not implemented)

- Inspect overlay (`Ctrl+I`) — P1
- Worker Board / pill strip — P1/P2
- `Ctrl+P` tool call inline linking — P1
- Customization presets — deferred
- Plugin ecosystem — §13 is explicitly out of scope
- `/share-report` flow — deferred
- WCAG contrast check at startup — P2
- `reduced_motion` wiring into existing animation.rs spinner — the
  `Theme::spinner_glyph()` API is added; callers updated in follow-up pass
