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

## What's intentionally NOT here yet

- **The chat UI.** That ships with the SolidJS web UI; this shell will
  load it from `dist/` when ready.
- **SSE consumption.** Approval-pending notifications are wired in Phase 2
  alongside the streaming-chat surface.
- **Cross-IPC into hone crates.** The supervisor still talks to `honed`
  over HTTP. A future optimisation is to embed the daemon in-process via
  Tauri-managed state, but the HTTP path keeps the GUI honest about the
  same wire format the TUI / web UI use.
- **Auto-update.** Tauri 2 supports it; we wire it once the marketing
  site has a signed release feed.

## Architecture invariants

- The frontend never touches the filesystem directly — everything goes
  through Tauri commands in `commands.rs`.
- The supervisor is the only thing that spawns child processes.
- Persistent preferences live in a single JSON at the OS-standard config
  directory — never in `localStorage` (which `dist/app.js` *does* use, but
  only for ephemeral UI state like "have I seen the wizard").
- All async commands drop locks before `.await` (Tauri 2 requires futures
  be `Send`; `parking_lot::MutexGuard` is `!Send`).
