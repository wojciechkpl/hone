# `hone-desktop` — Tauri 2 shell for `honed`

Phase 1 of the GUI roadmap (see `priv/hone-gui-design-2026-05.md`).

This is a **placeholder shell**: it spawns or connects to `honed`, displays
its health, and exposes a tiny command palette + system tray. The full
SolidJS web UI lands in a later phase and will be loaded into this same
shell.

## Layout

```
apps/hone-desktop/
├── README.md          (this file)
├── dist/              static frontend (vanilla HTML/JS/CSS — no Node toolchain)
│   ├── index.html
│   ├── style.css
│   └── app.js
└── src-tauri/         Tauri 2 Rust app
    ├── Cargo.toml
    ├── tauri.conf.json
    ├── build.rs
    ├── capabilities/
    │   └── default.json
    ├── icons/         (placeholder PNGs — replace before bundling)
    └── src/
        ├── main.rs
        ├── lib.rs
        ├── commands.rs    Tauri commands invoked from JS
        ├── config.rs      ~/.config/hone/desktop.json
        ├── supervisor.rs  honed sidecar spawn / health / shutdown
        ├── sse.rs         SSE frame parser + buffered framer
        ├── runtime.rs     in-flight `POST /run` consumer + event router
        └── tray.rs        system tray menu
```

## Prerequisites

The Tauri 2 toolchain needs:

- A C toolchain (Xcode CLT on macOS, MSVC build tools on Windows, gcc on Linux).
- Rust 1.85+ (already required by the workspace).
- Linux only: `webkit2gtk-4.1` and `libayatana-appindicator3` development packages.
- Optionally `cargo install tauri-cli@^2` for the `cargo tauri` subcommand.

No Node.js / npm is required for v0.0.1 — the frontend is plain static
HTML/CSS/JS in `dist/`. When the SolidJS web UI is added later, a Node
toolchain becomes a dev dependency for *building* the frontend, but the
final bundle is still served from `dist/`.

## Develop

From the workspace root:

```bash
# build only this crate
cargo check -p hone-desktop

# unit tests (no GUI launched)
cargo test -p hone-desktop --lib

# run the actual app
cargo run -p hone-desktop
```

If you have `tauri-cli` installed:

```bash
cd apps/hone-desktop/src-tauri
cargo tauri dev
```

The wizard on first launch picks one of two daemon-discovery modes:

- **Managed sidecar** — the app spawns `honed` on a free local port and
  supervises it. Requires `honed` to be on `PATH`. Build it with
  `cargo build -p honed --release` and put the binary somewhere on `PATH`,
  or symlink `target/release/honed`.
- **External daemon** — the app speaks HTTP to a `honed` you run elsewhere
  (e.g. on a Linux dev VM). You provide URL + bearer token.

The chosen mode is persisted to `~/.config/hone/desktop.json`. Wipe that
file (or use the `Cmd+K` palette → "Reset onboarding") to re-show the wizard.

## Tray menu

- **Show Hone** — bring the window forward
- **Hide window** — hide (the daemon keeps running)
- **Stop honed sidecar** — terminate the supervised child (no-op in external mode)
- **Quit Hone** — best-effort sidecar shutdown then exit

The window's close button hides rather than quits, so the tray remains the
single source of "is the daemon still running."

## Bundling

```bash
cd apps/hone-desktop/src-tauri
cargo tauri build
```

Produces a platform-native installer in `target/release/bundle/`. The
icons in `src-tauri/icons/` are placeholder transparent PNGs; replace them
with real artwork before shipping. `cargo tauri icon path/to/source.png`
generates the full set from a single 1024×1024 source.

## Phase 2 status

**Slice A (shipped):**
- Streaming chat surface — `POST /run` SSE consumer, assistant bubble,
  inline tool-call cards.
- Cost / token meter in the topbar.
- `run_turn` / `run_cancel` Tauri commands.

**Slice B.1 (this drop) — approval queue, end-to-end:**
- New honed endpoint `POST /run/{request_id}/step` accepting a
  `StepAction` JSON body. Returns 202 / 404 / 410 / 400.
- `RunRequest` gained `request_id` and `approval_policy` fields. When
  `approval_policy != Auto`, the agent loop runs in step-mode; the GUI's
  `request_id` is registered server-side so subsequent `/step` posts
  reach the right agent.
- GUI: "Step-mode" toggle in the sidebar. When on, requests carry
  `approval_policy: { AskRisky: { always_allowed: [] } }`.
- GUI: approval queue pane lists `tool_call_paused` and `approval_required`
  events; each card has Approve / Approve all / Skip / Deny buttons that
  POST a `StepAction` via the new `run_step` Tauri command.

**Phase 5 slice B (this drop) — artifact pane:**
- New Tauri command `reveal_in_finder(path)` — opens the OS file
  manager at `path` (macOS `open -R`, Windows `explorer /select,`,
  Linux `xdg-open` on parent dir). Rejects null bytes + nonexistent
  paths.
- GUI: collapsible **Artifacts** card in the sidebar tracks every
  `write_file` / `edit_file` / `read_file` tool call. Each row shows
  tool pill + path + call count + relative timestamp + a **Reveal**
  button. Clear button wipes the session list.
- Implementation is JS-only on the consumer side; ties into the
  existing `tool_call_start` / `tool_call_end` event flow by stashing
  the `args` at start time so `end` can resolve the path.

**Phase 5 slice A — multi-daemon switcher:**
- `DesktopConfig` extended with a list of named `DaemonProfile`s plus
  an `active` pointer. Legacy single-daemon configs auto-migrate to a
  profile named `local sidecar` (or `external`) on load.
- New Tauri commands: `list_daemons`, `add_daemon`, `remove_daemon`,
  `switch_daemon`. Switch tears down the previously active sidecar
  (if any) and seeds the external slot for HTTP-mode profiles.
- GUI: topbar shows `<active-name> · <mode> @ <url>` with a clickable
  caret that opens a profile dropdown — switch / remove / add external.
- 12 new config + command tests cover migration idempotency,
  add/remove/switch invariants, and the internally-tagged wire shape
  for `DaemonProfile`.

**Phase 4 slice D — eval-run dashboard:**
- New honed endpoints `GET /evals?limit=N&tag=...` (returns recent
  `RunSummary` rows) and `GET /evals/{id}` (returns full
  `EvalRunRecord` with per-task outcomes). Both 503 when no eval store
  configured.
- AppState gained `eval_history: Option<Arc<EvalRunStore>>`. honed
  opens a `SqliteStore` at `HONE_EVAL_HISTORY_DB` on startup.
- GUI: collapsible **Evals** card with tag-filter input. Each row
  shows model + tag pill + colored pass/total + visual pass-bar +
  timestamp + commit. Click expands per-task outcomes (status dot,
  duration_ms, cost, tokens) plus a total cost footer.

**Phase 4 (Differentiators) is now complete.** All four feature areas
from the design doc ship with daemon endpoint + GUI surface + tests.

**Phase 4 slice C — provenance hash-chain explorer:**
- New honed endpoints `GET /provenance` (lists `*.jsonl` files under
  `HONE_PROVENANCE_DIR`), `GET /provenance/{id}` (returns full chain),
  and `GET /provenance/{id}/verify` (returns `{ valid, broken_at,
  reason, merkle_root, entry_count }`).
- All return 503 when no provenance dir is configured. Path-traversal
  in `{id}` is rejected with 400.
- GUI: collapsible **Provenance** card listing chain files. Click an
  entry to expand: green ✓ banner + Merkle root for valid chains,
  red ✗ banner pinpointing the broken index for tampered ones, plus
  the first 200 entries (kind + summary).

**Phase 4 slice B — memory browser:**
- New honed endpoints `GET /memory` (returns up to N memories, optionally
  filtered by `?q=` substring or `?kind=`) and `GET /memory/stats`
  (returns `{ total, by_kind, top_tags }`). Both 503 when no store
  configured.
- AppState gained `memory_store: Option<Arc<dyn MemoryStore>>`. honed
  opens a `SqliteStore` at `HONE_MEMORY_DB` on startup; absent var means
  `/memory*` returns 503 (read-only viewer).
- GUI: collapsible **Memory** card with search box, stats summary
  (total + per-kind breakdown + top tags as chips), and a scrollable
  list of memory rows (kind pill, content preview, weight + tags).

**Phase 4 slice A — sandbox policy viewer (read-only):**
- New honed endpoint `GET /sandbox/policies` returns the active
  server-wide policy (currently `SandboxPolicy::permissive()` wired
  into the agent path) plus every per-agent override declared via the
  `sandbox:` block in agent frontmatter.
- `SandboxPolicyWire` mirrors `hone_sandbox::SandboxPolicy` since the
  underlying type doesn't derive `Serialize`. The active policy carries
  a human-readable `description`; per-agent overrides are bare.
- GUI: collapsible **Sandbox** card showing the active policy as a
  rule table (network on/off, allow_read/write/exec/domains/env counts)
  plus a row per agent override (mode + path counts + network flag).
  Read-only — editor lands in slice A.2 once the sandbox builder gains
  serde support.

**Phase 3 slice C — MCP marketplace + installed list:**
- New honed endpoints `GET /mcp/marketplace` (returns the bundled
  `Marketplace::builtin()` catalog of curated servers — filesystem,
  github, git, slack, puppeteer, sqlite, brave-search) and
  `GET /mcp/installed` (merges global `~/.config/hone/mcp.json` with
  project `.hone/mcp.json`, tagging each entry with its source layer).
- GUI: two new sections inside the Discover card. Installed shows
  command + args inline; marketplace entries are click-to-expand and
  carry an Install button that writes the entry to the project's
  `.hone/mcp.json`.
- New Tauri command `mcp_install` preserves sibling entries when adding
  to an existing config, refuses to overwrite without `force`, rejects
  path-traversal in ids.

**Phase 3 slice B — detail expand + Run/Fork actions:**
- New honed endpoints `GET /agents/{name}` and `GET /recipes/{name}`
  return full detail (system prompt for agents, all steps for recipes).
- GUI: clicking a Discover entry expands inline with the detail. Recipe
  expansion shows every step + a "Use first step as prompt" button that
  populates the chat input. Agent expansion shows the system prompt and,
  for non-project entries, a "Fork to .hone/agents/" button.
- New Tauri command `agent_fork` writes `<project_root>/.hone/agents/<name>.md`
  reconstructed from the detail JSON. Refuses to overwrite without
  `force=true`. Rejects path-traversal in agent names.

**Phase 3 slice A — discover pane (read-only):**
- New honed endpoints `GET /agents` and `GET /recipes` returning JSON
  arrays. Each entry has a `source` tag (`builtin` / `user` / `project`).
- GUI: collapsible **Discover** card in the sidebar with two sections.
  Each entry shows name + source pill + description + meta line
  (model/tool count for agents; step count + `test_cmd` flag for recipes).
- Read-only — running a recipe / forking a builtin agent is the next slice.

**Phase 2 slice B.2 — multi-agent orchestrate + task tree:**
- New honed endpoint `POST /orchestrate` that runs the multi-agent
  `Orchestrator` (decompose → execute → merge) and streams the same SSE
  shape as `/run` plus `plan_created` / `worker_started` /
  `worker_completed` / `budget_progress`.
- `OrchestrateRequest` accepts `prompt`, optional `model`, `history`,
  `working_dir`, `max_workers` (default 4), `total_budget_usd`.
- GUI: "Orchestrate (multi-agent)" toggle next to the task-tree pane.
  When checked, the prompt is dispatched via the new `run_orchestrate`
  Tauri command instead of `run_turn`.
- GUI: live task-tree shows the goal, one task node per worker with a
  status icon (queued/running/completed/failed), budget bar at the
  bottom. Worker nodes pulse while running.

Beyond Phase 2: real chat UI styling (will move to the SolidJS web UI),
auto-update, real icon artwork, in-process embed of `honed`.

## Architecture invariants

- The frontend never touches the filesystem directly — everything goes
  through Tauri commands in `commands.rs`.
- The supervisor is the only thing that spawns child processes.
- Persistent preferences live in a single JSON at the OS-standard config
  directory — never in `localStorage` (which `dist/app.js` *does* use, but
  only for ephemeral UI state like "have I seen the wizard").
- All async commands drop locks before `.await` (Tauri 2 requires futures
  be `Send`; `parking_lot::MutexGuard` is `!Send`).
