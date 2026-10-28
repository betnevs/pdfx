#!/usr/bin/env python3
"""Generate the 1024x1024 PDFX app-icon source without third-party packages.

The output is committed as assets/app-icon.png; run `npx tauri icon` afterwards
to derive the platform icon set in src-tauri/icons/.
"""

from __future__ import annotations

import struct
import sys
import zlib
from pathlib import Path

SIZE = 1024
SUPERSAMPLE = 2
INK = (31, 36, 48, 255)
PAGE = (250, 249, 246, 255)
ACCENT = (232, 69, 60, 255)
TRANSPARENT = (0, 0, 0, 0)


def rounded_rect_contains(x: float, y: float, x0: float, y0: float, x1: float, y1: float, radius: float) -> bool:
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    cx = min(max(x, x0 + radius), x1 - radius)
    cy = min(max(y, y0 + radius), y1 - radius)
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius**2


def polygon_contains(x: float, y: float, points: list[tuple[float, float]]) -> bool:
    inside = False
    j = len(points) - 1
    for i, (xi, yi) in enumerate(points):
        xj, yj = points[j]
        if (yi > y) != (yj > y) and x < (xj - xi) * (y - yi) / (yj - yi) + xi:
            inside = not inside
        j = i
    return inside


def sample(x: float, y: float) -> tuple[int, int, int, int]:
    # Soft outer rounded square.
    if not rounded_rect_contains(x, y, 64, 64, 960, 960, 184):
        return TRANSPARENT

    # White sheet with a folded top-right corner.
    fold = [(724, 250), (724, 408), (566, 250)]
    if rounded_rect_contains(x, y, 300, 250, 724, 790, 18) or polygon_contains(x, y, fold):
        if polygon_contains(x, y, fold):
            return ACCENT
        return PAGE

    # Accent bar at the bottom of the sheet: a simple "text" rhythm.
    if rounded_rect_contains(x, y, 372, 618, 652, 652, 17):
        return ACCENT
    if rounded_rect_contains(x, y, 372, 686, 584, 720, 17):
        return ACCENT

    return INK


def render() -> bytearray:
    pixels = bytearray()
    step = 1.0 / SUPERSAMPLE
    for py in range(SIZE):
        for px in range(SIZE):
            r = g = b = a = 0
            for sy in range(SUPERSAMPLE):
                for sx in range(SUPERSAMPLE):
                    sample_color = sample(px + (sx + 0.5) * step, py + (sy + 0.5) * step)
                    r += sample_color[0]
                    g += sample_color[1]
                    b += sample_color[2]
                    a += sample_color[3]
            count = SUPERSAMPLE * SUPERSAMPLE
            pixels.extend((r // count, g // count, b // count, a // count))
    return pixels


def write_png(path: Path, width: int, height: int, rgba: bytearray) -> None:
    def chunk(kind: bytes, payload: bytes) -> bytes:
        body = kind + payload
        return struct.pack(">I", len(payload)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = bytearray()
    stride = width * 4
    for row in range(height):
        raw.append(0)  # filter: none
        raw.extend(rgba[row * stride : (row + 1) * stride])

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)


def main() -> None:
    output = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("assets/app-icon.png")
    write_png(output, SIZE, SIZE, render())
    print(f"wrote {output} ({output.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
