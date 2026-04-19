---
name: Hone TUI Design Context
description: Core brand, palette, and architecture decisions for the Hone TUI design spec
type: project
---

Hone is a Rust-based AI coding agent with a Ratatui TUI. The tagline is "SHARPEN YOUR CODE". The brand mark is a stepped/pixelated double-arrow pointing right — rendered in pure ASCII as `>=>` (compact) or a 9-row stacked mark for the splash screen.

**Accent palette (signature):** hot pink #F75077 -> rose #E8304A -> deep crimson #7B1D35. This gradient is the sole identity color. It appears on exactly one element at a time in any given view.

**Background:** near-black navy #0E1117. Surface layers step up in luminance: raised #161B24, overlay #1E2535.

**Design spec location:** `/Users/wojciechkowalinski/Sync/work/hone/docs/tui-design-spec.md`

**Key architectural decisions:**
- The `Theme` struct needs a full expansion from 6 fields to ~35 tokens (see spec Section 18.1)
- `AnimationState` must be added to `App` — tick rate 100ms
- `StatusInfo` needs `budget_usd: Option<f64>` and `session_start: Instant`
- The conversation viewport must become a `ConversationWidget` implementing `StatefulWidget`
- Three layout modes: single-pane (default), split-pane (`Ctrl+P`), tool panel (`/panel`)

**Status bar design (see `priv/status-bar-design.md`):**
- `sparkline_spans` must be refactored to `bar_spans(fraction, width, warn, crit, theme)` and reused for both context window and budget bars — unified visual grammar
- `StatusInfo` needs: `session_name: Option<String>`, `phase: TaskPhase`, `provider_health: ProviderHealth`, `turn_count: u32`, `workers_completed: usize`, `workers_total: usize`
- Three layout tiers by terminal width: Minimal (<100), Standard (100–139), Power (140+)
- Section elision order when width is tight: turns → session → phase → budget → elapsed → health → tool (model/cost/context never elided)
- Tool icons: ✦ read, ✎ write, ❯ exec/bash, ? search — prepend to tool name, no extra cols (1 char)
- `Ctrl+B` = compact toggle, `Ctrl+S` = section preset cycle (All → Essential → Minimal)
- NO_COLOR: bars use ASCII `|` chars, spinner uses `| / - \`, icons use text (`read`/`write`/`exec`)
- Phase detection: from tool name on `ToolCallStart` — read→Exploration, write→Editing, bash+test→Verification
- Priority implementation order: context bar, bar_spans refactor, TaskPhase, ProviderHealth badge, tool icons, width-adaptive layout, StatusBarConfig TOML, keyboard shortcuts, NO_COLOR, workers bar

**Agent observability design (see `priv/ux/agent-observability-design.md`):**
- Three disclosure tiers: Tier 0 = status bar (always), Tier 1 = Agent Rail (Ctrl+R, auto-shown at Standard+), Tier 2 = Inspect overlay (Ctrl+I)
- Agent Rail is a new 16–20 col strip RIGHT of conversation — NOT the existing SplitHorizontal file preview
- Rail has three sections: [FILES THIS TURN] accumulates from TurnLedger, [CONTEXT INJECTED] from new AgentEvent::ContextInjected, [MEMORY] from new AgentEvent::MemoryRecalled
- Inspect overlay has 5 tabs: Files / Skeleton / Memory / Compression / Trace (Tab 5 requires --audit)
- Multi-agent: pill strip in status bar center zone; Ctrl+W opens Worker Board overlay with DAG and per-worker table
- New keybinds: Ctrl+R (rail), Ctrl+I (inspect), Ctrl+W (workers), Ctrl+P (tool→preview link)
- New AgentEvent variants needed: FileAccessed, ContextInjected, MemoryRecalled — must be added to hone_proto
- Rollout: Phase 1 = Rail (P0, ~2 weeks), Phase 2 = Inspect + tool linking (P1, ~3 weeks), Phase 3 = Multi-agent board (P1, ~3 weeks)
- Operation color grammar: read = accent_hot (pink), write = teal #2DD4BF, exec = amber #FBBF24 — always dual-encoded with symbols for colorblind safety
- Section 8 (Customization): config split = user-global ~/.config/hone/config.toml (ergonomics) vs project .hone/config.toml (policy — committable). 4 layout presets: minimal/debugger/observability/parallel-work. Accessibility opt-outs: reduced_motion (P0), ascii_only (P0), high_contrast (P1). Op colors accept hex or xterm-256 int. Keybinding remap is P2. /configure in-TUI editor is P2.
- Section 9 (Reports): reports are a ROLL-UP of audit.jsonl traces — not a parallel artifact. trace_id field links report.json → hone trace show. Two surfaces: stdout terminal summary (<30 lines) + .hone/reports/<ts>-<id>.md. JSON sidecar at report.json (schema v1). Retention default: last 100 OR last 30 days. CLI: hone reports list/show/diff/share/purge. /share-report flow: redact absolute paths → $PROJECT, redact env var values, show preview before action.
- Section 13 (Plugin & Theme Ecosystem): oh-my-zsh-inspired ecosystem layer. Key decisions: (1) ~/.config/hone/custom/ is sacrosanct — never touched by updates. (2) V1 plugins are pure data (YAML/TOML) — no executable code, no WASM yet. (3) Permission manifest in every plugin.toml — network/shell_exec forbidden in V1. (4) Trust tiers: verified (signed) / community (registry) / unverified (git URL) / local (custom/). (5) Async widget polling with stale indicator (V2) — matches Starship model. Plugin kinds ranked by V1 priority: recipe-pack > keymap > persona-pack > theme > widget > report-section > slash-command > mcp-bundle; hooks and formatters deferred to V2. Registry at github.com/hone-sh/registry — registry.toml index, shallow clone on hone init. 5 bundled themes: crimson (default/brand), neon (cyberpunk), chalk (light), ghost (minimal), matrix (monochrome). Ecosystem rollout: Phase 1 = themes + recipe-packs (2026-07), Phase 2 = widgets + report-sections + keymaps + persona-packs (2026-08), Phase 3 = hooks + WASM sandbox + signing + full registry (2026-10).

**Advanced Inspection design (see `priv/ux/advanced-inspection-design.md`):**
- Levels 3–5 of the progressive disclosure stack (Levels 0–2 are in agent-observability-design.md)
- Level 3 (Ctrl+M): Prompt Mirror — read-only overlay of assembled context before dispatch. Sections: [SYSTEM], [CODE INDEX — strategy], [MEMORY — N recalled], [HISTORY — BM25 reordered], [THIS TURN]. Fill bar at top. Context Steering sub-feature: Ctrl+E per-row exclusion for next turn only; Ctrl+Enter dispatches with exclusions; Esc cancels. Requires new `AgentEvent::PromptReady { segments: Vec<PromptSegment>, total_tokens, context_limit }` from ContextManager. Only visible at Standard+ width tiers.
- Level 4 (Ctrl+A): Session Anatomy — two sub-views toggled by Tab. Sub-view A (Phase Timeline): per-turn table with phase, tool summary, tokens, cost, elapsed, outcome; phase transition separator rows; Enter jumps conversation to turn; / filter by phase/outcome/tools. Sub-view B (Memory Graph): all session memories with recall count, weight, age; e to annotate, d to delete with confirmation; memory health bar. Data from `completed_turns: Vec<TurnLedger>` + `MemoryStore::list()`. Needs 4 new `TurnLedger` fields: total_tokens, cost_usd, elapsed_ms, had_error.
- Level 5 (--step / /step): Agent Debugger — step card overlays bottom 30% before each tool dispatch. Actions: y=approve, n=skip (synthetic result), e=edit args, m=modify patch (edit_file only), q=abort turn. Channel protocol: `ToolStepFilter` trait wraps dispatcher; per-call `oneshot::channel` with `StepDecision`; agent awaits reply before executing. [STEP] badge in status bar when active.
- New render modules needed: `prompt_mirror.rs`, `session_anatomy.rs`, `step_card.rs`
- P1 (no backend changes): Session Anatomy P1 (Timeline + Memory list), step mode activation flag. P2 (needs new events/channel): PromptReady emission, Context Steering dispatch, ToolStepFilter channel, patch editor.

**Why:** The terminal-banner.png brand asset shows an existing vision. The spec makes that vision implementable from Ratatui primitives without sixel/image rendering.

**How to apply:** When discussing TUI implementation, reference the spec section numbers. Implementation priority order is in Section 20. For observability features (Levels 0–2), reference `priv/ux/agent-observability-design.md`. For Levels 3–5 features (Prompt Mirror, Session Anatomy, Agent Debugger), reference `priv/ux/advanced-inspection-design.md`. For ecosystem/plugin questions, reference section 13 of agent-observability-design.md.
