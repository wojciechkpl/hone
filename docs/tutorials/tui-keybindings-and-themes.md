# TUI Keybindings and Themes

This guide covers everything you can do without leaving the keyboard: navigation, the command palette, autocomplete, themes, and accessibility flags.

## Layout overview

```
┌──────────────────── header (cwd, model, focus) ────────────────────┐
│                                                                    │
│                     conversation pane                              │
│                     (scrollable; tool calls inline)                │
│                                                                    │
├──────────────────── input (3 rows) ─────────────────────────────────┤
│ > type here, /<command>, or @<agent-name>                          │
└──────────────────── status bar (state, tokens, cost) ──────────────┘
```

## Keybindings

### Global

| Key | Action |
|---|---|
| `Ctrl+C` | Quit |
| `Ctrl+K` | Open command palette (fuzzy-search every command) |
| `Ctrl+F` | Search input history |
| `Ctrl+/` | Search the conversation scrollback |
| `Ctrl+A` | Session anatomy (turn-by-turn token/cost stats) |
| `Ctrl+M` | Prompt mirror (preview the exact context sent to the model) |
| `Ctrl+O` | Session switcher |
| `Ctrl+R` | Toggle the agent rail sidebar |
| `Esc` | Cancel current turn / close any overlay |
| `?` | Help overlay (shows all keybindings) |

### Input box

| Key | Action |
|---|---|
| `Enter` | Send |
| `Shift+Enter` / `Alt+Enter` | Insert newline (multi-line prompt) |
| `Up` / `Down` | Walk through input history |
| `Tab` | Accept the current autocomplete suggestion |
| `/` (at column 0) | Trigger slash-command autocomplete |
| `@` (at column 0) | Trigger agent-name autocomplete |

### Conversation pane (focus via `Ctrl+R` or scroll wheel)

| Key | Action |
|---|---|
| `j` / `k` | Scroll down / up one line |
| `Page Down` / `Page Up` | Scroll one viewport |
| `g` | Jump to oldest message |
| `G` | Jump to latest, exit scrollback |
| `Enter` | Expand a folded tool call |

### Overlays (palette / session switcher / anatomy)

| Key | Action |
|---|---|
| `j` / `k` or `Down` / `Up` | Move selection |
| `Enter` | Select |
| `Esc` | Close without selecting |

## The command palette

Press `Ctrl+K`. Start typing — both names and descriptions are fuzzy-matched. Each row shows `/name` followed by a short description so you don't need to memorise commands.

Common commands:

| Command | Effect |
|---|---|
| `/quit`, `/q` | Exit hone |
| `/help` | Show keybindings |
| `/cost` | Show session cost so far |
| `/review` | Diff every change made this session |
| `/configure` | Open the in-TUI config editor |
| `/configure provider <name>` | Hot-swap the active provider |
| `/configure model <id>` | Hot-swap the model |
| `/theme <name>` | Switch theme (`crimson`, `neon`, `chalk`, `minimal`) |
| `/theme-preview` | Render every theme color in a demo card |
| `/clarify [prompt]` | Force a clarification round before the next turn |
| `/tdd [cmd]` | Start TDD watch mode (auto-detects test command if omitted) |
| `/tdd off` | Stop the watcher |
| `/set vim` / `/set novim` | Toggle vim-style keybindings |

## Autocomplete

Typing `/` or `@` in column 0 pops up an autocomplete list. The dropdown shows the command/agent name **plus its description** so you can pick by intent rather than memory:

```
╭──────────────────────────────────────────────╮
│ /quit         Exit hone                      │
│ /help         Show keyboard shortcuts        │
│ /clear        Clear conversation history     │
│ /review       Enter review mode              │
│ /configure    Open configuration wizard      │
╰──────────────────────────────────────────────╯
```

`Tab` accepts the highlighted entry; `Up`/`Down` move the selection.

## Themes

Hone ships with four built-in themes:

| Theme | Description |
|---|---|
| `crimson` | Default. Dark base, hot-pink accents. |
| `neon` | High-contrast variant. Dark navy base, electric-cyan accents — visibly distinct from crimson. |
| `chalk` | Light theme. White surfaces, deep red accents. |
| `minimal` | Pure ANSI defaults; no color codes. Good for `script(1)` capture and unusual terminals. |

### Switching at runtime

```
/theme neon
```

The change is immediate and any `~/.config/hone/theme.toml` overrides are re-applied on every switch (see below).

### Switching at startup

```bash
HONE_THEME=chalk hone
```

`HONE_THEME` accepts the same names as `/theme`.

### Custom palette overrides

Create `~/.config/hone/theme.toml`:

```toml
# Override individual color tokens. Hex (#rrggbb) or ANSI-256 indices both work.
text_primary  = "#f0f0f0"
accent_hot    = "#ff5577"
surface_base  = 234
role_assistant = "#80c8ff"
```

Hone reads the file at startup (and on every `/theme` switch). RGB values you set there always win over the built-in palette.

The full token list is in `crates/hone-tui/src/theme.rs` (`Theme` struct fields). The 35 tokens cover surfaces, text, roles, accents, syntax, diffs, and the budget meter.

### Quick preview

```
/theme-preview
```

Shows every token side-by-side so you can eyeball contrast.

## Accessibility flags

These are environment variables. Set them before launching hone:

| Variable | Effect |
|---|---|
| `NO_COLOR=1` | Disable all colors. Equivalent to `/theme minimal`. |
| `HONE_NO_COLOR=1` | Same as `NO_COLOR`, scoped to hone. |
| `HONE_REDUCED_MOTION=1` | Replace spinners with a static `·` glyph. |
| `HONE_ASCII_ONLY=1` | Use plain ASCII for box-drawing (no Unicode borders). Helps `script` recordings, `ssh` over slow links, and some `tmux` configurations. |

You can combine them: `HONE_NO_COLOR=1 HONE_ASCII_ONLY=1 hone`.

## Vim-style keybindings (optional)

```
/set vim
```

Adds `h`/`j`/`k`/`l` navigation in the conversation pane, `i` to enter insert mode in the input box, `Esc` to leave insert mode, and `:` for command-line mode (mirroring `/`).

```
/set novim
```

Reverts to the default bindings.

## Mouse

Mouse scroll works in the conversation pane and in the agent rail. Click-and-drag inside the conversation selects text (highlighted in reverse video); copy with your terminal's normal copy shortcut.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Description text invisible in palette | Old build (≤ April 2026) | Update — descriptions now use `text_secondary`, contrast is fixed |
| `/theme neon` looks identical to crimson | Same; `neon` was nearly identical to `crimson` in older builds | Update — neon now differs in 10 high-visibility fields |
| `/theme` reverts my `theme.toml` overrides | Old build | Update — overrides are re-applied on every switch |
| Borders show as `+--+` instead of `╭──╮` | `HONE_ASCII_ONLY=1` is set, or terminal lacks UTF-8 | Unset the env var; check `LANG` ends in `.UTF-8` |
| Spinners not animating | `HONE_REDUCED_MOTION=1` | Unset to restore animation |

## Related

- [`docs/tutorials/getting-started.md`](getting-started.md) — first session walkthrough.
- [`docs/tui-design-spec.md`](../tui-design-spec.md) — design tokens, layout grammar, and the full theme palette reference.
