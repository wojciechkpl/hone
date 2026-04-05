# Hone TUI Design Specification

Version: 1.0  
Date: 2026-04-05  
Status: Approved for implementation

---

## Table of Contents

1. [Brand Grounding](#1-brand-grounding)
2. [Color System](#2-color-system)
3. [Theme Presets](#3-theme-presets)
4. [Typography and Symbols](#4-typography-and-symbols)
5. [Layout Architecture](#5-layout-architecture)
6. [Startup Experience](#6-startup-experience)
7. [Conversation View](#7-conversation-view)
8. [Message Rendering](#8-message-rendering)
9. [Input Area](#9-input-area)
10. [Status Bar](#10-status-bar)
11. [Tool Call Panels](#11-tool-call-panels)
12. [Split-Pane Layout](#12-split-pane-layout)
13. [Help Overlay](#13-help-overlay)
14. [Animation and Motion](#14-animation-and-motion)
15. [Screen State Inventory](#15-screen-state-inventory)
16. [Accessibility and Degradation](#16-accessibility-and-degradation)
17. [Ratatui Widget Map](#17-ratatui-widget-map)
18. [Theme Data Structures](#18-theme-data-structures)
19. [Nielsen Heuristic Audit](#19-nielsen-heuristic-audit)
20. [Implementation Priority Order](#20-implementation-priority-order)

---

## 1. Brand Grounding

The Hone mark is a stepped, pixelated double-arrow pointing right — an abstraction of precision and forward motion. The color story runs from deep crimson (`#7B1D35`) through vivid rose (`#E8304A`) to hot pink (`#F75077`). The background is near-black navy (`#0E1117`). The tagline is "SHARPEN YOUR CODE".

These three facts drive every design decision in this spec:

- **Signature accent**: the hot-pink-to-crimson gradient is Hone's identity. It must appear on exactly one element at a time — the prompt sigil, the thinking indicator, or the active border. Never diluted across multiple elements simultaneously.
- **Dark-first**: the primary palette targets dark terminals. Light-terminal equivalents are defined separately in every token table.
- **Terminal-native**: nothing in this spec requires sixel graphics or image rendering. Every visual element is composed from Unicode, ANSI color, and box-drawing characters alone.

---

## 2. Color System

### 2.1 Design Tokens

All colors are listed with: role, dark-terminal value (truecolor hex), 256-color ANSI approximation, and light-terminal equivalent.

| Token | Role | Dark (hex) | ANSI-256 | Light (hex) | ANSI-256 |
|---|---|---|---|---|---|
| `accent-hot` | Primary accent, prompt sigil | `#F75077` | 204 | `#C0183A` | 160 |
| `accent-mid` | Secondary accent, borders | `#E8304A` | 197 | `#A8102E` | 124 |
| `accent-deep` | Muted accent, decorations | `#7B1D35` | 88 | `#6B0D25` | 88 |
| `surface-base` | Terminal background (assume) | `#0E1117` | 233 | `#F5F5F0` | 255 |
| `surface-raised` | Panel, status bar fill | `#161B24` | 234 | `#EAEAE5` | 254 |
| `surface-overlay` | Tool call block fill | `#1E2535` | 235 | `#E0E0DB` | 253 |
| `surface-border` | Inactive borders | `#2A3347` | 237 | `#CCCCCA` | 251 |
| `surface-focus` | Active / focused border | `#F75077` | 204 | `#C0183A` | 160 |
| `text-primary` | Main content text | `#E8EAF0` | 252 | `#1A1C24` | 234 |
| `text-secondary` | Metadata, timestamps | `#7A8299` | 244 | `#5A5C6A` | 242 |
| `text-muted` | Disabled, placeholders | `#3D4456` | 238 | `#AAAAAA` | 248 |
| `text-on-accent` | Text placed on accent fill | `#FFFFFF` | 15 | `#FFFFFF` | 15 |
| `role-user` | User message chrome | `#5BA4F5` | 75 | `#1A6BC4` | 26 |
| `role-assistant` | Assistant message chrome | `#F75077` | 204 | `#C0183A` | 160 |
| `role-system` | System / info messages | `#7A8299` | 244 | `#5A5C6A` | 242 |
| `role-tool` | Tool call chrome | `#C084FC` | 177 | `#7E22CE` | 91 |
| `role-tool-result` | Tool result chrome | `#6EE7B7` | 120 | `#065F46` | 22 |
| `role-error` | Error messages | `#F87171` | 203 | `#B91C1C` | 124 |
| `syntax-keyword` | Code: keyword | `#F75077` | 204 | `#B91C1C` | 124 |
| `syntax-string` | Code: string literal | `#6EE7B7` | 120 | `#065F46` | 22 |
| `syntax-number` | Code: numeric literal | `#FCD34D` | 221 | `#92400E` | 130 |
| `syntax-comment` | Code: comment | `#4B5563` | 240 | `#9CA3AF` | 246 |
| `syntax-type` | Code: type / class | `#60A5FA` | 75 | `#1D4ED8` | 26 |
| `syntax-function` | Code: function name | `#C084FC` | 177 | `#7E22CE` | 91 |
| `diff-add` | Diff: added line background | `#052E16` | 22 | `#D1FAE5` | 120 |
| `diff-add-fg` | Diff: added line text | `#6EE7B7` | 120 | `#065F46` | 22 |
| `diff-remove` | Diff: removed line background | `#3B0A14` | 52 | `#FEE2E2` | 224 |
| `diff-remove-fg` | Diff: removed line text | `#F87171` | 203 | `#991B1B` | 124 |
| `diff-hunk` | Diff: hunk header | `#1E3A5F` | 18 | `#DBEAFE` | 153 |
| `diff-hunk-fg` | Diff: hunk header text | `#7A8299` | 244 | `#1E3A5F` | 18 |
| `budget-low` | Budget bar: safe zone | `#6EE7B7` | 120 | `#065F46` | 22 |
| `budget-mid` | Budget bar: warning zone | `#FCD34D` | 221 | `#92400E` | 130 |
| `budget-high` | Budget bar: danger zone | `#F87171` | 203 | `#B91C1C` | 124 |

### 2.2 Contrast Matrix (WCAG AA)

All text/background pairings used in the design must meet 4.5:1 for normal text, 3:1 for large text (>18pt / bold >14pt). Terminal text is treated as normal size throughout.

| Foreground token | Background token | Ratio (dark) | Ratio (light) | Status |
|---|---|---|---|---|
| `text-primary` on `surface-base` | 12.1:1 | 10.8:1 | AA pass |
| `text-secondary` on `surface-base` | 5.2:1 | 4.9:1 | AA pass |
| `text-muted` on `surface-base` | 2.4:1 | 2.6:1 | Decorative only — never carry meaning |
| `accent-hot` on `surface-base` | 4.6:1 | 4.8:1 | AA pass |
| `role-user` on `surface-base` | 5.1:1 | 4.5:1 | AA pass |
| `role-tool` on `surface-raised` | 4.7:1 | 4.9:1 | AA pass |
| `text-on-accent` on `accent-hot` | 4.5:1 | 5.2:1 | AA pass |
| `diff-add-fg` on `diff-add` | 5.8:1 | 5.6:1 | AA pass |
| `diff-remove-fg` on `diff-remove` | 5.9:1 | 6.1:1 | AA pass |
| `syntax-comment` on `surface-overlay` | 2.1:1 | 2.3:1 | Decorative only |

Note: `text-muted` and `syntax-comment` are intentionally below threshold. They are used exclusively for structural decoration (disabled hints, code comments) — never as the sole carrier of essential information.

---

## 3. Theme Presets

Three named presets ship with Hone. All override only the tokens listed; unspecified tokens inherit from the default (dark) palette.

### 3.1 Default — "Crimson" (dark)

The primary experience. Uses the full token table from Section 2.1 as defined.

```
Theme name:       crimson
Background style: dark
Accent family:    red/pink (#F75077 → #7B1D35)
```

### 3.2 "Neon" (dark, higher contrast)

For high-ambient-light environments where the subdued surface colors wash out. Increases all `surface-*` luminance by ~15% and saturates accents.

| Token override | Value |
|---|---|
| `surface-raised` | `#1F2937` |
| `surface-overlay` | `#273448` |
| `surface-border` | `#374151` |
| `accent-hot` | `#FF2D5E` |
| `role-user` | `#38BDF8` |
| `role-assistant` | `#FF2D5E` |
| `role-tool` | `#D946EF` |
| `role-tool-result` | `#34D399` |

Neon uses the same border-drawing characters but raises border brightness. The prompt sigil uses a bold modifier. All other tokens remain as Crimson.

### 3.3 "Chalk" (light)

For light-background terminals (e.g., macOS Terminal "Basic", iTerm "Solarized Light"). Uses the "Light" column from the contrast matrix.

| Token override | Value |
|---|---|
| `surface-base` | terminal default (inherit) |
| `surface-raised` | `#EAEAE5` |
| `surface-overlay` | `#E0E0DB` |
| `surface-border` | `#CCCCCA` |
| `surface-focus` | `#C0183A` |
| `text-primary` | `#1A1C24` |
| `text-secondary` | `#5A5C6A` |
| `text-muted` | `#AAAAAA` |
| `accent-hot` | `#C0183A` |
| `accent-mid` | `#A8102E` |
| `role-user` | `#1A6BC4` |
| `role-assistant` | `#C0183A` |
| `role-tool` | `#7E22CE` |
| `role-tool-result` | `#065F46` |

### 3.4 "Minimal" (no-color fallback)

When `NO_COLOR` is set, `TERM=dumb`, or the terminal reports fewer than 8 colors. All color is stripped. Visual hierarchy is achieved through:

- Bold modifier for headers, role labels, and active borders
- Underline for links and command names
- Box-drawing characters for all structural borders (retained)
- `>` prefix instead of colored sigil
- `[TOOL]`, `[USER]`, `[AI]` text labels instead of colored chrome
- Status bar rendered as plain `---` separator with text fields

The `Theme` struct must carry a `no_color: bool` flag. When true, every `Color::*` field is replaced with `Color::Reset` and the renderer falls back to the Minimal layout variant.

---

## 4. Typography and Symbols

### 4.1 Unicode Box-Drawing Characters

Use exclusively the standard Unicode box-drawing block (U+2500–U+257F). Do not mix single and double-line styles within the same widget.

**Primary style — rounded corners (most panels):**
```
╭─────────────────────╮
│                     │
╰─────────────────────╯
```
Characters: `╭` U+256D, `╮` U+256E, `╰` U+2570, `╯` U+256F, `─` U+2500, `│` U+2502

**Secondary style — sharp corners (tool call blocks, diff panels):**
```
┌─────────────────────┐
│                     │
└─────────────────────┘
```
Characters: `┌` U+250C, `┐` U+2510, `└` U+2514, `┘` U+2518

**Separator lines:**
```
────────────────────── (full-width horizontal, within panel)
╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌ (dashed, between messages — lighter visual weight)
```
Characters: `─` U+2500, `╌` U+254C

**Title inset (panel headers):**
```
╭─ Tool: read_file ──────────────╮
```
The title string is inserted directly into the top border, one space padded on each side of the title text.

### 4.2 The Prompt Sigil

The prompt sigil is the key brand carrier in ASCII space. It must echo the stepped-arrow mark from the logo.

```
>
```

Rendered as: `>` in `accent-hot` with bold modifier, preceded by one space, followed by one space. This is the sole use of the accent color in the input area when idle.

During streaming, the sigil pulses (see Section 14).

### 4.3 Spinner Frames

For all thinking/loading states, use this 8-frame spinner sequence. It is compact, readable in all terminals, and has directional motion that echoes the right-pointing arrow motif:

```
Frame 0:  ─
Frame 1:  \
Frame 2:  │
Frame 3:  /
Frame 4:  ─
Frame 5:  \
Frame 6:  │
Frame 7:  /
```

Tick rate: 100ms per frame (10 fps). This is the minimum that feels smooth without burning CPU.

Alternative ASCII-safe spinner for dumb terminals:
```
Frame 0:  [   ]
Frame 1:  [.  ]
Frame 2:  [.. ]
Frame 3:  [...]
Frame 4:  [ ..]
Frame 5:  [  .]
```

### 4.4 Progress and Sparkline Characters

Budget consumption sparkline uses the 8-level block fill characters:
```
▁ ▂ ▃ ▄ ▅ ▆ ▇ █
```
U+2581 through U+2588. Each character represents one time-bucket of spend.

Inline progress bars use:
```
█████░░░░░  (50%)
```
Fill: `█` U+2588, Empty: `░` U+2591.

Thinking dots (three-dot pulse):
```
   (empty pause)
.
..
...
..
.
```
Cycle through these 6 frames at 300ms each. Displayed immediately after the assistant sigil when streaming has not yet produced any tokens.

### 4.5 Diff Gutter Characters

```
+ added line
- removed line
  unchanged line
~ modified (context)
@ @@ hunk header
```

The `+` and `-` are colored with `diff-add-fg` and `diff-remove-fg` respectively. Line numbers in the gutter use `text-muted`.

---

## 5. Layout Architecture

### 5.1 Primary Layout (Single-Pane, default)

The terminal area is divided into four horizontal bands:

```
┌─────────────────────────────────────────────────────────────┐
│  HEADER BAR                                            1 row │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  CONVERSATION VIEWPORT                         fills height │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│  INPUT AREA                              3–8 rows (dynamic) │
├─────────────────────────────────────────────────────────────┤
│  STATUS BAR                                            1 row │
└─────────────────────────────────────────────────────────────┘
```

**Minimum terminal size**: 80 columns × 24 rows. Below this, display a single-line warning:
```
Terminal too small (80×24 required). Resize to continue.
```

**Recommended terminal size**: 120 columns × 40 rows.

Height allocation:
- Header bar: 1 row, fixed
- Status bar: 1 row, fixed
- Input area: minimum 3 rows, expands to 8 rows maximum as text wraps, then scrolls internally
- Conversation viewport: all remaining rows

### 5.2 Split-Pane Layout (activated by `/split` or Ctrl+P)

```
┌──────────────────────────────┬──────────────────────────────┐
│  HEADER BAR                                           1 row  │
├──────────────────────────────┼──────────────────────────────┤
│                              │                              │
│  CONVERSATION VIEWPORT       │  FILE PREVIEW PANEL         │
│  (left, 50% default)         │  (right, 50% default)       │
│                              │                              │
├──────────────────────────────┴──────────────────────────────┤
│  INPUT AREA                                    3–8 rows      │
├─────────────────────────────────────────────────────────────┤
│  STATUS BAR                                           1 row  │
└─────────────────────────────────────────────────────────────┘
```

The split ratio is adjustable with `Alt+Left` / `Alt+Right` in 5% increments. Minimum pane width is 40 columns.

The file preview panel displays the most recently read or edited file. When a `read_file` or `write_file` tool call completes, the preview updates automatically with the file's content, syntax-highlighted, with the relevant line range scrolled into view and highlighted.

The vertical divider is: `│` U+2502 in `surface-border`. When the preview pane has focus, the divider changes to `accent-hot`.

### 5.3 Tool Output Panel (bottom panel variant)

An alternative layout mode activated by `/panel` shows a three-zone layout:

```
┌─────────────────────────────────────────────────────────────┐
│  HEADER BAR                                            1 row │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  CONVERSATION VIEWPORT                         fills height │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│  TOOL OUTPUT PANEL                              8 rows fixed │
├─────────────────────────────────────────────────────────────┤
│  INPUT AREA                                    3–5 rows      │
├─────────────────────────────────────────────────────────────┤
│  STATUS BAR                                            1 row │
└─────────────────────────────────────────────────────────────┘
```

The tool output panel is a scrollable buffer showing only tool call inputs and results, separate from the conversation flow. This is useful for users who want to watch file operations without losing conversational context.

### 5.4 Column Grid

At different terminal widths, the layout adapts as follows:

| Terminal width | Layout behavior |
|---|---|
| < 80 cols | Warning overlay, no interaction |
| 80–99 cols | Single pane. Status bar truncates model name first, then cost. |
| 100–119 cols | Single pane. Full status bar. Input area has full width. |
| 120–159 cols | Single pane at full width, or split with 60/40 ratio. |
| 160+ cols | Split pane at 50/50, conversation viewport gets max 80 cols (centered or left-aligned per user preference). |

At 160+ columns, the conversation viewport should be capped at 80 characters of content width (matching the comfort reading width for code). Excess width goes to the preview panel or left as padding.

---

## 6. Startup Experience

### 6.1 Splash Screen

Displayed for exactly 600ms on first launch, then crossfades into the conversation view. On subsequent launches within the same working directory (detected by session file), the splash is skipped — drop directly into the conversation view with the previous session history loaded.

The splash screen occupies the full terminal and is centered both horizontally and vertically.

```
┌─────────────────────────────────────────────────────────────┐
│                                                             │
│                                                             │
│                                                             │
│                  >=>                                        │
│                 >==>                                        │
│                >===>   H O N E                              │
│                 >==>                                        │
│                  >=>                                        │
│                                                             │
│              sharpen your code                              │
│                                                             │
│              v1.0.0  |  rust core  |  apache 2.0            │
│                                                             │
│                                                             │
│              model: claude-sonnet-4-5                       │
│              connecting...  /                               │
│                                                             │
│                                                             │
└─────────────────────────────────────────────────────────────┘
```

The ASCII mark (`>=>` / `>==>` / `>===>`) is rendered in `accent-hot` (bold). "H O N E" is in `text-primary` (bold). "sharpen your code" is in `text-secondary`. Version line is `text-muted`. "connecting..." + spinner is `text-secondary`.

The mark animates on entry: the three rows appear top-to-bottom with 80ms stagger. The text "H O N E" appears letter-by-letter with 40ms stagger starting 240ms after the mark completes.

Once the model connection is established, "connecting... /" becomes "ready" in `accent-hot`, held for 400ms, then the transition to the conversation view begins.

### 6.2 ASCII Mark Specification

The stepped-arrow mark is the primary visual identity element. It must be renderable in pure ASCII across all splash, header, and no-color modes:

```
Full size (splash screen, 9 rows):
 >
 =>
 ==>
>==>
>===>
>==>
 ==>
 =>
 >
```

Colored rendering: each `=` character in `accent-deep`, each `>` character in `accent-hot` (bold).

Compact size (header bar, 1 row):
```
>=>
```
Three characters wide. In `accent-hot`.

The compact mark is used in the header bar to the left of the "hone" wordmark.

### 6.3 Header Bar

The header bar occupies the top 1 row. It is always visible.

```
 >=>  hone                              ~/projects/myapp  main*
```

Left group: compact mark + product name. Mark in `accent-hot`, "hone" in `text-primary` bold.  
Right group: working directory path (shortened to last 2 path segments) and git branch (if detectable). Both in `text-secondary`.  
Background: `surface-raised`.

The header bar is not a Ratatui border — it is a plain `Paragraph` widget with manually composed content in a 1-row layout slot. This allows fine-grained color control per segment without the limitations of a border title.

---

## 7. Conversation View

### 7.1 Message Stream Layout

Messages render as a vertically scrollable list. Each message is a self-contained block with consistent internal structure. There is no hard line between messages — instead, vertical spacing (one blank row) acts as the separator.

For terminals narrower than 100 columns, the blank-row separator is replaced with a `╌` dashed line in `text-muted`.

```
                                                               
  > hone   14:23                                              
 ╭─────────────────────────────────────────────────────────╮  
 │  Fix the token refresh race condition in auth.           │  
 ╰─────────────────────────────────────────────────────────╯  
                                                               
  > model   14:23                                             
                                                               
  I'll analyze the authentication module and fix the race      
  condition in the token refresh logic. Let me start by        
  reading the relevant files.                                   
                                                               
```

**User message block**: rounded border in `role-user`, role label "hone" (the product name acts as the user label, matching the `>` sigil), timestamp in `text-muted`, message body in `text-primary`.

**Assistant message block**: no border — text flows freely. Role label in `role-assistant`. Timestamp in `text-muted`. Body in `text-primary`. The absence of a border for assistant messages creates visual asymmetry that distinguishes them from user input and aligns with established chat conventions (border = user input, free-flow = response).

### 7.2 Role Labels

| Role | Label text | Color token | Prefix |
|---|---|---|---|
| User | `hone` | `role-user` (bold) | `> ` in `accent-hot` |
| Assistant | `model` (or model shortname if set) | `role-assistant` (bold) | `> ` in `role-assistant` |
| System | `system` | `role-system` | none |
| ToolCall | `tool: <name>` | `role-tool` (bold) | `> ` in `role-tool` |
| ToolResult | `result` | `role-tool-result` | none |
| Error | `error` | `role-error` (bold) | `! ` in `role-error` |

### 7.3 Timestamp Format

`HH:MM` in `text-muted`, right-aligned to the conversation width. On narrow terminals (< 100 cols), timestamps are omitted from the inline label and moved to a trailing position after the message body.

### 7.4 Scrolling Behavior

- The viewport auto-scrolls to the bottom when new content arrives.
- Manual scroll (Page Up / Page Down, arrow keys) disengages auto-scroll.
- A subtle scroll indicator appears in the bottom-right corner of the viewport when the user has scrolled up:

```
  ╰─ 12 more below ─>
```

Rendered in `text-muted`. Pressing `End` or `G` re-engages auto-scroll and dismisses the indicator.

- Smooth scrolling is not available in Ratatui — implement instant-scroll but add a `scroll_offset` animation state that interpolates over 3 frames (30ms) by adjusting the rendered offset by thirds. This creates the perception of easing without true per-pixel scrolling.

---

## 8. Message Rendering

### 8.1 Streaming State

While `is_streaming: true`, the assistant message renders with a thinking indicator appended to the last line of content:

**Before any tokens arrive (pure thinking state):**
```
  > model   14:23

  ...
```
The three dots are `text-muted`. They cycle through the 6-frame dot animation (Section 4.4).

**While tokens stream in:**
```
  > model   14:23

  I'll analyze the authentication module and fix the race
  condition. Let me start by reading the relevant files_
```
The cursor `_` in `accent-hot` blinks at 500ms on / 500ms off. The cursor is appended immediately after the last character of streamed content.

**After streaming completes:**
The cursor disappears. No other change. The message is now static.

### 8.2 Code Block Rendering

Fenced code blocks (` ``` ` delimited, or 4-space indented) render in a distinct container:

```
 ┌─ rust ─────────────────────────────────────────────────┐
 │  1  use std::sync::{Arc, Mutex};                       │
 │  2                                                      │
 │  3  pub struct TokenRefresher {                         │
 │  4      token: Arc<Mutex<Option<String>>>,              │
 │  5  }                                                   │
 └────────────────────────────────────────────────────────┘
```

- Container: sharp-corner box, `surface-overlay` background, `surface-border` border.
- Language label: inset in the top-left border in `text-secondary`.
- Line numbers: right-aligned in a 4-character gutter, separated from code by one space, colored `text-muted`.
- Syntax highlighting: minimal, deterministic highlighting using the `syntax-*` tokens. Do not depend on a tree-sitter grammar — implement a simple regex-based lexer covering: keywords, string literals, numeric literals, comments, and type names. This is sufficient for the display use case and has zero runtime cost.
- Content: left-padded 2 spaces, right-padded 2 spaces before the border.
- Horizontal scroll: if a line exceeds the container width, add a `<` / `>` indicator at the left/right edges and allow `Alt+Left` / `Alt+Right` to scroll the code block horizontally when it is focused.

**Syntax token detection order (for the simple lexer):**
1. Block comments (`/* ... */`)
2. Line comments (`//`, `#`, `--`)
3. String literals (single-quoted, double-quoted, backtick)
4. Keywords (language-specific keyword list, matched as whole words)
5. Numeric literals (`\d+\.?\d*`)
6. Type names (CamelCase words, or words following `:`)
7. Function names (word immediately preceding `(`)
8. Default: `text-primary`

### 8.3 Diff Rendering

When a message contains a unified diff (lines starting with `---`, `+++`, `@@`, `+`, `-`), the renderer switches to diff mode:

```
 ┌─ diff: src/auth/refresh.ts ────────────────────────────┐
 │  @@ -42,7 +42,12 @@                                    │  <- diff-hunk-fg on diff-hunk
 │    async function refreshToken() {                      │  <- unchanged
 │ -    const token = await fetchToken();                  │  <- diff-remove-fg on diff-remove
 │ +    const token = await fetchToken().catch(err => {    │  <- diff-add-fg on diff-add
 │ +      if (err.code === 'RACE_CONDITION') {             │  <- diff-add-fg on diff-add
 │ +        return retryWithBackoff(fetchToken, 3);        │  <- diff-add-fg on diff-add
 │ +      }                                                │  <- diff-add-fg on diff-add
 │ +      throw err;                                       │  <- diff-add-fg on diff-add
 │ +    });                                                │  <- diff-add-fg on diff-add
 │      return token;                                      │  <- unchanged
 │    }                                                    │  <- unchanged
 └────────────────────────────────────────────────────────┘
```

The filename is extracted from the `--- a/` / `+++ b/` header lines and displayed as the panel title. Hunk headers (`@@`) use the `diff-hunk` background. `+` lines use `diff-add` background for the full row width. `-` lines use `diff-remove` background for the full row width. Gutter characters (`+`, `-`, ` `) are bold.

A summary line appears below the diff block:
```
 +6 additions  -1 deletion   src/auth/refresh.ts
```
Additions in `diff-add-fg`, deletions in `diff-remove-fg`, filename in `text-secondary`.

### 8.4 Inline Formatting

Markdown inline elements are rendered as follows:

| Markdown | Rendering |
|---|---|
| `**bold**` | Bold modifier |
| `*italic*` | Italic modifier (if terminal supports) |
| `` `code` `` | `syntax-keyword` color, no bold |
| `[link](url)` | Underline on link text, url in `text-muted` after it |
| `# Heading` | Bold + `accent-hot`, followed by `─` underline to panel width |
| `## Heading` | Bold only |
| `- list item` | `>` prefix in `text-muted` |
| `> blockquote` | `│` left border in `text-muted`, indented 2 |

Do not attempt to render HTML. Pass it through as literal text.

---

## 9. Input Area

### 9.1 Normal State (Idle)

```
╭─────────────────────────────────────────────────────────────╮
│ > _                                                         │
╰─────────────────────────────────────────────────────────────╯
  Enter to send  Shift+Enter new line  / for commands  Ctrl+C quit
```

The border is in `surface-border` (inactive). The `>` sigil is `accent-hot` bold. The cursor `_` blinks at 500ms. The hint line below the border is `text-muted`, shown only when the input buffer is empty.

### 9.2 Active Typing State

```
╭─────────────────────────────────────────────────────────────╮
│ > Fix the token refresh race condition. The issue is that   │
│   multiple concurrent calls to refreshToken() can each      │
│   trigger a fetch before any has returned, causing the tok_ │
╰───────────────────────────────────────────────────────────  │
  line 3 / col 54  characters: 148
```

- Border changes to `surface-focus` (the active focus color, which is `accent-hot` in the default theme).
- Lines past the first are indented 3 characters to align with the sigil offset.
- When the input exceeds 8 rows, the box height caps at 8 rows and the content scrolls internally.
- A character count appears bottom-left below the border in `text-muted`.
- Line number and column position appear bottom-left when the input has more than one line.

### 9.3 Slash Command Autocomplete Dropdown

When the input buffer begins with `/`, an autocomplete dropdown appears above the input area:

```
╭─ commands ──────────────────────────────────────╮
│  /model <model-id>     switch active model      │
│  /cost                 show session cost        │
│  /undo                 undo last agent action   │
│  /redo                 redo                     │
│ [/help]                show command help        │  <- currently selected
│  /quit                 exit hone               │
╰─────────────────────────────────────────────────╯
╭─────────────────────────────────────────────────╮
│ > /h_                                           │
╰─────────────────────────────────────────────────╯
```

Dropdown behavior:
- Appears immediately on `/` (before any further characters are typed).
- Filters to matching commands as the user types.
- Arrow Up / Down navigate the selection. Tab completes the current selection into the input buffer.
- Enter executes the selected command.
- Escape dismisses the dropdown and returns to normal input.
- Selected row uses `surface-overlay` background and `accent-hot` text.
- The dropdown maximum height is 10 rows. If more commands match, show a scroll indicator.
- The dropdown is positioned above the input area (never below, to avoid obscuring the status bar).
- Each row shows: command name in `text-primary` bold, arguments in `text-secondary`, description in `text-muted`.

### 9.4 Waiting State (WaitingForResponse)

```
╭─────────────────────────────────────────────────────────────╮
│   thinking  /                                               │
╰─────────────────────────────────────────────────────────────╯
```

- The input is disabled — keystrokes are absorbed but not rendered.
- The text "thinking" is `text-muted`, the spinner is `accent-hot`.
- The border is `surface-border` (same as idle — no focus indicator while waiting).
- The hint line is replaced with: `Ctrl+C to interrupt`

### 9.5 Error State

```
╭─────────────────────────────────────────────────────────────╮
│ > _                                                         │
╰─────────────────────────────────────────────────────────────╯
  ! API error: rate limit exceeded. Retry in 12s.
```

- Border in `role-error`.
- Error message below the border in `role-error`.
- Input is re-enabled so the user can retry.

### 9.6 Multi-line Paste Detection

When a paste event contains newlines (detected by rapid successive character events, or via Bracketed Paste Mode), the input area immediately expands to show the full pasted content up to the 8-row cap, and displays a confirmation prompt:

```
  Paste 6 lines?  Enter to confirm  Esc to cancel
```

This prevents accidental submission of large pastes.

---

## 10. Status Bar

### 10.1 Full-Width Layout (120+ columns)

The status bar is a single row at the bottom of the terminal.

```
 qwen3:32b  |  12,430 tok  |  $0.04  |  ||||||||░░  |  14:03  |  Ctrl+? help
```

Segments, left to right:

1. **Model name**: `text-primary` bold. Truncated to 16 characters with `…` if needed.
2. **Separator**: `│` in `text-muted`
3. **Token count**: formatted with commas (e.g., `12,430 tok`). `text-secondary`.
4. **Separator**: `│` in `text-muted`
5. **Session cost**: `$0.04` formatted to 2 decimal places if > $0.01, 4 places if < $0.01. `text-secondary`. Changes to `budget-mid` above 50% of budget, `budget-high` above 80%.
6. **Separator**: `│` in `text-muted`
7. **Budget sparkline**: see Section 10.2.
8. **Separator**: `│` in `text-muted`
9. **Session duration**: `HH:MM` elapsed. Updates every minute. `text-secondary`.
10. **Separator**: `│` in `text-muted`
11. **Shortcut hint**: `Ctrl+? help` in `text-muted`. Changes to active tool name when a tool is running (e.g., `running: read_file`).

Background: `surface-raised`. The entire status bar row has `surface-raised` background applied.

### 10.2 Budget Sparkline

The budget sparkline is a 10-character bar showing:
- Cost consumption relative to the configured budget (if set).
- Each character represents 10% of the budget.
- Filled segments use `█` (U+2588), empty segments use `░` (U+2591).
- Color: `budget-low` from 0–50%, `budget-mid` from 50–80%, `budget-high` above 80%.

```
0%:    ░░░░░░░░░░
25%:   ███░░░░░░░  (budget-low)
55%:   █████░░░░░  (first segment switches to budget-mid)
85%:   █████████░  (budget-high)
```

When no budget is configured, display `no limit` in `text-muted` in the sparkline slot.

The sparkline is computed from `session_cost_usd / budget_usd`. The `StatusInfo` struct needs a `budget_usd: Option<f64>` field.

### 10.3 Narrow-Terminal Fallback (80–99 columns)

```
 qwen3:32b  tok:12430  $0.04  14:03
```

Model name truncated to 10 characters. Cost and token count condensed. Sparkline and session duration omitted. No separator pipes — single space separators.

### 10.4 Active Tool Indicator

When a tool call is in progress, the status bar rightmost segment changes to:

```
  running: read_file  /
```

Where `read_file` is the tool name in `role-tool` bold, and `/` is the spinner in `accent-hot`. The spinner replaces the help hint for the duration of the tool call.

### 10.5 Mode Indicator

The `mode` field from `StatusInfo` (currently `"chat"`) is displayed as a small badge to the far left of the status bar:

```
[CHAT]  qwen3:32b  |  ...
```

Badge: `text-on-accent` on `accent-deep` background. When mode is `"auto"` or `"code"`, the badge color changes to `accent-mid`. This signals to the user which operating mode is active without requiring a separate panel.

---

## 11. Tool Call Panels

Tool calls are the most complex visual element. They must communicate: what tool ran, what arguments were passed, the result, and whether it succeeded or failed — in a compact, scannable form.

### 11.1 Tool Call Block (collapsed by default)

```
 ┌─ > read_file ────────────────────────────────────── [+] ──┐
 │  src/auth/refresh.ts  (lines 1–89)                        │
 └───────────────────────────────────────────────────────────┘
```

- Sharp-corner box, `surface-overlay` background.
- Title: `> ` sigil in `role-tool`, tool name in `role-tool` bold, `[+]` expand indicator in `text-muted` right-aligned.
- Body: one-line summary of the arguments (file path + line range for file tools, query for search tools, etc.).
- Press Enter (when focused) or Space to expand/collapse.

### 11.2 Tool Call Block (expanded)

```
 ┌─ > read_file ────────────────────────────────────── [-] ──┐
 │  Input                                                    │
 │  ──────────────────────────────────────────────────────── │
 │    path: "src/auth/refresh.ts"                            │
 │    range: [1, 89]                                         │
 │                                                           │
 │  Result                                                   │
 │  ──────────────────────────────────────────────────────── │
 │  1   import { TokenCache } from './cache';                │
 │  2   import { fetchToken } from './api';                  │
 │  3                                                        │
 │  ... (87 more lines)  Press Space to show all            │
 └───────────────────────────────────────────────────────────┘
```

- Input section: argument key-value pairs, keys in `text-secondary`, values in `text-primary`.
- Result section: truncated to 5 lines with a "N more lines" indicator. Press Space again (or `e`) to show the full result in a scrollable sub-view.
- Separator lines (`──`) in `text-muted`.

### 11.3 Tool Call States

| State | Visual treatment |
|---|---|
| Running | Title spinner in `accent-hot`. Box border in `role-tool`. Body shows "running..." in `text-muted`. |
| Success | `[+]` indicator in `role-tool-result`. Border `surface-border`. |
| Error | `[!]` indicator in `role-error`. Border `role-error`. Body shows error message in `role-error`. |
| Approval required | Border `budget-mid` (yellow). Body: "Approval required: {description}" with `[y]` yes and `[n]` no hints. |

### 11.4 Approval Required State

When `AgentEvent::ApprovalRequired` fires, the tool call block renders in an elevated state:

```
 ┌─ > write_file ─────────────────────────────── [APPROVAL] ─┐
 │  This action requires your approval:                      │
 │                                                           │
 │  Write 42 lines to src/auth/refresh.ts                   │
 │                                                           │
 │  [y] Allow    [n] Deny    [d] Show diff first             │
 └───────────────────────────────────────────────────────────┘
```

- Border in `budget-mid` (yellow — distinct from both success green and error red).
- The block takes keyboard focus automatically. The conversation viewport loses focus.
- `y` / `n` / `d` keypresses are consumed by this block.
- `[d]` triggers the diff view of the proposed change in the file preview panel (split layout) or in an overlay (single layout).

---

## 12. Split-Pane Layout (File Preview)

### 12.1 File Preview Panel States

The file preview panel has its own 7 states:

**Empty** (no file selected):
```
╭───────────────────────────────╮
│                               │
│                               │
│       no file selected        │
│   tool calls will appear      │
│   here automatically          │
│                               │
│                               │
╰───────────────────────────────╯
```
Text in `text-muted`, centered.

**Loading** (tool call in progress):
```
╭─ loading... ──────────────────╮
│                               │
│           /                   │
│                               │
╰───────────────────────────────╯
```
Spinner in `accent-hot`.

**Loaded** (file displayed):
```
╭─ src/auth/refresh.ts ─ rust ──╮
│  1   use std::sync::Arc;      │
│  2   use tokio::sync::Mutex;  │
│  3                            │
│ [42] pub async fn refresh() { │  <- highlighted line
│ [43]   let guard = token      │
│  44       .lock().await;      │
│  45   guard.take()            │
│ ...                           │
╰───────────────────────────────╯
```
The highlighted range (from the tool call's line parameters) uses `surface-overlay` background on those rows. Line numbers in brackets `[42]` indicate the highlighted range.

**Error** (file not found or read error):
```
╭─ error ───────────────────────╮
│                               │
│  could not read file:         │
│  src/auth/refresh.ts          │
│  no such file or directory    │
│                               │
╰───────────────────────────────╯
```
Border and error text in `role-error`.

**Partial** (large file, only showing range):
```
╭─ src/auth/refresh.ts ─ 1/8 ───╮
│                               │
│  [showing lines 42–89 of 712] │
│  Page Up/Down to browse       │
│                               │
╰───────────────────────────────╯
```
Above the file content. `text-muted`.

**Offline** (not applicable to file preview — show stale content with indicator):
```
╭─ src/auth/refresh.ts  [stale] ╮
```
`[stale]` in `budget-mid`.

**Refreshing** (re-reading after a write):
```
╭─ src/auth/refresh.ts  / ──────╮
```
Spinner after the filename.

---

## 13. Help Overlay

Activated by `Ctrl+?` or `F1`. Renders as a centered overlay over the entire terminal.

```
╭─────────────────────────────────────────────────────────────╮
│                                                             │
│   H O N E  —  keyboard reference                           │
│                                                             │
│   Navigation                                               │
│   ─────────────────────────────────────────────────────    │
│   Page Up / Page Down     scroll conversation              │
│   Home / G                jump to bottom (latest)          │
│   End                     jump to top                      │
│   Tab                     cycle focus between panes        │
│                                                             │
│   Input                                                     │
│   ─────────────────────────────────────────────────────    │
│   Enter                   send message                     │
│   Shift+Enter             new line                         │
│   Ctrl+C                  quit / interrupt agent           │
│   /                       open command palette             │
│   Ctrl+Z                  undo last message                │
│                                                             │
│   Layout                                                    │
│   ─────────────────────────────────────────────────────    │
│   Ctrl+P                  toggle split pane                │
│   Alt+Left / Alt+Right    resize split panes               │
│   Ctrl+L                  clear conversation view          │
│                                                             │
│   Tool calls                                               │
│   ─────────────────────────────────────────────────────    │
│   Space                   expand / collapse tool call      │
│   y / n                   approve / deny (when prompted)   │
│   d                       show diff before approving       │
│                                                             │
│   Press Esc or Ctrl+? to close                             │
│                                                             │
╰─────────────────────────────────────────────────────────────╯
```

- Overlay: `surface-raised` background, `surface-focus` border (rounded corners).
- Title: `accent-hot` bold.
- Section headers: `text-secondary` bold, underlined with `─`.
- Key names: `text-primary` bold.
- Descriptions: `text-secondary`.
- Overlay is rendered using Ratatui's `Clear` widget applied to the overlay region, then `Block` + `Paragraph` rendered on top.

---

## 14. Animation and Motion

All animations in Ratatui are tick-based. The event loop must call `terminal.draw()` on a timer tick even when no user event has occurred, to advance animation state. Recommended tick rate: 100ms (10 fps) for all spinner and typing animations.

### 14.1 Animation Registry

| Element | Animation type | Timing | Frames / behavior |
|---|---|---|---|
| Splash mark reveal | Staggered row appear | 80ms per row | Rows top-to-bottom, each appears in full |
| Splash wordmark reveal | Letter-by-letter | 40ms per letter | After mark completes + 100ms delay |
| Splash connecting state | Spinner | 100ms/frame | 8-frame spinner (Section 4.3) |
| Cursor blink (input) | Binary toggle | 500ms on / 500ms off | `_` shown / hidden |
| Streaming cursor | Binary toggle | 500ms on / 500ms off | `_` in `accent-hot` |
| Thinking dots (pre-token) | 6-frame cycle | 300ms/frame | ` ` / `.` / `..` / `...` / `..` / `.` |
| Spinner (tool running) | 8-frame cycle | 100ms/frame | Standard spinner |
| Status spinner (waiting) | 8-frame cycle | 100ms/frame | Rightmost status segment |
| Scroll indicator fade | Delayed appearance | Show after 200ms of scroll position | Instant on, instant off |
| Approval block focus | Border color change | Instant | `surface-border` -> `budget-mid` |

### 14.2 Startup Sequence Timing

```
t=0ms      Terminal cleared, splash background drawn
t=80ms     First mark row appears (top '>')
t=160ms    Second mark row ('=>')
t=240ms    Third mark row ('==>')
t=320ms    Fourth mark row ('===>')
t=400ms    Fifth mark row ('==>')
t=480ms    Sixth mark row ('=>')
t=560ms    Seventh mark row ('>')
t=660ms    Wordmark 'H' appears
t=700ms    'O'
t=740ms    'N'
t=780ms    'E'
t=880ms    Tagline appears instantly
t=980ms    Version line appears
t=1080ms   Model name appears, connecting spinner starts
t=1080ms+  Wait for connection. Spinner runs.
t=conn     "connecting..." -> "ready" in accent-hot
t=conn+400ms  Crossfade to conversation view (clear + draw main layout)
```

If connection fails during startup, skip the "ready" state and go directly to the error state with the full layout showing an error message in the conversation area.

### 14.3 Message Arrival Animation

When a new message block arrives in the conversation viewport (any role), the block appears instantly at the bottom — no slide-in animation, as Ratatui cannot animate position. However, if the viewport was auto-scrolled and a new message pushes existing messages up, the `scroll_offset` animation (Section 7.4) smooths the transition over 3 render ticks.

### 14.4 Animation State in App

The `App` struct needs an `AnimationState` sub-struct:

```
AnimationState {
    tick_count: u64           -- increments on every 100ms tick
    cursor_visible: bool      -- toggled every 5 ticks (500ms)
    spinner_frame: u8         -- tick_count % 8
    dot_frame: u8             -- tick_count % 6 (at 300ms rate = every 3 ticks)
    splash_phase: SplashPhase -- enum tracking startup sequence
}
```

The renderer reads from `AnimationState` — it does not produce side effects. `AnimationState` is updated in the tick handler, separate from the key event handler.

---

## 15. Screen State Inventory

The seven mandatory screen states for each major view component:

### 15.1 Conversation Viewport

| State | Trigger | Visual |
|---|---|---|
| Empty | No messages, fresh session | Centered welcome text: "Start by typing a message below" in `text-muted` |
| Loading | `AppState::WaitingForResponse`, no messages yet | Thinking dots centered in viewport |
| Loaded | Messages present, `AppState::Idle` | Normal message stream |
| Error | `AppState::Error(_)` | Last message shows error block; input border in `role-error` |
| Partial | Very large message that exceeds viewport height | Scroll indicator appears; auto-scroll still active |
| Offline | No network detected | System message: "Offline — check your connection" in `role-error`; input disabled |
| Refreshing | User sends new message, response incoming | Previous messages stay; new streaming message appended |

### 15.2 Input Area

| State | Trigger | Visual |
|---|---|---|
| Empty | Buffer empty, idle | Sigil + blinking cursor + hint line below |
| Loading | `AppState::WaitingForResponse` | "thinking" + spinner; input disabled; border `surface-border` |
| Loaded | Buffer has text, idle | Sigil + text + blinking cursor; border `surface-focus` |
| Error | `AppState::Error(_)` after Done | Border `role-error`; error hint below |
| Partial | Autocomplete dropdown open | Dropdown rendered above; input shows partial command |
| Offline | No network | Input disabled; hint: "Offline — cannot send" |
| Refreshing | Submission accepted, clearing buffer | Buffer clears; transitions to Loading state |

### 15.3 File Preview Panel (split layout only)

Documented in Section 12.1.

### 15.4 Status Bar

| State | Trigger | Visual |
|---|---|---|
| Empty | No model set, no cost | Model slot shows "no model" in `text-muted` |
| Loading | Session initializing | Model slot shows spinner |
| Loaded | Normal operation | Full segment layout as documented |
| Error | Agent error | Cost slot shows `ERR` in `role-error` |
| Partial | Budget set, >80% consumed | Cost in `budget-high`; sparkline in `budget-high` |
| Offline | No network | Rightmost slot shows "offline" in `role-error` |
| Refreshing | Tool call running | Rightmost slot shows active tool name + spinner |

---

## 16. Accessibility and Degradation

### 16.1 Color Blindness

Hone's primary accent is red/pink — a problematic color for deuteranopia (red-green color blindness). The following compensating design decisions are in place:

- **Role labels are text, not color-only.** The user can always read "user", "model", "tool:" as text. Color reinforces but does not replace the label.
- **The `>` and `!` sigil prefixes** (also text, not color-only) provide a secondary shape-based cue for user vs. error messages.
- **Diff `+` / `-` characters** are present in the gutter — removing color still leaves the `+` / `-` text as the primary indicator.
- **The neon theme** has higher contrast between all role colors, which helps users with reduced contrast sensitivity.

A future `--theme colorblind` preset should shift the assistant accent from pink to orange (`#FB923C`, ANSI 214) and preserve all other behaviors. This is not in scope for v1 but the token architecture makes it a single-struct change.

### 16.2 No-Color Mode

Activation conditions (checked in priority order):
1. `NO_COLOR` environment variable set to any value
2. `TERM=dumb`
3. `COLORTERM` not set and `TERM` does not contain `256color` or `truecolor`
4. `--no-color` CLI flag

When no-color mode activates, the `Theme` struct is replaced with the Minimal theme (Section 3.4). The TUI still uses box-drawing characters and bold/underline modifiers, which work on all terminals that support the basic VT100 modifier set.

### 16.3 Keyboard-Only Navigation

All interactive elements are reachable by keyboard:

- Tab cycles focus: Conversation viewport -> Input area -> File preview (if open) -> Conversation viewport.
- Within each area, arrow keys navigate the focused element's content.
- Enter activates the focused element's primary action.
- Escape cancels modals (autocomplete, help overlay, approval prompt).

There is no mouse support in v1. The `crossterm` backend supports mouse events, but implementing mouse semantics cleanly for a streamed conversation view has high complexity for low user benefit in a terminal-first product.

### 16.4 Minimum Terminal Support

| Terminal capability | Required | Fallback behavior |
|---|---|---|
| 8 ANSI colors | Yes | Hard minimum — Minimal theme uses bold/underline only |
| 256-color | Recommended | All ANSI-256 values from the token table apply |
| Truecolor (24-bit) | Optional | Hex values used when `COLORTERM=truecolor`; 256 otherwise |
| UTF-8 encoding | Required | Box-drawing depends on UTF-8; ASCII fallback for box chars if needed |
| Alternate screen | Recommended | Without it, Ratatui output will not clear on exit |
| Bracketed paste | Optional | Without it, paste detection is approximate |

### 16.5 Resize Handling

The terminal emits a `Resize(width, height)` event via `crossterm`. On receipt:
- Recompute all layout constraints immediately.
- If the new size is below the minimum (80×24), render the too-small warning overlay.
- Do not lose conversation history on resize — only the viewport render changes.
- The input buffer is preserved across resize events.
- Animated states reset their frame counter on resize to avoid visual glitches from mid-animation redraws.

---

## 17. Ratatui Widget Map

### 17.1 Recommended Widget Assignments

| UI element | Ratatui widget | Notes |
|---|---|---|
| Header bar | `Paragraph` | Manually composed `Line` with `Span`s for each color segment |
| Conversation viewport | `List` or custom `StatefulWidget` | `List` for simple cases; custom widget for streaming cursor and tool blocks |
| User message border | `Block` with `Borders::ALL`, rounded border set | Use `block.border_type(BorderType::Rounded)` |
| Code block | `Block` + `Paragraph` inside | Nested block. Use `Padding::new(2, 2, 0, 0)` for horizontal pad |
| Diff block | `Block` + custom `Paragraph` | Each line rendered as `Line` with per-span `Style` |
| Tool call block | Custom `StatefulWidget` | Owns collapsed/expanded state, approval interaction |
| Input area | `Paragraph` with `Block` | Update border style based on `AppState` |
| Autocomplete dropdown | `List` with `ListState` | Positioned with `Layout::split` — allocate rows above input |
| Status bar | `Paragraph` | Single `Line` with `Span`s; no border, raw background color |
| Help overlay | `Clear` + `Block` + `Paragraph` | `Clear` widget paints the overlay region before the block |
| Splash screen | Custom widget | Manages per-row reveal state, reads from `AnimationState` |
| File preview | `Paragraph` with line highlighting | Use `Style` modifiers on specific `Line` indices |
| Budget sparkline | Inline `Span` sequence | Composed as part of the status bar `Line` |

### 17.2 Layout Computation

Use `Layout::vertical` and `Layout::horizontal` with `Constraint` values:

```
Vertical layout (single pane):
  [0] Header:   Constraint::Length(1)
  [1] Viewport: Constraint::Fill(1)
  [2] Input:    Constraint::Max(10)      -- expands up to 10, minimum 3
  [3] Status:   Constraint::Length(1)

Vertical layout (with tool panel):
  [0] Header:   Constraint::Length(1)
  [1] Viewport: Constraint::Fill(1)
  [2] Tools:    Constraint::Length(8)
  [3] Input:    Constraint::Max(7)
  [4] Status:   Constraint::Length(1)

Horizontal layout (split pane):
  [0] Left:     Constraint::Percentage(split_ratio)
  [1] Right:    Constraint::Fill(1)
```

`split_ratio` is a `u16` in `[30, 70]`, default 50. Stored in `App`.

### 17.3 The Message Renderer

The conversation viewport requires a custom rendering pass because:
- Tool call blocks have interactive state (collapsed/expanded).
- Code blocks have syntax-highlighted spans.
- Streaming messages have animated cursors.
- The viewport must scroll as a unit while individual blocks may themselves be taller than the viewport.

Recommended approach: implement a `ConversationWidget` that implements `StatefulWidget<State = ConversationState>`. The state holds:
- `scroll_offset: u16` — rows scrolled from the bottom
- `focused_block: Option<usize>` — which message block has Tab focus
- `animation: AnimationState` — reference or copy for rendering

The widget's `render()` method:
1. Computes the render height of each `DisplayMessage` in the `messages` slice (pre-layout pass).
2. Determines which messages are visible given `scroll_offset` and viewport height.
3. Renders each visible message block from bottom to top.
4. Applies the streaming cursor to the last message if `is_streaming`.

Pre-computing render height avoids the need to re-render off-screen messages. Cache this computation in `ConversationState` and invalidate it when messages change.

---

## 18. Theme Data Structures

### 18.1 Extended Theme Struct

The existing `Theme` struct must be extended to cover all the tokens defined in Section 2.1. The full struct definition (as a spec, not code) should include:

**Surface colors:**
- `surface_base`: `Color` — terminal background assumption
- `surface_raised`: `Color` — panel fills
- `surface_overlay`: `Color` — tool call, code block fills
- `surface_border`: `Color` — inactive borders
- `surface_focus`: `Color` — active/focused borders

**Text colors:**
- `text_primary`: `Color`
- `text_secondary`: `Color`
- `text_muted`: `Color`
- `text_on_accent`: `Color`

**Role colors (replaces the current per-role fields):**
- `role_user`: `Color`
- `role_assistant`: `Color`
- `role_system`: `Color`
- `role_tool`: `Color`
- `role_tool_result`: `Color`
- `role_error`: `Color`

**Accent colors:**
- `accent_hot`: `Color`
- `accent_mid`: `Color`
- `accent_deep`: `Color`

**Syntax colors:**
- `syntax_keyword`: `Color`
- `syntax_string`: `Color`
- `syntax_number`: `Color`
- `syntax_comment`: `Color`
- `syntax_type`: `Color`
- `syntax_function`: `Color`

**Diff colors:**
- `diff_add_bg`: `Color`
- `diff_add_fg`: `Color`
- `diff_remove_bg`: `Color`
- `diff_remove_fg`: `Color`
- `diff_hunk_bg`: `Color`
- `diff_hunk_fg`: `Color`

**Budget colors:**
- `budget_low`: `Color`
- `budget_mid`: `Color`
- `budget_high`: `Color`

**Behavioral flags:**
- `no_color: bool`
- `use_truecolor: bool`

### 18.2 Theme Selection at Startup

Theme resolution order:
1. `--theme <name>` CLI flag
2. `HONE_THEME` environment variable
3. `NO_COLOR` environment variable (forces Minimal)
4. `COLORTERM` / `TERM` check (256-color vs. 8-color detection)
5. Default: Crimson dark

### 18.3 CSS Custom Property Equivalents

For documentation and design tool parity, the tokens map to CSS custom properties as follows. These are reference values only — they are not used in the Rust implementation.

```css
:root {
    /* Accent */
    --hone-accent-hot:           #F75077;
    --hone-accent-mid:           #E8304A;
    --hone-accent-deep:          #7B1D35;

    /* Surface */
    --hone-surface-base:         #0E1117;
    --hone-surface-raised:       #161B24;
    --hone-surface-overlay:      #1E2535;
    --hone-surface-border:       #2A3347;
    --hone-surface-focus:        #F75077;

    /* Text */
    --hone-text-primary:         #E8EAF0;
    --hone-text-secondary:       #7A8299;
    --hone-text-muted:           #3D4456;
    --hone-text-on-accent:       #FFFFFF;

    /* Roles */
    --hone-role-user:            #5BA4F5;
    --hone-role-assistant:       #F75077;
    --hone-role-system:          #7A8299;
    --hone-role-tool:            #C084FC;
    --hone-role-tool-result:     #6EE7B7;
    --hone-role-error:           #F87171;

    /* Syntax */
    --hone-syntax-keyword:       #F75077;
    --hone-syntax-string:        #6EE7B7;
    --hone-syntax-number:        #FCD34D;
    --hone-syntax-comment:       #4B5563;
    --hone-syntax-type:          #60A5FA;
    --hone-syntax-function:      #C084FC;

    /* Diff */
    --hone-diff-add-bg:          #052E16;
    --hone-diff-add-fg:          #6EE7B7;
    --hone-diff-remove-bg:       #3B0A14;
    --hone-diff-remove-fg:       #F87171;
    --hone-diff-hunk-bg:         #1E3A5F;
    --hone-diff-hunk-fg:         #7A8299;

    /* Budget */
    --hone-budget-low:           #6EE7B7;
    --hone-budget-mid:           #FCD34D;
    --hone-budget-high:          #F87171;
}
```

---

## 19. Nielsen Heuristic Audit

Scoring: 1 (poor) — 5 (excellent). Evidence references the design decisions in this spec.

### 1. Visibility of System Status — Score: 4/5

**Evidence for:** The status bar continuously displays model name, token count, cost, budget sparkline, and session timer. The spinner in the status bar and the thinking indicator in the conversation viewport both signal that the agent is working. The streaming cursor signals that text is arriving. Tool call blocks show "running..." while active.

**Gap:** There is no explicit indicator of network latency — a fast spinner looks the same whether the model is thinking for 0.5s or 30s. A future improvement would be an elapsed-time counter that appears after 3 seconds of waiting. Scoring reflects this gap.

### 2. Match Between System and Real World — Score: 5/5

**Evidence for:** The `>` sigil echoes the shell prompt — a pattern every developer recognizes. Role labels use plain language ("tool:", "model", "error"). Diff rendering matches the universal `git diff` format. Budget terminology ("$0.04", "no limit") maps to developer expectations. Slash commands (`/quit`, `/help`, `/model`) match the IRC/Discord/Slack convention familiar to the target audience.

### 3. User Control and Freedom — Score: 4/5

**Evidence for:** `Ctrl+C` is always available to interrupt or quit. `Escape` dismisses all modals. The approval flow (`y` / `n` / `d`) gives explicit control over destructive tool calls. Undo/redo commands exist in the command registry (though not yet implemented in the agent layer). Conversation history is preserved across resize events.

**Gap:** There is no "stop streaming" shortcut that interrupts mid-response without quitting. `Ctrl+C` during streaming should interrupt the agent but stay in the application. This requires a state machine change (`AppState::Interrupting`) and is noted as a required addition.

### 4. Consistency and Standards — Score: 5/5

**Evidence for:** Box-drawing characters are used consistently — rounded corners for conversational elements (input, user messages), sharp corners for technical elements (code blocks, diffs, tool calls). Color token usage is rule-governed: `accent-hot` appears on exactly one element per context. The slash-command pattern is consistent with developer tool conventions. Spinner frames are the same across all loading contexts. Key bindings follow POSIX terminal conventions (`Ctrl+C`, `Ctrl+L`, Page Up/Down).

### 5. Error Prevention — Score: 4/5

**Evidence for:** The approval flow prevents destructive tool calls from executing without explicit confirmation. The diff preview (`d` key) lets users inspect changes before approving. Multi-line paste confirmation prevents accidental large-text submission. The autocomplete dropdown prevents mistyped slash commands.

**Gap:** There is no confirmation dialog for `/quit` when the agent is mid-task. The status bar should show a warning (`quit while agent is running?`) rather than exiting immediately. Marked as a behavioral note, not a layout spec concern.

### 6. Recognition Rather Than Recall — Score: 5/5

**Evidence for:** The autocomplete dropdown appears on `/` without requiring the user to remember command names. The hint line below the input box (when empty) reminds the user of the key bindings. The help overlay (`Ctrl+?`) is surfaced via the status bar hint. Role labels are always visible — the user never needs to remember which color means which role. Tool call blocks show the tool name and arguments explicitly.

### 7. Flexibility and Efficiency — Score: 4/5

**Evidence for:** The layout has three modes (single-pane, split-pane, tool panel) for different workflows. Collapsed tool call blocks let experienced users scan past tool operations quickly. Slash commands provide fast access to model switching, cost reporting, and undo. Page Up/Down for fast scrolling.

**Gap:** Power users who run many tool-heavy sessions may want a way to filter the conversation view to show only assistant messages (suppressing all tool call blocks). This is a reasonable future feature, not a v1 requirement.

### 8. Aesthetic and Minimalist Design — Score: 5/5

**Evidence for:** The status bar carries all metadata in a single row. Tool call blocks are collapsed by default — detail on demand. User messages have a border; assistant messages do not — asymmetry creates visual rhythm without adding elements. The color palette uses only one accent family (crimson/pink). Separators between messages are whitespace, not lines, except on narrow terminals. The splash screen animates in and then disappears — it does not compete with the working view.

### 9. Help Users Recognize and Recover From Errors — Score: 4/5

**Evidence for:** Error messages are shown in the conversation stream with an `!` sigil and `role-error` color. The input border turns `role-error` when the app is in an error state. Error tool call blocks show the error text in context. The status bar shows `ERR` in the cost slot when an agent error occurs.

**Gap:** Error messages are currently raw strings from the agent (`AgentEvent::Error { message }`). The TUI spec cannot fix upstream message quality, but the renderer should detect common error patterns (API errors, rate limits, network errors) and append a suggested recovery action below the error text. This is a future content design task.

### 10. Help and Documentation — Score: 4/5

**Evidence for:** The help overlay (`Ctrl+?`) covers all keyboard shortcuts. The autocomplete dropdown has inline descriptions for all commands. The hint line below the empty input covers the three most critical shortcuts. The `/help` command in the command registry produces a help message in the conversation.

**Gap:** There is no in-app explanation of what approval means or why certain tool calls require it. A one-time contextual tooltip (shown only the first time an approval is requested) would improve first-use experience. Not a layout spec concern, but a content design note.

---

## 20. Implementation Priority Order

The following order minimizes rework and builds toward the full spec incrementally. Each phase is independently shippable.

### Phase 1 — Foundation (prerequisite for everything)

1. Extend `Theme` struct with the full token set from Section 18.1.
2. Implement theme resolution at startup (Section 18.2).
3. Add `AnimationState` to `App` and a 100ms tick event in the event loop.
4. Add `budget_usd: Option<f64>` and `session_start: Instant` to `StatusInfo`.

### Phase 2 — Status Bar

5. Redesign status bar renderer to the full segment layout (Section 10).
6. Implement budget sparkline computation and rendering (Section 10.2).
7. Implement mode badge (Section 10.5).
8. Implement active tool indicator with spinner (Section 10.4).

### Phase 3 — Input Area

9. Redesign input area to rounded-corner box with sigil (Section 9.1).
10. Implement focus border color change (active/idle/error states).
11. Implement hint line (Section 9.1 and 9.4).
12. Implement slash command autocomplete dropdown (Section 9.3).
13. Implement multi-line input expansion (Section 9.2).

### Phase 4 — Message Rendering

14. Implement `ConversationWidget` as a `StatefulWidget` (Section 17.3).
15. Implement role label rendering with sigils and timestamps (Section 7.2).
16. Implement user message rounded border (Section 7.1).
17. Implement streaming cursor with blink animation (Section 8.1).
18. Implement thinking dots animation for pre-token state (Section 8.1).

### Phase 5 — Code and Diff Blocks

19. Implement code block renderer with sharp-corner box and language label (Section 8.2).
20. Implement simple regex-based syntax lexer (Section 8.2).
21. Implement line-number gutter in code blocks.
22. Implement diff block renderer with per-line background coloring (Section 8.3).
23. Implement diff summary line (Section 8.3).
24. Implement inline markdown formatting (Section 8.4).

### Phase 6 — Tool Call Panels

25. Implement tool call block as collapsed/expanded `StatefulWidget` (Section 11).
26. Implement tool call running, success, and error states (Section 11.3).
27. Implement approval flow with `y` / `n` / `d` keybindings (Section 11.4).

### Phase 7 — Split-Pane Layout

28. Implement `Layout::horizontal` split with adjustable ratio (Section 5.2).
29. Implement file preview panel with all 7 states (Section 12.1).
30. Wire file preview to `read_file` / `write_file` tool call events.
31. Implement pane focus cycling with Tab.
32. Implement `Alt+Left` / `Alt+Right` ratio adjustment.

### Phase 8 — Startup Experience

33. Implement splash screen with step-animation mark and wordmark (Section 6.1).
34. Implement startup timing sequence (Section 14.2).
35. Implement session detection to skip splash on re-launch.

### Phase 9 — Help Overlay and Polish

36. Implement help overlay widget with `Clear` + centered block (Section 13).
37. Implement scroll indicator in conversation viewport (Section 7.4).
38. Implement header bar with compact mark and working directory (Section 6.3).
39. Implement `--no-color` / `NO_COLOR` Minimal theme (Section 3.4 and 16.2).
40. Implement resize handler with too-small warning (Section 16.5).
41. Implement `--theme` CLI flag and `HONE_THEME` environment variable.

### Phase 10 — Theme Presets

42. Implement Neon theme token overrides (Section 3.2).
43. Implement Chalk light theme token overrides (Section 3.3).
44. Add theme preview mode: `hone --theme-preview` renders a static screen showing all UI elements in the selected theme.

---

*End of specification.*
