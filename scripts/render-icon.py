#!/usr/bin/env python3
"""Render assets/icon.svg and assets/icon.png (1024x1024) from one shape list.

Standard library only: shapes are rasterised with analytic signed distances.
"""

import math
import struct
import zlib
from pathlib import Path

SIZE = 1024
CENTER = SIZE / 2
# Apple icon grid: an 824px body leaves a 100px transparent margin.
BODY_HALF = 412.0
SQUIRCLE_N = 5.0

TEAL = (0x2D, 0xD4, 0xBF)
BLUE = (0x25, 0x63, 0xEB)
WHITE = (0xFF, 0xFF, 0xFF)

# (kind, geometry, color, opacity)
GLYPH = [
    ("circle", (300, 318, 24), WHITE, 1.0),
    ("rect", (352, 298, 372, 40, 20), WHITE, 1.0),
    ("circle", (300, 408, 24), WHITE, 0.8),
    ("rect", (352, 388, 292, 40, 20), WHITE, 0.8),
    ("circle", (300, 498, 24), WHITE, 0.6),
    ("rect", (352, 478, 212, 40, 20), WHITE, 0.6),
    ("rect", (276, 590, 472, 168, 84), WHITE, 1.0),
    ("circle", (664, 674, 62), BLUE, 1.0),
]

ROOT = Path(__file__).resolve().parent.parent / "assets"


def hex_color(rgb):
    return "#{:02X}{:02X}{:02X}".format(*rgb)


def squircle_path():
    points = []
    for i in range(360):
        t = 2 * math.pi * i / 360
        ct, st = math.cos(t), math.sin(t)
        x = CENTER + BODY_HALF * math.copysign(abs(ct) ** (2 / SQUIRCLE_N), ct)
        y = CENTER + BODY_HALF * math.copysign(abs(st) ** (2 / SQUIRCLE_N), st)
        points.append(f"{x:.1f},{y:.1f}")
    return "M" + " L".join(points) + " Z"


def write_svg():
    path = squircle_path()
    shapes = []
    for kind, g, color, opacity in GLYPH:
        attrs = f'fill="{hex_color(color)}"'
        if opacity < 1:
            attrs += f' fill-opacity="{opacity}"'
        if kind == "circle":
            shapes.append(f'  <circle cx="{g[0]}" cy="{g[1]}" r="{g[2]}" {attrs}/>')
        else:
            shapes.append(
                f'  <rect x="{g[0]}" y="{g[1]}" width="{g[2]}" height="{g[3]}" rx="{g[4]}" {attrs}/>'
            )
    svg = f"""<svg xmlns="http://www.w3.org/2000/svg" width="{SIZE}" height="{SIZE}" viewBox="0 0 {SIZE} {SIZE}">
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="1" y2="1">
      <stop offset="0" stop-color="{hex_color(TEAL)}"/>
      <stop offset="1" stop-color="{hex_color(BLUE)}"/>
    </linearGradient>
    <linearGradient id="shine" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.18"/>
      <stop offset="0.5" stop-color="#FFFFFF" stop-opacity="0"/>
    </linearGradient>
  </defs>
  <path d="{path}" fill="url(#bg)"/>
  <path d="{path}" fill="url(#shine)"/>
{chr(10).join(shapes)}
</svg>
"""
    (ROOT / "icon.svg").write_text(svg)


def coverage(distance):
    return min(1.0, max(0.0, 0.5 - distance))


def squircle_distance(x, y):
    u = abs(x - CENTER) / BODY_HALF
    v = abs(y - CENTER) / BODY_HALF
    n = SQUIRCLE_N
    r = (u**n + v**n) ** (1 / n)
    if r == 0:
        return -BODY_HALF
    grad = math.sqrt(u ** (2 * n - 2) + v ** (2 * n - 2)) / r ** (n - 1)
    return (r - 1) * BODY_HALF / grad


def shape_distance(kind, g, x, y):
    if kind == "circle":
        cx, cy, radius = g
        return math.hypot(x - cx, y - cy) - radius
    left, top, w, h, radius = g
    qx = abs(x - (left + w / 2)) - (w / 2 - radius)
    qy = abs(y - (top + h / 2)) - (h / 2 - radius)
    outside = math.hypot(max(qx, 0), max(qy, 0))
    return outside + min(max(qx, qy), 0) - radius


def bounds(kind, g):
    if kind == "circle":
        cx, cy, radius = g
        return cx - radius - 1, cy - radius - 1, cx + radius + 1, cy + radius + 1
    left, top, w, h, _ = g
    return left - 1, top - 1, left + w + 1, top + h + 1


def over(dst, color, alpha):
    """Composite a straight-alpha color onto a premultiplied RGBA pixel."""
    keep = 1 - alpha
    return [color[0] * alpha + dst[0] * keep, color[1] * alpha + dst[1] * keep,
            color[2] * alpha + dst[2] * keep, alpha + dst[3] * keep]


def render():
    lo, hi = CENTER - BODY_HALF, CENTER + BODY_HALF
    glyph = [(kind, g, color, opacity, bounds(kind, g)) for kind, g, color, opacity in GLYPH]
    rows = bytearray()
    for py in range(SIZE):
        rows.append(0)
        y = py + 0.5
        for px in range(SIZE):
            x = px + 0.5
            pixel = [0.0, 0.0, 0.0, 0.0]
            if lo - 1 <= x <= hi + 1 and lo - 1 <= y <= hi + 1:
                body = coverage(squircle_distance(x, y))
                if body > 0:
                    t = ((x - lo) + (y - lo)) / (2 * (hi - lo))
                    fill = [TEAL[i] + (BLUE[i] - TEAL[i]) * t for i in range(3)]
                    shine = 0.18 * max(0.0, 1 - ((y - lo) / (hi - lo)) / 0.5)
                    fill = [c + (255 - c) * shine for c in fill]
                    pixel = over(pixel, fill, body)
                    for kind, g, color, opacity, (x0, y0, x1, y1) in glyph:
                        if x0 <= x <= x1 and y0 <= y <= y1:
                            a = coverage(shape_distance(kind, g, x, y)) * opacity
                            if a > 0:
                                pixel = over(pixel, color, a)
            alpha = pixel[3]
            if alpha <= 0:
                rows.extend(b"\0\0\0\0")
            else:
                rows.extend(round(min(255, c / alpha)) for c in pixel[:3])
                rows.append(round(alpha * 255))
    return bytes(rows)


def write_png(raw):
    def chunk(tag, data):
        body = tag + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0)
    png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header)
    png += chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")
    (ROOT / "icon.png").write_bytes(png)


if __name__ == "__main__":
    ROOT.mkdir(exist_ok=True)
    write_svg()
    write_png(render())
