# `hone-webui` — SolidJS browser UI

Phase 0 scaffold of the SolidJS web UI from `priv/webui-design.md`.
Served by `honed` at `/ui` via `rust-embed` (the build output lands in
`apps/webui/dist/` and is compiled into the daemon binary).

## What's in Phase 1 (this drop)

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
