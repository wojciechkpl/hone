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

**Why:** The terminal-banner.png brand asset shows an existing vision. The spec makes that vision implementable from Ratatui primitives without sixel/image rendering.

**How to apply:** When discussing TUI implementation, reference the spec section numbers. Implementation priority order is in Section 20.
