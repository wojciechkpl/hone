# Icons

Hone monogram icons. The 1024×1024 `icon.png` is the canonical source —
release CI runs `cargo tauri icon icons/icon.png` to derive the platform
formats `.icns` (macOS) and `.ico` (Windows) at bundle time.

Files:

- `icon.png` — 1024×1024 RGBA, the canonical source.
- `32x32.png`, `128x128.png`, `128x128@2x.png` — derivatives bundled
  with the app on macOS / Linux / generic platforms.
- `tray-icon.png` — small monochrome tray icon (template-rendered on
  macOS for proper menu-bar contrast).

The artwork is a stylized "H" in the brand accent (`#5ac8fa`) on the
dark card background (`#15171c`) with a rounded-square mask. It's
intentionally simple — readable at 16×16 and signals "developer tool"
without competing with whatever filesystem icon theme the user runs.

## Regenerating

The icons are produced by a tiny Python script in
`scripts/generate_icons.py` (writing PNGs by hand with `zlib`+`struct`
so no PIL/imagemagick dep is required). Run:

```bash
python3 scripts/generate_icons.py
```

For platform-specific artefacts (macOS `.icns`, Windows `.ico`), install
`tauri-cli` and run from the workspace root:

```bash
cd apps/hone-desktop/src-tauri
cargo tauri icon icons/icon.png
```

That writes `icon.icns` and `icon.ico` alongside the PNG derivatives.
