"""Generates the Rhumb icon set from a single vector description.

Run:  python tools/make_icon.py

Writes:
  assets/icon.png   256x256 RGBA, for the window, AppImage and taskbar
  assets/icon.ico   multi-size Windows icon for the installer and Explorer
  assets/icon.svg   source of truth, for anyone who wants to edit it
"""

import struct
import zlib
from pathlib import Path

# ---- palette (matches src/theme.rs) ---------------------------------------
BG = (0x0D, 0x0E, 0x10)
ACCENT = (0xF6, 0xF7, 0xF8)
DIM = (0x6A, 0x6E, 0x76)

S = 256  # supersample factor target size


def rounded_rect_alpha(x, y, w, h, radius, px, py, samples=3):
    """Coverage of a rounded rectangle at point (px, py), supersampled."""
    hits = 0
    for sy in range(samples):
        for sx in range(samples):
            fx = px + (sx + 0.5) / samples
            fy = py + (sy + 0.5) / samples
            if not (x <= fx <= x + w and y <= fy <= y + h):
                continue
            # Distance to the rounded corner, if in a corner region.
            cx = min(max(fx, x + radius), x + w - radius)
            cy = min(max(fy, y + radius), y + h - radius)
            dx = fx - cx
            dy = fy - cy
            if dx * dx + dy * dy <= radius * radius:
                hits += 1
    return hits / (samples * samples)


def blend(dst, src, a):
    return tuple(int(round(d + (s - d) * a)) for d, s in zip(dst, src))


def render(size=S, ss=3):
    """Renders the icon: a dark rounded square, a folder outline, an X."""
    n = size * ss
    k = n / 256.0  # the drawing is authored in a 256x256 space
    buf = [[(0, 0, 0, 0.0)] * n for _ in range(n)]
    m = 6 * k  # margin
    radius = 52 * k

    # Panel plate.
    for py in range(n):
        for px in range(n):
            a = rounded_rect_alpha(m, m, n - 2 * m, n - 2 * m, radius, px, py)
            if a > 0:
                buf[py][px] = (*BG, a)

    def stroke(segments, width, color):
        half = width * k / 2
        for py in range(n):
            for px in range(n):
                best = 0.0
                for (x1, y1), (x2, y2) in segments:
                    # Distance from the pixel centre to the segment.
                    vx, vy = x2 - x1, y2 - y1
                    wx, wy = px + 0.5 - x1, py + 0.5 - y1
                    seg = vx * vx + vy * vy
                    t = 0.0 if seg == 0 else max(0.0, min(1.0, (wx * vx + wy * vy) / seg))
                    dx = wx - t * vx
                    dy = wy - t * vy
                    d = (dx * dx + dy * dy) ** 0.5
                    cov = max(0.0, min(1.0, half - d + 0.5))
                    best = max(best, cov)
                if best > 0:
                    r, g, b, a = buf[py][px]
                    buf[py][px] = (*blend((r, g, b), color, best), max(a, best))

    def u(v):
        return v * k

    def poly(points):
        """Converts a polyline into consecutive point pairs."""
        return list(zip(points, points[1:]))

    # Folder outline: tab, then body.
    fx0, fy0, fx1, fy1 = u(58), u(84), u(198), u(180)
    tab = [(fx0, fy0), (fx0 + 46, fy0), (fx0 + 60, fy0 + 22), (fx1, fy0 + 22)]
    body = [(fx0, fy0), (fx0, fy1), (fx1, fy1), (fx1, fy0 + 22)]
    stroke(poly(tab) + poly(body), 11, ACCENT)

    # The X: two diagonals inside the folder.
    cx0, cy0, cx1, cy1 = u(94), u(112), u(162), u(158)
    stroke([((cx0, cy0), (cx1, cy1)), ((cx1, cy0), (cx0, cy1))], 13, ACCENT)

    # Downsample the supersampled buffer.
    out = bytearray()
    for y in range(size):
        out.append(0)  # PNG filter: none
        for x in range(size):
            r = g = b = a = 0.0
            for j in range(ss):
                for i in range(ss):
                    pr, pg, pb, pa = buf[y * ss + j][x * ss + i]
                    r += pr * pa
                    g += pg * pa
                    b += pb * pa
                    a += pa
            count = ss * ss
            if a > 0:
                r, g, b = r / a, g / a, b / a
            a /= count
            out += bytes((int(round(r)), int(round(g)), int(round(b)), int(round(a * 255))))
    return bytes(out)


def write_png(path, size, raw):
    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    Path(path).write_bytes(png)
    return len(png)


def write_ico(path, sizes, pngs):
    """ICO with PNG-compressed entries (Windows Vista and later)."""
    count = len(sizes)
    header = struct.pack("<HHH", 0, 1, count)
    offset = 6 + 16 * count
    entries = b""
    data = b""
    for size, png in zip(sizes, pngs):
        entries += struct.pack(
            "<BBBBHHII",
            0 if size >= 256 else size,
            0 if size >= 256 else size,
            0,
            0,
            1,
            32,
            len(png),
            offset,
        )
        data += png
        offset += len(png)
    Path(path).write_bytes(header + entries + data)
    return len(header + entries + data)


SVG = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">
  <rect x="6" y="6" width="244" height="244" rx="52" fill="#0D0E10"/>
  <g fill="none" stroke="#F6F7F8" stroke-width="11" stroke-linejoin="round">
    <path d="M58 84 h46 l14 22 h80 v74 H58 Z"/>
  </g>
  <g fill="none" stroke="#F6F7F8" stroke-width="13" stroke-linecap="round">
    <path d="M94 112 L162 158 M162 112 L94 158"/>
  </g>
</svg>
"""


def main():
    root = Path(__file__).resolve().parent.parent
    assets = root / "assets"
    assets.mkdir(parents=True, exist_ok=True)

    (assets / "icon.svg").write_text(SVG, encoding="utf-8")

    full = render(256, ss=3)
    n = write_png(assets / "icon.png", 256, full)
    print(f"assets/icon.png  {n:>7,} bytes (256x256)")

    sizes = [16, 24, 32, 48, 64, 128, 256]
    pngs = []
    for s in sizes:
        ss = 3 if s <= 64 else 2
        raw = render(s, ss=ss)
        tmp = assets / f".icon_{s}.png"
        write_png(tmp, s, raw)
        pngs.append(tmp.read_bytes())
    n = write_ico(assets / "icon.ico", sizes, pngs)
    for s in sizes:
        (assets / f".icon_{s}.png").unlink()
    print(f"assets/icon.ico  {n:>7,} bytes ({', '.join(str(s) for s in sizes)})")


if __name__ == "__main__":
    main()
