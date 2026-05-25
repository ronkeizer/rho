#!/usr/bin/env python3
"""Generate the Rho app icon (assets/icon.png) from scratch — no third-party
deps, just the stdlib (zlib + struct for PNG encoding).

The mark is a stylized lowercase Greek rho (ρ): a ring (the bowl) with a
vertical descender stem on its lower-left, in white on a rounded indigo
square. Edges are anti-aliased via signed-distance fields, so a single
sample per pixel is enough and the whole thing renders in well under a
second of pure Python.

Run from the repo root:  python3 scripts/gen-icon.py
Then build the iconset / .icns with scripts/make-macos-app.sh.
"""

import math
import os
import struct
import zlib

SIZE = 1024  # master resolution; downscaled by sips for the iconset.

# Palette (sRGB 0-255).
BG_TOP = (99, 102, 241)     # indigo-500
BG_BOT = (67, 56, 202)      # indigo-700
GLYPH = (255, 255, 255)

# Rounded-square background geometry.
MARGIN = 80
RRECT_R = 200

# Glyph geometry (in master pixels).
BOWL_CX, BOWL_CY = 540, 430
BOWL_OUTER, BOWL_INNER = 250, 140
STEM_CX = 318
STEM_HALF_W = 56
STEM_TOP, STEM_BOT = 420, 868


def clamp01(x):
    return 0.0 if x < 0.0 else (1.0 if x > 1.0 else x)


def sdf_rrect(px, py, cx, cy, hx, hy, r):
    """Signed distance to a rounded rectangle (negative inside)."""
    dx = abs(px - cx) - (hx - r)
    dy = abs(py - cy) - (hy - r)
    ox, oy = max(dx, 0.0), max(dy, 0.0)
    return math.hypot(ox, oy) + min(max(dx, dy), 0.0) - r


def coverage(dist):
    """AA coverage from a signed distance: ~1px transition band."""
    return clamp01(0.5 - dist)


def render():
    half = SIZE / 2.0
    bg_hx = bg_hy = half - MARGIN
    bowl_mid = (BOWL_OUTER + BOWL_INNER) / 2.0
    bowl_half_thick = (BOWL_OUTER - BOWL_INNER) / 2.0
    stem_cy = (STEM_TOP + STEM_BOT) / 2.0
    stem_hy = (STEM_BOT - STEM_TOP) / 2.0

    rows = []
    for y in range(SIZE):
        row = bytearray()
        for x in range(SIZE):
            px, py = x + 0.5, y + 0.5

            # Background rounded square (alpha + vertical gradient).
            bg_d = sdf_rrect(px, py, half, half, bg_hx, bg_hy, RRECT_R)
            bg_a = coverage(bg_d)
            t = clamp01((py - MARGIN) / (SIZE - 2 * MARGIN))
            br = round(BG_TOP[0] + (BG_BOT[0] - BG_TOP[0]) * t)
            bg = round(BG_TOP[1] + (BG_BOT[1] - BG_TOP[1]) * t)
            bb = round(BG_TOP[2] + (BG_BOT[2] - BG_TOP[2]) * t)

            # Glyph = ring (bowl) ∪ stem.
            ring_d = abs(math.hypot(px - BOWL_CX, py - BOWL_CY) - bowl_mid) - bowl_half_thick
            stem_d = sdf_rrect(px, py, STEM_CX, stem_cy, STEM_HALF_W, stem_hy, STEM_HALF_W)
            glyph_a = coverage(min(ring_d, stem_d))

            # Composite glyph over background; alpha is the background mask.
            r = round(br + (GLYPH[0] - br) * glyph_a)
            g = round(bg + (GLYPH[1] - bg) * glyph_a)
            b = round(bb + (GLYPH[2] - bb) * glyph_a)
            a = round(255 * bg_a)
            row += bytes((r, g, b, a))
        rows.append(bytes(row))
    return rows


def write_png(path, rows):
    raw = bytearray()
    for row in rows:
        raw.append(0)  # filter type 0 (None)
        raw += row

    def chunk(tag, data):
        out = struct.pack(">I", len(data)) + tag + data
        return out + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", ihdr)
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


if __name__ == "__main__":
    here = os.path.dirname(os.path.abspath(__file__))
    out = os.path.join(here, "..", "assets", "icon.png")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    write_png(out, render())
    print("wrote", os.path.normpath(out))
