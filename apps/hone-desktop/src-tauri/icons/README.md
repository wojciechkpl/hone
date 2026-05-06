# Icons

Placeholder directory. Drop the following before bundling:

- `32x32.png`, `128x128.png`, `128x128@2x.png` — generic PNG icons
- `icon.icns` — macOS bundle icon
- `icon.ico` — Windows bundle icon
- `tray-icon.png` — small monochrome tray icon (template-rendered on macOS)

Tauri's `icon` CLI can generate the full set from a single 1024×1024 PNG:

    cargo tauri icon path/to/source.png

For v0.0.1 development the bundler is not invoked (`cargo run` only loads
the dev window), so missing icons do not block local iteration.
