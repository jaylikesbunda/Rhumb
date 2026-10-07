"""Generates the Rhumb icon set from a single vector description.

Run:  python tools/make_icon.py        (needs Pillow: pip install pillow)

Writes:
  assets/icon.png   256x256 RGBA, for the AppImage and anywhere a single PNG is wanted
  assets/icon.ico   multi-size Windows icon for the window, installer and Explorer
  assets/icon.svg   the same drawing as vector, for anyone who wants to edit it
"""

import io
import math
from pathlib import Path

from PIL import Image, ImageDraw

# ---- palette (matches src/theme.rs) ---------------------------------------
BG = (0x0D, 0x0E, 0x10, 255)
FG = (0xF6, 0xF7, 0xF8, 255)

# ---- drawing, authored in a 256x256 space ---------------------------------
PLATE_RADIUS = 56
STROKE = 9.0
# A closed folder outline: tab at the top left, sloping into the body.
# Each vertex carries the radius its corner is rounded with.
FOLDER = [
    ((42, 68), 11),
    ((106, 68), 5),
    ((121, 88), 5),
    ((214, 88), 11),
    ((214, 194), 11),
    ((42, 194), 11),
]
CROSS = [((102, 122), (154, 164)), ((154, 122), (102, 164))]


def weight_for(size):
    """Strokes thicken at small sizes, or they dissolve into grey at 16 and 24."""
    if size <= 16:
        return 1.6
    if size <= 24:
        return 1.3
    if size <= 32:
        return 1.1
    return 1.0


def rounded_path(corners, steps=10):
    """Turns corner vertices with radii into a polyline with arcs at each one."""
    pts = []
    n = len(corners)
    for i, (p, r) in enumerate(corners):
        prev = corners[i - 1][0]
        nxt = corners[(i + 1) % n][0]
        v1 = (prev[0] - p[0], prev[1] - p[1])
        v2 = (nxt[0] - p[0], nxt[1] - p[1])
        l1 = math.hypot(*v1)
        l2 = math.hypot(*v2)
        u1 = (v1[0] / l1, v1[1] / l1)
        u2 = (v2[0] / l2, v2[1] / l2)
        angle = math.acos(max(-1.0, min(1.0, u1[0] * u2[0] + u1[1] * u2[1])))
        # Distance back along each edge to where the arc starts.
        t = min(r / math.tan(angle / 2), l1 / 2, l2 / 2)
        rr = t * math.tan(angle / 2)
        a = (p[0] + u1[0] * t, p[1] + u1[1] * t)
        b = (p[0] + u2[0] * t, p[1] + u2[1] * t)
        bis = (u1[0] + u2[0], u1[1] + u2[1])
        bl = math.hypot(*bis)
        d = rr / math.sin(angle / 2)
        c = (p[0] + bis[0] / bl * d, p[1] + bis[1] / bl * d)
        a0 = math.atan2(a[1] - c[1], a[0] - c[0])
        a1 = math.atan2(b[1] - c[1], b[0] - c[0])
        da = (a1 - a0 + math.pi) % (2 * math.pi) - math.pi
        for s in range(steps + 1):
            ang = a0 + da * s / steps
            pts.append((c[0] + rr * math.cos(ang), c[1] + rr * math.sin(ang)))
    return pts


def stroke_line(draw, pts, width, closed=False):
    """A polyline with round joins and caps."""
    seq = pts + [pts[0]] if closed else pts
    draw.line(seq, fill=FG, width=round(width), joint="curve")
    r = width / 2
    for x, y in (pts if closed else [pts[0], pts[-1]]):
        draw.ellipse((x - r, y - r, x + r, y + r), fill=FG)


def render(size, weight=1.0):
    ss = 16 if size <= 64 else 8
    n = size * ss
    k = n / 256.0
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle((0, 0, n - 1, n - 1), radius=PLATE_RADIUS * k, fill=BG)

    w = STROKE * weight * k
    outline = [(x * k, y * k) for x, y in rounded_path(FOLDER)]
    stroke_line(d, outline, w, closed=True)
    for (x1, y1), (x2, y2) in CROSS:
        stroke_line(d, [(x1 * k, y1 * k), (x2 * k, y2 * k)], w)
    return img.resize((size, size), Image.LANCZOS)


def svg():
    def path(corners):
        pts = rounded_path(corners, steps=6)
        return "M" + " L".join(f"{x:.1f} {y:.1f}" for x, y in pts) + " Z"

    cross = " ".join(f"M{a[0]} {a[1]} L{b[0]} {b[1]}" for a, b in CROSS)
    return f"""<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256">
  <rect width="256" height="256" rx="{PLATE_RADIUS}" fill="#0D0E10"/>
  <g fill="none" stroke="#F6F7F8" stroke-width="{STROKE:g}" stroke-linejoin="round" stroke-linecap="round">
    <path d="{path(FOLDER)}"/>
    <path d="{cross}"/>
  </g>
</svg>
"""


def main():
    root = Path(__file__).resolve().parent.parent
    assets = root / "assets"
    assets.mkdir(parents=True, exist_ok=True)

    (assets / "icon.svg").write_text(svg(), encoding="utf-8")
    render(256).save(assets / "icon.png")
    print("assets/icon.png  (256x256)")

    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    frames = [render(s, weight_for(s)) for s in sizes]
    # Pillow writes a PNG-compressed ICO entry per size when given the frames.
    frames[-1].save(
        assets / "icon.ico",
        format="ICO",
        sizes=[(s, s) for s in sizes],
        append_images=frames[:-1],
    )
    print(f"assets/icon.ico  ({', '.join(str(s) for s in sizes)})")


if __name__ == "__main__":
    main()
