# `hone-webui` — SolidJS browser UI

Phase 0 scaffold of the SolidJS web UI from `priv/webui-design.md`.
Served by `honed` at `/ui` via `rust-embed` (the build output lands in
`apps/webui/dist/` and is compiled into the daemon binary).

## What's in Phase 3B (this drop)

- **Tunnel section** in SettingsPanel — start / stop / status against
  the existing `POST /tunnel`, `DELETE /tunnel`, `GET /tunnel`
  endpoints. Public URL displayed in a monospace box with a Copy
  button (clipboard API).
- **Team usage section** in SettingsPanel — read-only display of
  `GET /team/usage`. Shows total tokens + USD plus a per-model
  breakdown. Refresh button.
- **`<CollaborativePanel>`** — new topbar dropdown next to the agent
  selector. Lists participants of the active session via
  `GET /sessions/{id}/participants`. Join with display name (POST
  `/join`), Leave (POST `/leave`). Owners get a Kick button per
  other participant (POST `/kick`). Display name + per-browser
  user_id persisted to localStorage.
- All four daemon surfaces in this PR are read/wrote against
  endpoints that already exist (tunnel, governance, collaborative).
  Zero daemon changes.
- Bundle: 23.75 KB JS gzipped initial (was 21.74).

## What's in Phase 3A

- New **\`<SettingsPanel>\`** slide-in drawer (right-docked, opens on
  ⚙ topbar button or \`/settings\` slash command). Theme picker
  grid, font-size selector (small / medium / large), daemon-endpoint
  override input (persisted but not yet read by the fetch helpers —
  next slice), and a Reset button that wipes settings without
  touching sessions or chat history.
- Settings persisted to \`localStorage\` via \`lib/settings.ts\`;
  applied on first paint via a tiny `applyToDocument` shim from
  \`main.tsx\` so the layout doesn't flash to defaults.
- New PWA **\`manifest.json\`** with name, icons, start_url \`/ui/\`,
  scope \`/ui/\`, and the brand colors. \`apple-mobile-web-app-*\`
  meta tags so iOS Safari "Add to Home Screen" picks the right
  display + name.
- New **service worker** at \`/ui/sw.js\` (registered only in
  production builds — dev keeps Vite's HMR untouched). Cache-first
  for hashed \`/ui/assets/*\`, stale-while-revalidate for the SPA
  shell, no caching for the API (so SSE / POST always go straight
  to the daemon).
- New \`/settings\` slash command opens the drawer.
- Bundle: 21.74 KB JS gzipped initial chunk (was 20 KB in Phase 2C).

## What's in Phase 2C

- **Lazy Shiki syntax highlighting** in `<CodeViewer>` (whole files
  picked up via the file tree or auto-opened by tool calls) and in
  `<MarkdownView>` fenced code blocks.
- `lib/highlight.ts` uses `createHighlighterCore` + dynamic
  per-language imports rather than the full `createHighlighter`
  bundle — only the languages we whitelist (rust, ts/js/tsx/jsx,
  python, go, ruby, java, c/cpp/csharp, html/css/scss, json/yaml/toml/xml,
  markdown, bash, sql, graphql, diff, dockerfile, makefile) ship in
  the build.
- Two themes wired: `github-dark-default` for crimson/neon, `github-light`
  for chalk/minimal. Theme picked at highlight time from the
  `[data-theme]` attribute on `<html>`.
- Bundle:
  - **Initial chunk: 20 KB JS + 2 KB CSS gzipped** (was 53 KB in
    Phase 2B; the Shiki dynamic import nudged vite to split more
    aggressively across the rest of the app too).
  - On first highlight: Shiki core (~53 KB gz) + Oniguruma WASM
    (~231 KB gz) + the language chunk (e.g. typescript = 17 KB gz,
    rust = 3 KB gz). Cached after first fetch.
  - Total dist directory: 3.1 MB (was 9.5 MB before the
    `createHighlighterCore` refactor).

## What's in Phase 2B

- **Resizable split-pane gutter** — 6px draggable bar between chat
  and preview. Updates a persisted `--split-ratio` (clamped to
  [0.25, 0.85], stored in localStorage so the layout survives reload).
- **`<SideBySideDiff>`** — in-house unified-diff parser
  (`lib/diff.ts`, no dep). Tool-call results that look like diffs
  render as side-by-side hunks with theme add/remove/hunk colors.
  Falls back to the existing collapsible monospace pre when the
  result isn't a diff.
- **`<WorkerProgressPanel>`** — top-of-chat banner that surfaces the
  orchestrator events the SSE stream already emits
  (`plan_created` / `worker_started` / `worker_completed` /
  `budget_progress`). Per-worker status dot pulses while running,
  flips to green / red on completion. Auto-hides when no
  orchestration is in flight; resets at the start of every new turn.
- Bundle: 53.28 KB JS + 6.70 KB CSS gzipped (was 43.95 + 6.59).
  Shiki syntax highlighting still deferred — its lazy-WASM bundle
  impact needs its own PR.

## What's in Phase 2A

- New honed endpoints `GET /files?path=<rel>` (directory listing,
  workspace-scoped, hidden entries skipped) and
  `GET /files/content?path=<rel>` (text-only file read, 2 MiB cap,
  binary detection via NUL byte sniff). Path traversal rejected with
  400 BAD REQUEST.
- New `<FilePreviewPanel>` docked to the right of the chat. Toggle on
  / off via the topbar `preview` button. Two columns: a lazy file
  tree (left) and a `<CodeViewer>` (right) with line numbers.
- Tool calls that name a path (`read_file`, `edit_file`, `write_file`)
  auto-open that file in the preview pane.
- 4 unit tests + 7 integration tests for the file endpoints
  (listing / hidden / traversal / read / binary / oversized /
  not-a-file).

## What's in Phase 1B

- `<SessionSelector>` in the topbar: lists local sessions (created /
  last_used / first-prompt-as-title), switch with one click,
  ✕ deletes both the local bucket and the daemon's session record.
  New sessions mint a UUID client-side and are persisted to IndexedDB.
- IndexedDB persistence for messages — `lib/db.ts` is a tiny
  Promise-based wrapper (no dep). Each session id maps to an array of
  `PersistedMessage`s. On session switch the chat panel hydrates from
  IDB; on every milestone event (tool start/end, done, error) it
  saves back. Survives page refresh.
- New `<ApprovalModal>` — when **step** is enabled in the topbar,
  every `tool_call_paused` (and `approval_required`) event pops a
  centred modal with `Allow` / `Always allow` / `Skip` / `Deny`
  buttons. POSTs the chosen `StepAction` to
  `POST /run/{request_id}/step`.
- New honed endpoint `GET /sessions` returns the list of session
  summaries (id + created_at + last_used). 2 integration tests.

## What's in Phase 1 (slice A)

Builds on Phase 0 with:
- All four themes from the design doc's 35-token palette: crimson,
  chalk, neon, minimal. Topbar button cycles through them.
- Agent selector — dropdown driven by `GET /agents` showing every
  built-in / user / project agent with name, source pill, description.
- Slash-command autocomplete in the input bar: `/theme <name>`,
  `/clear` (reset cost meter), `/help`. Commands are intercepted
  client-side and never hit `/run`.
- Cost / token meter in the StatusBar — accumulates `done` events.
- Markdown rendering in assistant bubbles: fenced code blocks
  (with lang label), inline code, bold, italic. In-house parser
  (~50 LoC, no dep) — Phase 2 will swap in Shiki for syntax
  highlighting.
- Collapsible tool-call cards: long output (>6 lines) collapses by
  default with a "show all" button. JSON results pretty-print.

Bundle: **30 KB JS + 6.5 KB CSS gzipped** (still well under the
200 KB budget).

## What's in Phase 0

- Project scaffolding: Vite 6, SolidJS, TypeScript strict, UnoCSS.
- TopBar with daemon health badge + theme toggle (crimson / chalk).
- ChatPanel with streaming `POST /run` consumer (SSE parsed via
  `fetch` + `ReadableStream` since `EventSource` only supports GET).
- StatusBar.
- Two themes from the design doc's 35-token palette (crimson, chalk).
- Vite dev proxy for every API endpoint honed exposes.

**Bundle size:** ~22 KB JS + ~5 KB CSS gzipped — well within the design
doc's 200 KB budget.

## Layout

```
apps/webui/
├── README.md          (this file)
├── package.json       (npm; pnpm-compatible)
├── tsconfig.json      (TS strict, JSX → SolidJS)
├── vite.config.ts     (base: /ui/, dev proxy to :3117)
├── uno.config.ts      (preset-mini only)
├── index.html
├── public/
│   └── favicon.svg
├── src/
│   ├── main.tsx       (SolidJS root mount)
│   ├── App.tsx        (top-level layout + theme)
│   ├── lib/
│   │   ├── sse.ts     (streaming POST + SSE frame parser)
│   │   └── api.ts     (fetch helper)
│   ├── types/
│   │   └── events.ts  (AgentEvent discriminated union)
│   ├── components/
│   │   ├── TopBar.tsx
│   │   ├── StatusBar.tsx
│   │   └── ChatPanel.tsx
│   └── styles/
│       ├── base.css
│       └── themes/
│           ├── crimson.css
│           └── chalk.css
└── dist/              (build output — gitignored, populated by `make webui`)
```

## Develop

```bash
# One-time install + production build (writes apps/webui/dist/).
make webui

# Hot-reload dev server at http://127.0.0.1:5174 with API proxy to :3117.
make webui-dev
```

The dev server's Vite proxy forwards `/run`, `/sessions`, `/health`,
`/agents`, `/recipes`, `/memory`, `/provenance`, `/evals`, `/sandbox`,
and `/mcp` to a locally-running `honed` at `127.0.0.1:3117`. Boot the
daemon in a second terminal before iterating:

```bash
ANTHROPIC_API_KEY=sk-... cargo run -p honed --release
```

## Production

The `dist/` output is embedded into the `honed` binary at compile time
via `rust-embed`. After `make webui`, rebuild the daemon:

```bash
make webui
cargo build --release -p honed
```

Then start `honed` and visit `http://127.0.0.1:3117/ui` in a browser.

If you're cloning fresh and want the daemon to compile *without*
running `make webui`, the `hone-server` build.rs writes a tiny stub
`dist/index.html` that explains how to populate the real bundle. The
daemon still starts and the TUI/CLI are unaffected.

## What's intentionally missing in Phase 0

- File preview / code viewer / diff renderer (Phase 2 of the design doc)
- Approval modal / worker progress / settings drawer (Phase 1 cont.)
- Slash-command autocomplete
- Session list + persistence (IndexedDB)
- All themes beyond crimson + chalk
- PWA manifest + service worker

Phase 0 ships the toolchain + the streaming-chat foundation so future
phases can iterate on a working surface rather than designing in a
vacuum.
