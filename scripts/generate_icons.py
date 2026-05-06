#!/usr/bin/env python3
"""Generate the Hone desktop app icons.

Pure-stdlib PNG writer (zlib + struct) so this script has no third-party
dependency. Re-run when the brand palette or monogram changes; commit
the resulting PNGs alongside the app.

Outputs into apps/hone-desktop/src-tauri/icons/:
  - icon.png         (1024x1024 canonical source)
  - 32x32.png
  - 128x128.png
  - 128x128@2x.png
  - tray-icon.png    (32x32 monochrome H on transparent)

For the macOS `.icns` / Windows `.ico` bundle artefacts, run
`cargo tauri icon icons/icon.png` from `apps/hone-desktop/src-tauri/`
after this script.
"""

from __future__ import annotations

import os
import struct
import zlib

ACCENT = (0x5A, 0xC8, 0xFA, 0xFF)  # brand accent (sky blue)
BODY = (0x15, 0x17, 0x1C, 0xFF)    # icon body (dark card)
EDGE = (0x2A, 0x2D, 0x36, 0xFF)    # rounded-edge highlight
TRANSP = (0, 0, 0, 0)


def png(w: int, h: int, pixels):
    sig = b"\x89PNG\r\n\x1a\n"

    def chunk(t: bytes, d: bytes) -> bytes:
        crc = zlib.crc32(t + d) & 0xFFFFFFFF
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", crc)

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)  # 8-bit RGBA
    raw = b""
    for row in pixels:
        raw += b"\x00" + bytes(row)
    idat = zlib.compress(raw, 9)
    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def render_logo(size: int):
    w = h = size
    radius = size * 0.22
    cx, cy = (w - 1) / 2.0, (h - 1) / 2.0
    bar_w = max(1, int(size * 0.10))
    h_w = int(size * 0.42)
    h_h = int(size * 0.50)
    h_left = int(cx - h_w / 2)
    h_right = int(cx + h_w / 2)
    h_top = int(cy - h_h / 2)
    h_bot = int(cy + h_h / 2)
    cross_top = int(cy - bar_w / 2)
    cross_bot = int(cy + bar_w / 2)
    margin = max(1, int(size * 0.03))
    x0, y0 = margin, margin
    x1, y1 = w - margin, h - margin

    pixels = []
    for y in range(h):
        row = bytearray(w * 4)
        for x in range(w):
            in_body = False
            in_edge = False
            if x0 <= x <= x1 and y0 <= y <= y1:
                dx = max(x0 + radius - x, 0, x - (x1 - radius))
                dy = max(y0 + radius - y, 0, y - (y1 - radius))
                d = (dx * dx + dy * dy) ** 0.5
                if d <= radius:
                    in_body = True
                    if d > radius - 1.5:
                        in_edge = True
            color = TRANSP
            if in_body:
                left_post = h_left <= x <= h_left + bar_w and h_top <= y <= h_bot
                right_post = h_right - bar_w <= x <= h_right and h_top <= y <= h_bot
                crossbar = h_left <= x <= h_right and cross_top <= y <= cross_bot
                if left_post or right_post or crossbar:
                    color = ACCENT
                else:
                    color = EDGE if in_edge else BODY
            row[x * 4 : (x + 1) * 4] = bytes(color)
        pixels.append(row)
    return pixels


def render_tray(size: int):
    w = h = size
    cx, cy = (w - 1) / 2.0, (h - 1) / 2.0
    bar_w = max(1, int(size * 0.16))
    h_w = int(size * 0.55)
    h_h = int(size * 0.65)
    h_left = int(cx - h_w / 2)
    h_right = int(cx + h_w / 2)
    h_top = int(cy - h_h / 2)
    h_bot = int(cy + h_h / 2)
    cross_top = int(cy - bar_w / 2)
    cross_bot = int(cy + bar_w / 2)
    pixels = []
    for y in range(h):
        row = bytearray(w * 4)
        for x in range(w):
            left_post = h_left <= x <= h_left + bar_w and h_top <= y <= h_bot
            right_post = h_right - bar_w <= x <= h_right and h_top <= y <= h_bot
            crossbar = h_left <= x <= h_right and cross_top <= y <= cross_bot
            if left_post or right_post or crossbar:
                row[x * 4 : (x + 1) * 4] = bytes((255, 255, 255, 255))
        pixels.append(row)
    return pixels


def main() -> int:
    root = os.path.join(
        os.path.dirname(os.path.abspath(__file__)),
        "..",
        "apps",
        "hone-desktop",
        "src-tauri",
        "icons",
    )
    root = os.path.normpath(root)
    os.makedirs(root, exist_ok=True)

    for name, size in [
        ("32x32.png", 32),
        ("128x128.png", 128),
        ("128x128@2x.png", 256),
        ("icon.png", 1024),
    ]:
        with open(os.path.join(root, name), "wb") as f:
            f.write(png(size, size, render_logo(size)))
    with open(os.path.join(root, "tray-icon.png"), "wb") as f:
        f.write(png(32, 32, render_tray(32)))

    print(f"icons written to {root}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
