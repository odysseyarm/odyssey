#!/usr/bin/env python3
"""Generates the SDK docs diagrams in this directory.

    python3 generate.py --simple-icons <simple-icons>/icons --emblem <emblem.png>

--simple-icons: the icons/ folder of the simple-icons npm package (CC0 logo
  shapes for the engine chart), e.g. from `npm pack simple-icons`.
--emblem: the Odyssey Arm emblem, e.g. src/assets/images/emblem.png in
  odysseyarm/odysseyarm.github.io.
Needs Pillow. Writes the SVGs next to this script unless --out is given.

Every diagram takes its colors from CSS variables, so the website can theme it
inline (--oa-dg-*), and falls back to light or dark colors through
prefers-color-scheme when shown as a plain image, as on GitHub. Diagrams use no
external fonts, ids or markers, so several can be inlined on one page.
"""
import argparse
import base64
import io
import math
import os
import re

OUT = os.path.dirname(os.path.abspath(__file__))
SI = None
EMBLEM = None

LIGHT = dict(fg="#1f2328", muted="#59636e", line="#d1d9e0", faint="#eef1f4", surface="#f6f8fa",
             panel="#ffffff", x="#cf222e", y="#1a7f37", z="#0969da", accent="#bc4c00",
             accentsoft="#fff4ea", dev="#57606a", devtop="#8c959f", devside="#424a53",
             trk="#d97a1e", trktop="#f0a25a", trkside="#a85a12",
             hand="#e4e8ec", handline="#424a53")
DARK = dict(fg="#e6edf3", muted="#9198a1", line="#3d444d", faint="#1c2128", surface="#161b22",
            panel="#0d1117", x="#ff7b72", y="#3fb950", z="#58a6ff", accent="#ffa657",
            accentsoft="#2b1d10", dev="#8b949e", devtop="#b1bac4", devside="#6e7681",
            trk="#e8893a", trktop="#ffb877", trkside="#b8661c",
            hand="#59616b", handline="#d4dae0")


def style():
    def block(c):
        return ";".join(f"--{k}:var(--oa-dg-{k},{v})" for k, v in c.items())
    return (
        "<style>"
        f".odg{{{block(LIGHT)};font-family:var(--oa-dg-font,system-ui,-apple-system,'Segoe UI',Helvetica,Arial,sans-serif)}}"
        f"@media (prefers-color-scheme:dark){{.odg{{{block(DARK)}}}}}"
        ".odg text{fill:var(--fg);font-size:16.5px;dominant-baseline:auto}"
        ".odg .m{fill:var(--muted)}"
        ".odg .s{font-size:14.5px}"
        ".odg .l{font-size:19px}"
        ".odg .b{font-weight:600}"
        ".odg .mono{font-family:var(--oa-dg-mono,ui-monospace,SFMono-Regular,Menlo,Consolas,monospace);font-size:14.5px}"
        ".odg .h{paint-order:stroke fill;stroke:var(--panel);stroke-width:5px;stroke-linejoin:round}"
        "</style>"
    )


class Svg:
    def __init__(self, w, h, title, desc=""):
        self.w, self.h = w, h
        self.parts = []
        self.title, self.desc = title, desc

    def add(self, s):
        self.parts.append(s)

    def save(self, name):
        head = (
            f'<svg xmlns="http://www.w3.org/2000/svg" class="odg" viewBox="0 0 {self.w} {self.h}" '
            f'width="{self.w}" height="{self.h}" role="img">'
            f"<title>{esc(self.title)}</title>"
            + (f"<desc>{esc(self.desc)}</desc>" if self.desc else "")
            + style()
        )
        body = "".join(self.parts)
        svg = head + body + "</svg>\n"
        # One element per line keeps diffs readable without blank lines (which
        # would end an HTML block when the site inlines the file).
        svg = re.sub(r">\s*<", ">\n<", svg)
        os.makedirs(OUT, exist_ok=True)
        with open(os.path.join(OUT, name), "w") as f:
            f.write(svg)


def esc(s):
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def f(v):
    return f"{v:.1f}".rstrip("0").rstrip(".")


# ---------------------------------------------------------------- primitives

def line(s, p1, p2, color="var(--fg)", width=2, dash=None, cap="round", opacity=None):
    d = f' stroke-dasharray="{dash}"' if dash else ""
    o = f' opacity="{opacity}"' if opacity else ""
    s.add(f'<path d="M{f(p1[0])} {f(p1[1])}L{f(p2[0])} {f(p2[1])}" fill="none" '
          f'style="stroke:{color}" stroke-width="{width}" stroke-linecap="{cap}"{d}{o}/>')


def poly(s, pts, fill="none", stroke=None, width=1.5, dash=None, opacity=None, join="round"):
    d = "M" + "L".join(f"{f(x)} {f(y)}" for x, y in pts) + "Z"
    st = f"fill:{fill}" + (f";stroke:{stroke}" if stroke else "")
    da = f' stroke-dasharray="{dash}"' if dash else ""
    o = f' opacity="{opacity}"' if opacity else ""
    sw = f' stroke-width="{width}" stroke-linejoin="{join}"' if stroke else ""
    s.add(f'<path d="{d}" style="{st}"{sw}{da}{o}/>')


def arrow(s, p1, p2, color="var(--fg)", width=2.5, head=11, dash=None):
    """Line from p1 to p2 ending in a filled head at p2."""
    dx, dy = p2[0] - p1[0], p2[1] - p1[1]
    n = math.hypot(dx, dy)
    ux, uy = dx / n, dy / n
    base = (p2[0] - ux * head, p2[1] - uy * head)
    line(s, p1, (base[0] + ux * 1, base[1] + uy * 1), color, width, dash)
    hw = head * 0.48
    left = (base[0] - uy * hw, base[1] + ux * hw)
    right = (base[0] + uy * hw, base[1] - ux * hw)
    poly(s, [p2, left, right], fill=color)


def dbl_arrow(s, p1, p2, color="var(--muted)", width=1.5, head=8):
    mid = ((p1[0] + p2[0]) / 2, (p1[1] + p2[1]) / 2)
    arrow(s, mid, p1, color, width, head)
    arrow(s, mid, p2, color, width, head)


def text(s, x, y, t, cls="", anchor="start", color=None, rotate=None, halo=False):
    c = cls + (" h" if halo else "")
    st = f' style="fill:{color}"' if color else ""
    rot = f' transform="rotate({rotate} {f(x)} {f(y)})"' if rotate is not None else ""
    s.add(f'<text x="{f(x)}" y="{f(y)}" text-anchor="{anchor}" class="{c.strip()}"{st}{rot}>{t}</text>')


def circle(s, c, r, fill="none", stroke=None, width=2):
    st = f"fill:{fill}" + (f";stroke:{stroke}" if stroke else "")
    sw = f' stroke-width="{width}"' if stroke else ""
    s.add(f'<circle cx="{f(c[0])}" cy="{f(c[1])}" r="{f(r)}" style="{st}"{sw}/>')


def rect(s, x, y, w, h, rx=0, fill="none", stroke=None, width=1.5, dash=None):
    st = f"fill:{fill}" + (f";stroke:{stroke}" if stroke else "")
    sw = f' stroke-width="{width}"' if stroke else ""
    da = f' stroke-dasharray="{dash}"' if dash else ""
    s.add(f'<rect x="{f(x)}" y="{f(y)}" width="{f(w)}" height="{f(h)}" rx="{rx}" style="{st}"{sw}{da}/>')


def out_of_page(s, c, color, r=9):
    """Axis pointing at the viewer: a dot in a circle."""
    circle(s, c, r, fill="var(--panel)", stroke=color, width=2.5)
    circle(s, c, 3, fill=color)


def into_page(s, c, color, r=9):
    """Axis pointing away from the viewer: a cross in a circle."""
    circle(s, c, r, fill="var(--panel)", stroke=color, width=2.5)
    k = r * 0.55
    line(s, (c[0] - k, c[1] - k), (c[0] + k, c[1] + k), color, 2.5)
    line(s, (c[0] + k, c[1] - k), (c[0] - k, c[1] + k), color, 2.5)


def crosshair(s, c, r=10, color="var(--accent)"):
    circle(s, c, r, stroke=color, width=2.5)
    line(s, (c[0], c[1] - r - 5), (c[0], c[1] - r + 4), color, 2.5)
    line(s, (c[0], c[1] + r - 4), (c[0], c[1] + r + 5), color, 2.5)
    line(s, (c[0] - r - 5, c[1]), (c[0] - r + 4, c[1]), color, 2.5)
    line(s, (c[0] + r - 4, c[1]), (c[0] + r + 5, c[1]), color, 2.5)
    circle(s, c, 2.5, fill=color)


def logo(s, slug, x, y, size=22, color="var(--fg)"):
    with open(os.path.join(SI, slug + ".svg")) as fh:
        d = re.search(r' d="([^"]+)"', fh.read()).group(1)
    k = size / 24
    s.add(f'<path transform="translate({f(x)} {f(y)}) scale({k:.4f})" d="{d}" style="fill:{color}"/>')


def emblem(s, x, y, size=24):
    from PIL import Image
    im = Image.open(EMBLEM).convert("RGBA")
    im.thumbnail((64, 64))
    buf = io.BytesIO()
    im.save(buf, "PNG", optimize=True)
    b64 = base64.b64encode(buf.getvalue()).decode()
    s.add(f'<image x="{f(x)}" y="{f(y)}" width="{size}" height="{size}" href="data:image/png;base64,{b64}"/>')


# --------------------------------------------------- oblique 3D projection
# Odyssey's frame: +X right, +Y up, +Z toward the viewer (the shooter).
# Cabinet projection: the screen plane (z = 0) is drawn face-on; +Z runs
# down and to the left at half scale.
ANGLE = math.radians(32)
DEPTH = 0.5


class Proj:
    def __init__(self, ox, oy, scale):
        self.ox, self.oy, self.k = ox, oy, scale

    def __call__(self, x, y, z):
        return (self.ox + self.k * (x - DEPTH * z * math.cos(ANGLE)),
                self.oy + self.k * (-y + DEPTH * z * math.sin(ANGLE)))



# ------------------------------------------------------- tiny 3D renderer

def v_add(a, b): return (a[0] + b[0], a[1] + b[1], a[2] + b[2])
def v_sub(a, b): return (a[0] - b[0], a[1] - b[1], a[2] - b[2])
def v_mul(a, k): return (a[0] * k, a[1] * k, a[2] * k)
def v_dot(a, b): return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
def v_cross(a, b): return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])
def v_norm(a):
    n = math.sqrt(v_dot(a, a))
    return (a[0] / n, a[1] / n, a[2] / n)


def m_apply(m, v):
    """m is a tuple of three columns."""
    return v_add(v_add(v_mul(m[0], v[0]), v_mul(m[1], v[1])), v_mul(m[2], v[2]))


def m_mul(a, b):
    return tuple(m_apply(a, col) for col in b)


IDENTITY = ((1, 0, 0), (0, 1, 0), (0, 0, 1))


def rot_x(deg):
    c, s_ = math.cos(math.radians(deg)), math.sin(math.radians(deg))
    return ((1, 0, 0), (0, c, s_), (0, -s_, c))


def rot_y(deg):
    c, s_ = math.cos(math.radians(deg)), math.sin(math.radians(deg))
    return ((c, 0, -s_), (0, 1, 0), (s_, 0, c))


def look_rotation(forward, up=(0, 1, 0)):
    """Device rotation whose line of sight (-Z) is `forward`."""
    z = v_norm(v_mul(forward, -1))
    x = v_norm(v_cross(up, z))
    y = v_cross(z, x)
    return (x, y, z)


# Direction from the scene toward the viewer under the oblique projection.
VIEW = v_norm((DEPTH * math.cos(ANGLE), DEPTH * math.sin(ANGLE), 1))

SHADES = {
    "gun": ("var(--devtop)", "var(--dev)", "var(--devside)"),
    "trk": ("var(--trktop)", "var(--trk)", "var(--trkside)"),
}


def box_faces(center, size, local_rot, kind):
    """Faces of a box (in model space) as (corner list, outward normal, kind)."""
    hx, hy, hz = size[0] / 2, size[1] / 2, size[2] / 2
    corners = {}
    for i in (-1, 1):
        for j in (-1, 1):
            for k in (-1, 1):
                corners[(i, j, k)] = v_add(center, m_apply(local_rot, (i * hx, j * hy, k * hz)))
    faces = [
        ((1, 0, 0), [(1, -1, -1), (1, 1, -1), (1, 1, 1), (1, -1, 1)]),
        ((-1, 0, 0), [(-1, -1, -1), (-1, -1, 1), (-1, 1, 1), (-1, 1, -1)]),
        ((0, 1, 0), [(-1, 1, -1), (-1, 1, 1), (1, 1, 1), (1, 1, -1)]),
        ((0, -1, 0), [(-1, -1, -1), (1, -1, -1), (1, -1, 1), (-1, -1, 1)]),
        ((0, 0, 1), [(-1, -1, 1), (1, -1, 1), (1, 1, 1), (-1, 1, 1)]),
        ((0, 0, -1), [(-1, -1, -1), (-1, 1, -1), (1, 1, -1), (1, -1, -1)]),
    ]
    return [([corners[c] for c in quad], m_apply(local_rot, n), kind) for n, quad in faces]


def pistol_model():
    """A pistol with the Odyssey tracker under its barrel, in the device's frame:
    +X right, +Y up, +Z back toward the shooter, line of sight -Z. The origin is
    the tracker's camera, on the tracker's front face."""
    cam = (0, 0.004, -0.052)

    def at(c):
        return v_sub(c, cam)

    parts = []
    parts += box_faces(at((0, 0.054, 0.040)), (0.030, 0.032, 0.200), IDENTITY, "gun")   # slide
    parts += box_faces(at((0, 0.026, 0.050)), (0.027, 0.024, 0.165), IDENTITY, "gun")   # frame
    parts += box_faces(at((0, -0.030, 0.118)), (0.029, 0.110, 0.048), rot_x(-17), "gun")  # grip
    parts += box_faces(at((0, 0.004, -0.017)), (0.040, 0.032, 0.070), IDENTITY, "trk")  # tracker
    return parts


def render_model(s, P, faces, rot=IDENTITY, pos=(0, 0, 0), scale=1.0, stroke="var(--panel)"):
    world = []
    for corners, normal, kind in faces:
        pts = [v_add(pos, m_apply(rot, v_mul(c, scale))) for c in corners]
        n = m_apply(rot, normal)
        if v_dot(n, VIEW) <= 1e-6:
            continue
        depth = sum(v_dot(p, VIEW) for p in pts) / 4
        world.append((depth, pts, n, kind))
    world.sort(key=lambda w: w[0])
    for _, pts, n, kind in world:
        top, front, side = SHADES[kind]
        score = {"top": n[1] * 1.15, "front": v_dot(n, VIEW) * 0.9, "side": 0.55}
        shade = {"top": top, "front": front, "side": side}[max(score, key=score.get)]
        poly(s, [P(*p) for p in pts], fill=shade, stroke=stroke, width=1.1)


def pistol_2d(s, x, y, k=1.0, facing="left"):
    """Side view of the pistol with the tracker, muzzle toward `facing`.
    (x, y) is the tracker's camera."""
    sgn = -1 if facing == "left" else 1

    def P2(px, py):
        # px grows toward the back of the gun.
        return (x - sgn * px * k, y + py * k)

    def path(pts, fill, stroke=None, w=0):
        poly(s, [P2(*q) for q in pts], fill=fill, stroke=stroke, width=w)

    # Frame with grip.
    path([(-6, -34), (178, -34), (186, -14), (192, 92), (186, 100), (146, 100), (140, 92), (126, -12),
          (84, -14), (-6, -14)], "var(--dev)")
    # Trigger guard and trigger.
    s.add('<path d="M{} {}Q{} {} {} {}L{} {}" fill="none" style="stroke:var(--dev)" stroke-width="{}" '
          'stroke-linejoin="round" stroke-linecap="round"/>'.format(
              *(f(v) for v in P2(84, -14)), *(f(v) for v in P2(84, 24)), *(f(v) for v in P2(112, 24)),
              *(f(v) for v in P2(128, 20)), f(6 * k)))
    tr0, tr1 = P2(108, -12), P2(112, 10)
    line(s, tr0, tr1, "var(--devside)", 4 * k)
    # Slide on top, with rear serrations.
    path([(-14, -64), (196, -64), (200, -60), (200, -34), (-14, -34), (-18, -40), (-18, -60)], "var(--devside)")
    for i in range(5):
        a, b = P2(168 + i * 6, -60), P2(168 + i * 6, -38)
        line(s, a, b, "var(--dev)", 1.6 * k)
    # Tracker under the frame; its camera is the front face.
    path([(0, -12), (72, -12), (76, -8), (76, 18), (72, 22), (0, 22)], "var(--trk)")
    cx, cy = P2(0, 5)
    circle(s, (cx, cy), 4.5 * k, fill="var(--devside)")


# ================================================================ diagrams

def coords_3d():
    s = Svg(760, 470, "Odyssey's 3D coordinate system",
            "The screen lies in the XY plane with the origin at its top-left corner. "
            "+X points right, +Y up, and +Z out of the screen toward the shooter. "
            "A device in front of the screen looks along its -Z axis; where that line "
            "meets the screen is the aimpoint.")
    P = Proj(318, 96, 222)
    W, H = 1.6, 0.9
    poly(s, [P(0, 0, 0), P(W, 0, 0), P(W, -H, 0), P(0, -H, 0)], fill="var(--surface)", stroke="var(--fg)", width=2.5)
    br = P(W, -H, 0)
    text(s, br[0] - 12, br[1] - 12, "screen (z = 0)", "s m", anchor="end")

    dev = (0.56, -0.68, 1.70)
    aim = (1.22, -0.40, 0)
    R = look_rotation(v_sub(aim, dev))
    cam = P(*dev)

    # Translation: origin to the device's tracker camera.
    arrow(s, P(0, 0, 0), cam, "var(--muted)", 2, head=11, dash="7 6")
    mid = P(*v_mul(dev, 0.55))
    text(s, mid[0] - 12, mid[1] + 6, "pose.translation", "s mono m", anchor="end", halo=True)

    # Line of sight to the aimpoint.
    line(s, cam, P(*aim), "var(--accent)", 2.5, dash="9 7")
    crosshair(s, P(*aim))
    a = P(*aim)
    text(s, a[0] - 16, a[1] - 18, "aimpoint", "b", color="var(--accent)", anchor="end", halo=True)
    los = P(*v_add(dev, v_mul(v_sub(aim, dev), 0.2)))
    text(s, los[0] + 18, los[1] + 30, "line of sight (the device's −Z)", "s", color="var(--accent)", halo=True)

    render_model(s, P, pistol_model(), R, dev, scale=2.7)
    circle(s, cam, 4.5, fill="var(--panel)", stroke="var(--accent)", width=2.2)
    g = P(*v_add(dev, (-0.30, -0.30, 0.40)))
    text(s, g[0] - 8, g[1], "device", "b", anchor="end")

    # Axes at the origin, drawn last so they sit on top.
    o = P(0, 0, 0)
    xt, yt, zt = P(0.62, 0, 0), P(0, 0.36, 0), P(0, 0, 1.0)
    arrow(s, o, xt, "var(--x)", 4, head=15)
    arrow(s, o, yt, "var(--y)", 4, head=15)
    arrow(s, o, zt, "var(--z)", 4, head=15)
    circle(s, o, 5.5, fill="var(--fg)")
    text(s, xt[0] - 6, xt[1] - 14, "+X", "l b", color="var(--x)", anchor="end")
    text(s, xt[0] + 4, xt[1] - 14, "right", "s m")
    text(s, yt[0] + 14, yt[1] + 8, "+Y", "l b", color="var(--y)")
    text(s, yt[0] + 44, yt[1] + 8, "up", "s m")
    text(s, zt[0] - 10, zt[1] + 22, "+Z", "l b", color="var(--z)", anchor="end")
    text(s, zt[0] - 10, zt[1] + 40, "toward the shooter", "s m", anchor="end")
    text(s, o[0] - 16, o[1] - 40, "origin", "b", anchor="end")
    text(s, o[0] - 16, o[1] - 21, "top-left corner", "s m", anchor="end")
    text(s, o[0] - 16, o[1] - 4, "of the screen", "s m", anchor="end")
    s.save("coordinate-system.svg")


def gizmo(s, c, axes, L=60):
    """axes: list of (label, direction, color) with direction in
    {'right','up','toward','away'}."""
    toward = (-math.cos(ANGLE) * 0.8, math.sin(ANGLE) * 0.8)
    dirs = {"right": (1, 0), "up": (0, -1), "toward": toward, "away": (-toward[0], -toward[1])}
    for label, d, color in axes:
        dx, dy = dirs[d]
        tip = (c[0] + dx * L, c[1] + dy * L)
        arrow(s, c, tip, color, 3.5, head=13)
        lx, ly = c[0] + dx * (L + 16), c[1] + dy * (L + 16) + 6
        text(s, lx, ly, label, "l b", anchor="middle", color=color)
    circle(s, c, 4.5, fill="var(--fg)")


# ------------------------------------------------------------- hands

# Right hand, palm toward the viewer (+Z), fingers up (+Y), thumb to +X. Millimeters.
# Each finger: list of (point, radius) from the knuckle to the tip.
FINGERS = {
    "index": ([(25, 40, 0), (26, 78, 0), (26, 100, 0), (26, 118, 0)], [12, 11.2, 10.4, 9.6]),
    "middle": ([(6, 42, 0), (6, 46, 40), (6, 47, 64), (6, 47, 82)], [12.6, 11.8, 11, 10.2]),
    "ring": ([(-13, 40, 3), (-13, 45, 32), (-13, 22, 40), (-13, 8, 28)], [12, 11.2, 10.4, 9.6]),
    "little": ([(-30, 33, 3), (-31, 36, 25), (-31, 18, 30), (-30, 8, 22)], [10.8, 10, 9.4, 8.8]),
    "thumb": ([(28, -24, 6), (52, -9, 12), (75, -1, 13), (97, 1, 11)], [17, 13.5, 12.4, 11.4]),
}
PALM_W, PALM_TOP, PALM_BOTTOM, PALM_T = 42, 42, -44, 14


def tapered(a, ra, b, rb):
    """Outline of two circles joined by their outer tangents, as a path."""
    dx, dy = b[0] - a[0], b[1] - a[1]
    d = math.hypot(dx, dy)
    if d <= abs(ra - rb) + 0.01:  # one circle inside the other
        c, r = (a, ra) if ra >= rb else (b, rb)
        return (f"M{f(c[0] - r)} {f(c[1])}a{f(r)} {f(r)} 0 1 0 {f(2 * r)} 0"
                f"a{f(r)} {f(r)} 0 1 0 {f(-2 * r)} 0Z")
    th = math.atan2(dy, dx)
    ph = math.acos(max(-1, min(1, (ra - rb) / d)))
    p = lambda c, r, ang: (c[0] + r * math.cos(ang), c[1] + r * math.sin(ang))
    a1, a2 = p(a, ra, th + ph), p(a, ra, th - ph)
    b1, b2 = p(b, rb, th + ph), p(b, rb, th - ph)
    large_a = 1 if ph < math.pi / 2 else 0
    large_b = 1 - large_a
    return (f"M{f(a1[0])} {f(a1[1])}A{f(ra)} {f(ra)} 0 {large_a} 1 {f(a2[0])} {f(a2[1])}"
            f"L{f(b2[0])} {f(b2[1])}A{f(rb)} {f(rb)} 0 {large_b} 1 {f(b1[0])} {f(b1[1])}Z")


def convex_hull(pts):
    pts = sorted(set(pts))
    def cross(o, a, b):
        return (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0])
    lo, hi = [], []
    for p in pts:
        while len(lo) >= 2 and cross(lo[-2], lo[-1], p) <= 0:
            lo.pop()
        lo.append(p)
    for p in reversed(pts):
        while len(hi) >= 2 and cross(hi[-2], hi[-1], p) <= 0:
            hi.pop()
        hi.append(p)
    return lo[:-1] + hi[:-1]


YAW, PITCH, CAM = math.radians(-18), math.radians(24), 360.0


def view(v, mirror):
    """Model -> camera space: mirror for the left hand, then turn the hand so
    the camera sees it from above, and from the right for the right hand or
    the left for the left hand, so its middle finger is never hidden."""
    x, y, z = v[0], v[1], -v[2] if mirror else v[2]
    yaw = -YAW if mirror else YAW
    x, z = x * math.cos(yaw) + z * math.sin(yaw), -x * math.sin(yaw) + z * math.cos(yaw)
    y, z = y * math.cos(PITCH) - z * math.sin(PITCH), y * math.sin(PITCH) + z * math.cos(PITCH)
    return (x, y, z)


def hand3d(s, ox, oy, k, mirror=False, colors=None):
    """A right hand (palm toward the viewer, middle finger toward them) or,
    mirrored, a left hand seen from the back with the middle finger pointing
    away. Returns, per finger, the projected tip, its screen direction, and a
    projector for further points."""
    colors = colors or {}

    def proj(v):
        x, y, z = view(v, mirror)
        sc = CAM / (CAM - z)
        return (ox + k * x * sc, oy - k * y * sc), sc, z

    H, L = "var(--hand)", "var(--handline)"
    items = []
    pts = []
    for z in (-PALM_T, PALM_T):
        for cx, cy, a0 in ((PALM_W - 14, PALM_TOP - 14, 0), (-PALM_W + 14, PALM_TOP - 14, 90),
                           (-PALM_W + 18, PALM_BOTTOM + 18, 180), (PALM_W - 18, PALM_BOTTOM + 18, 270)):
            r = 14 if cy > 0 else 18
            for i in range(7):
                a = math.radians(a0 + i * 15)
                pts.append(proj((cx + r * math.cos(a), cy + r * math.sin(a), z))[0])
    hull = convex_hull([(round(x, 1), round(y, 1)) for x, y in pts])
    items.append((proj((0, 0, 0))[2] - 4, "palm", hull))
    items.append((proj((0, -60, 0))[2] - 30, "seg", ((0, -40, 0), 32, (0, -96, 0), 29, H)))
    for name, (joints, radii) in FINGERS.items():
        if name != "thumb":   # knuckle on the back of the hand
            j = joints[0]
            kn = (j[0], j[1] - 4, -9)
            items.append((proj(kn)[2], "seg", (kn, radii[0] * 0.95, kn, radii[0] * 0.95, H)))
        for i in range(3):
            a, b = joints[i], joints[i + 1]
            color = colors.get(name, H) if (name != "thumb" or i > 0) else H
            items.append(((proj(a)[2] + proj(b)[2]) / 2 + 0.1, "seg", (a, radii[i], b, radii[i + 1], color)))
        # Nail, on the back of the last segment.
        a, b = joints[-2], joints[-1]
        u = [b[i] - a[i] for i in range(3)]
        n = math.sqrt(sum(c * c for c in u)) or 1
        nail_c = (b[0] - u[0] / n * 6, b[1] - u[1] / n * 6, b[2] - u[2] / n * 6)
        # The back of a finger is its -Z side in the model, except for the
        # thumb (its nail faces +X/+Y-ish) and the bent middle finger (+Y).
        off = {"thumb": (0, 4, -9), "middle": (0, 8.5, 0)}.get(name, (0, 0, -8.5))
        nail = tuple(nail_c[i] + off[i] for i in range(3))
        items.append((proj(nail)[2] + 0.2, "nail", (nail, radii[-1] * 0.62)))
    items.sort(key=lambda t: t[0])
    for _, kind, data in items:
        if kind == "palm":
            d = "M" + "L".join(f"{f(x)} {f(y)}" for x, y in data) + "Z"
            s.add(f'<path d="{d}" style="fill:{H};stroke:{L}" stroke-width="2.5" stroke-linejoin="round"/>')
        elif kind == "nail":
            c, r = data
            (px, py), sc, z = proj(c)
            if z < proj((c[0], c[1], 0))[2] - 100:
                continue
            s.add(f'<ellipse cx="{f(px)}" cy="{f(py)}" rx="{f(r * k * sc)}" ry="{f(r * k * sc * 1.15)}" '
                  f'style="fill:var(--panel);stroke:{L}" stroke-width="1.6" opacity="0.9"/>')
        else:
            a, ra, b, rb, color = data
            (pa, sa, _), (pb, sb, _) = proj(a), proj(b)
            d = tapered(pa, ra * k * sa, pb, rb * k * sb)
            s.add(f'<path d="{d}" style="fill:{color};stroke:{L}" stroke-width="2.5" stroke-linejoin="round"/>')
            ha, hb = ra * k * sa, rb * k * sb
            off = lambda q, r: (q[0] - 0.32 * r, q[1] - 0.32 * r)
            d = tapered(off(pa, ha), ha * 0.42, off(pb, hb), hb * 0.42)
            s.add(f'<path d="{d}" style="fill:#fff" opacity="0.28"/>')
    tips = {}
    for name, (joints, _) in FINGERS.items():
        a, b = joints[-2], joints[-1]
        tips[name] = (a, b)
    return tips, proj



def axis_hand(s, ox, oy, k, side):
    """The axis sign with an arrow from each fingertip: thumb +X, index +Y,
    middle finger +Z. side="right": right hand, palm toward the viewer, +Z at
    them. side="left": left hand from the back, +Z away from them."""
    mirror = side == "left"
    colors = {"thumb": "var(--x)", "index": "var(--y)", "middle": "var(--z)"}
    arrows = [("thumb", (1, 0, 0), "X", "var(--x)"), ("index", (0, 1, 0), "Y", "var(--y)"),
              ("middle", (0, 0, 1), "Z", "var(--z)")]

    def ends(name, u, start, length):
        joints, radii = FINGERS[name]
        tip, r = joints[-1], radii[-1]
        return [proj(tuple(tip[i] + u[i] * (r + d) for i in range(3)))[0] for d in (start, length, length + 18)]

    tmp = Svg(1, 1, "")
    _, proj = hand3d(tmp, ox, oy, k, mirror)   # the projector only depends on the view
    hand3d(s, ox, oy, k, mirror, colors)
    for name, u, label, color in arrows:
        if name == "middle" and mirror:
            # Points away, behind the hand: dashed while hidden, then solid.
            p0, p1, pl = ends(name, u, -10, 64)
            mid = (p0[0] + (p1[0] - p0[0]) * 0.55, p0[1] + (p1[1] - p0[1]) * 0.55)
            line(s, p0, mid, color, 2.5, dash="4 5")
            arrow(s, mid, p1, color, 3.5, head=13)
        else:
            p0, p1, pl = ends(name, u, 6, 30 if name == "index" else 44)
            arrow(s, p0, p1, color, 3.5, head=13)
        if name == "index":
            text(s, p1[0] + 12, p1[1] + 12, label, "l b", color=color, halo=True)
        else:
            text(s, pl[0], pl[1] + 7, label, "l b", anchor="middle", color=color, halo=True)
    return proj


def up_glyph(s, c, up, color):
    """Row label: a small axis tripod with its up axis colored."""
    for dx, dy in ((-0.85, 0.5), (0.85, 0.5)):
        line(s, c, (c[0] + dx * 26, c[1] + dy * 26), "var(--muted)", 3)
    arrow(s, c, (c[0], c[1] - 44), color, 4, head=14)
    circle(s, c, 4, fill="var(--fg)")
    text(s, c[0], c[1] + 46, f"{up} up", "l b", anchor="middle", color=color)


def engines():
    s = Svg(760, 700, "Axis conventions in common engines",
            "Two hands make the axis sign: thumb +X pointing right, index finger +Y up, "
            "middle finger +Z. On the right hand, palm toward you, the middle finger points at "
            "you; on the left hand, back toward you, it points away. Below them, a two-by-two "
            "chart. Rows: Y up, Z up. Columns: right-handed, left-handed. Odyssey is "
            "right-handed with Y up and forward along -Z, like Bevy, Godot, three.js, and "
            "OpenGL. Unity is left-handed with Y up and forward along +Z. Blender is "
            "right-handed with Z up and forward along -Y. Unreal Engine is left-handed with Z "
            "up and forward along +X.")
    X0, Y0, CW, CH, G = 136, 322, 297, 156, 14
    heads = [("Right-handed", "right", "Z points toward you"), ("Left-handed", "left", "Z points away from you")]
    for i, (title, side, ztext) in enumerate(heads):
        mx = X0 + i * (CW + G) + CW / 2
        text(s, mx, 30, title, "l b", anchor="middle")
        text(s, mx, 52, ztext, "s m", anchor="middle")
        axis_hand(s, mx - 10, 222, 0.86, side)
    up_glyph(s, (64, Y0 + 92), "Y", "var(--y)")
    up_glyph(s, (64, Y0 + CH + G + 92), "Z", "var(--z)")
    cells = [
        (0, 0, True, "forward: −Z",
         [("odyssey", "Odyssey"), ("bevy", "Bevy"), ("godotengine", "Godot"), ("threedotjs", "three.js"), (None, "OpenGL")]),
        (0, 1, False, "forward: −Y", [("blender", "Blender")]),
        (1, 0, False, "forward: +Z", [("unity", "Unity")]),
        (1, 1, False, "forward: +X", [("unrealengine", "Unreal Engine")]),
    ]
    for col, row, ours, fwd, tools in cells:
        x = X0 + col * (CW + G)
        y = Y0 + row * (CH + G)
        if ours:
            rect(s, x, y, CW, CH, rx=14, fill="var(--accentsoft)", stroke="var(--accent)", width=2)
        else:
            rect(s, x, y, CW, CH, rx=14, fill="var(--surface)", stroke="var(--line)", width=1.5)
        text(s, x + CW - 18, y + CH - 16, fwd, "s m", anchor="end")
        if len(tools) == 1:
            slots = [(x + CW / 2 - 70, y + CH / 2 + 2)]
        else:
            slots = [(x + 22, y + 40), (x + 22, y + 74), (x + 22, y + 108),
                     (x + 160, y + 40), (x + 160, y + 74)]
        for (slug, name), (lx, ty) in zip(tools, slots):
            if slug == "odyssey":
                emblem(s, lx - 3, ty - 20, 27)
                text(s, lx + 30, ty, name, "b", color="var(--accent)")
            elif slug:
                logo(s, slug, lx, ty - 17, 22)
                text(s, lx + 30, ty, name)
            else:
                text(s, lx + 30, ty, name)
    text(s, X0 + CW + G / 2, Y0 + 2 * CH + G + 40,
         "Same thumb and index finger, mirrored hand: only the middle finger, Z, changes direction.",
         "s m", anchor="middle")
    s.save("engine-axes.svg")


def aimpoint():
    s = Svg(700, 400, "Aimpoint coordinates",
            "The aimpoint is a fraction of the screen. (0, 0) is the top-left corner, "
            "(1, 1) the bottom-right corner, and (0.5, 0.5) the center. x grows to the right "
            "and y grows downward. Values outside 0 to 1 mean the device is aimed past an edge.")
    x0, y0, W, H = 118, 70, 448, 252
    rect(s, x0, y0, W, H, rx=6, fill="var(--surface)", stroke="var(--fg)", width=2.5)
    for i in (1, 2, 3):
        line(s, (x0 + W * i / 4, y0), (x0 + W * i / 4, y0 + H), "var(--line)", 1, dash="3 6")
        line(s, (x0, y0 + H * i / 4), (x0 + W, y0 + H * i / 4), "var(--line)", 1, dash="3 6")
    arrow(s, (x0, y0 - 26), (x0 + 150, y0 - 26), "var(--x)", 3, head=12)
    text(s, x0 + 160, y0 - 21, "x", "l b", color="var(--x)")
    arrow(s, (x0 - 26, y0), (x0 - 26, y0 + 130), "var(--y)", 3, head=12)
    text(s, x0 - 26, y0 + 154, "y", "l b", anchor="middle", color="var(--y)")
    for (u, v, lab, dx, dy, anc) in [(0, 0, "(0, 0)", 12, 24, "start"), (1, 0, "(1, 0)", -12, 24, "end"),
                                     (0, 1, "(0, 1)", 12, -14, "start"), (1, 1, "(1, 1)", -12, -14, "end")]:
        p = (x0 + W * u, y0 + H * v)
        circle(s, p, 5, fill="var(--fg)")
        text(s, p[0] + dx, p[1] + dy, lab, "mono", anchor=anc)
    c = (x0 + W / 2, y0 + H / 2)
    crosshair(s, c)
    text(s, c[0] + 22, c[1] - 14, "(0.5, 0.5)", "mono b", color="var(--accent)", halo=True)
    text(s, c[0] + 22, c[1] + 4, "center", "s m", halo=True)
    op = (x0 + W * 1.14, y0 + H * 0.3)
    circle(s, op, 8, stroke="var(--muted)", width=2)
    line(s, (op[0] - 4, op[1] - 4), (op[0] + 4, op[1] + 4), "var(--muted)", 2)
    line(s, (op[0] + 4, op[1] - 4), (op[0] - 4, op[1] + 4), "var(--muted)", 2)
    text(s, op[0], op[1] + 28, "(1.14, 0.3)", "mono m", anchor="middle")
    text(s, op[0], op[1] + 46, "aimed past", "s m", anchor="middle")
    text(s, op[0], op[1] + 62, "the right edge", "s m", anchor="middle")
    text(s, x0 + W / 2, y0 + H + 40, "Origin at the top-left; y grows downward, like UI and image coordinates.",
         "s m", anchor="middle")
    s.save("aimpoint.svg")


def device_axes():
    s = Svg(760, 512, "The device's own axes",
            "A pistol with the Odyssey tracker under its barrel. Its own axes start at the "
            "tracker's camera: +X to the device's right, +Y up out of its top, and +Z back toward "
            "the shooter. Its line of sight, the way the tracker looks, is -Z. The columns of "
            "pose.rotation are these axes in the screen's frame.")
    P = Proj(430, 168, 1250)
    R = rot_y(-40)
    los_dir = m_apply(R, (0, 0, -1))
    le = P(*v_mul(los_dir, 0.22))
    line(s, P(0, 0, 0), le, "var(--accent)", 2.5, dash="9 7")
    text(s, le[0] + 10, le[1] + 5, "−Z", "l b", color="var(--accent)")
    render_model(s, P, pistol_model(), R, (0, 0, 0), scale=1.0)
    o = P(0, 0, 0)
    xt = P(*v_mul(m_apply(R, (1, 0, 0)), 0.15))
    yt = P(*v_mul(m_apply(R, (0, 1, 0)), 0.13))
    zt = P(*v_mul(m_apply(R, (0, 0, 1)), 0.135))
    for tip, color in ((xt, "var(--x)"), (yt, "var(--y)"), (zt, "var(--z)")):
        arrow(s, o, tip, "var(--panel)", 8, head=17)
        arrow(s, o, tip, color, 4, head=15)
    circle(s, o, 6, fill="var(--panel)", stroke="var(--fg)", width=2.5)
    text(s, xt[0] + 12, xt[1] + 10, "+X", "l b", color="var(--x)")
    text(s, yt[0] + 14, yt[1] + 8, "+Y", "l b", color="var(--y)")
    zm = ((o[0] + zt[0]) / 2, (o[1] + zt[1]) / 2)
    text(s, zm[0] - 8, zm[1] + 30, "+Z", "l b", color="var(--z)", anchor="middle", halo=True)
    text(s, o[0] + 16, o[1] - 14, "origin: the tracker's camera", "s b", halo=True)
    # Legend.
    rows = [("+X", "var(--x)", "the device's right", "column 1", "m11, m21, m31"),
            ("+Y", "var(--y)", "up, out of its top", "column 2", "m12, m22, m32"),
            ("+Z", "var(--z)", "back, toward the shooter", "column 3", "m13, m23, m33"),
            ("−Z", "var(--accent)", "line of sight: where the tracker looks", None, None)]
    ly = 376
    rect(s, 40, ly - 28, 680, 150, rx=12, fill="var(--surface)", stroke="var(--line)", width=1.2)
    text(s, 330, ly - 6, "pose.rotation", "s mono m", anchor="start")
    for i, (lab, color, desc, col, entries) in enumerate(rows):
        yy = ly + 20 + i * 27
        text(s, 64, yy, lab, "b", color=color)
        text(s, 104, yy, desc)
        if col:
            text(s, 430, yy, col, "s m")
            text(s, 500, yy, entries, "mono")
    s.save("device-axes.svg")


def zeroing():
    s = Svg(760, 340, "What zeroing corrects",
            "Side view. The Odyssey tracker sits below the bore and its line of sight is not "
            "parallel to it, so before zeroing the aimpoint misses the target. Zeroing records "
            "the offset from the tracker to the sight line and the angle that makes that line "
            "pass through the target.")
    sx = 74
    rect(s, sx - 6, 40, 12, 250, rx=3, fill="var(--fg)")
    text(s, sx, 316, "screen", "s m", anchor="middle")
    target = (sx + 6, 124)
    cam = (528, 206)
    bore = (cam[0], cam[1] - 50)
    miss = (sx + 6, 238)
    line(s, cam, miss, "var(--z)", 2.5, dash="8 7")
    line(s, bore, target, "var(--accent)", 3)
    pistol_2d(s, cam[0], cam[1], 1.0, "left")
    crosshair(s, target, 11)
    circle(s, miss, 7, fill="var(--z)")
    text(s, target[0] + 20, target[1] - 18, "target", "b", color="var(--accent)", halo=True)
    text(s, miss[0] + 18, miss[1] + 26, "aimpoint before zeroing", "s", color="var(--z)", halo=True)
    text(s, 300, 124, "sight line, used for the aimpoint after zeroing", "s", color="var(--accent)",
         anchor="middle", halo=True)
    text(s, 300, 246, "the tracker's own line of sight", "s", color="var(--z)", anchor="middle", halo=True)
    # Offset dimension, in front of the muzzle.
    dx = cam[0] - 44
    line(s, (dx - 8, bore[1]), (dx + 8, bore[1]), "var(--fg)", 1.5)
    line(s, (dx - 8, cam[1]), (dx + 8, cam[1]), "var(--fg)", 1.5)
    dbl_arrow(s, (dx, bore[1] + 2), (dx, cam[1] - 2), "var(--fg)", 1.6, 8)
    text(s, dx - 12, (bore[1] + cam[1]) / 2 - 2, "offset", "s b", anchor="end", halo=True)
    text(s, dx - 12, (bore[1] + cam[1]) / 2 + 14, "6.35 cm up", "s m", anchor="end", halo=True)
    text(s, 410, 318, "Zeroing finds the angle that puts the sight line through the target,",
         "s m", anchor="middle")
    text(s, 410, 335, "then applies it to every aimpoint from that device.", "s m", anchor="middle")
    s.save("zeroing.svg")


def shot_timing():
    s = Svg(760, 266, "Finding the aimpoint at the moment of the shot",
            "A timeline. Tracking events arrive about every 10 ms. The trigger breaks; the "
            "impact is detected a little later, after the shot delay. Subtract the shot delay "
            "from the impact's timestamp and use the tracking event closest to that time.")
    y = 124
    x0, x1 = 40, 720
    arrow(s, (x0, y), (x1, y), "var(--muted)", 1.8, head=10)
    text(s, x1, y + 28, "time", "s m", anchor="end")
    shot, impact = 300, 520
    for x in range(60, 700, 36):
        near = abs(x - shot) < 18
        circle(s, (x, y), 7 if near else 5, fill="var(--accent)" if near else "var(--muted)")
    text(s, 60, y - 22, "tracking events, about every 10 ms", "s m")
    line(s, (shot, y + 14), (shot, y + 56), "var(--fg)", 2)
    text(s, shot, y + 76, "trigger breaks", "s b", anchor="middle")
    line(s, (impact, y - 64), (impact, y + 56), "var(--z)", 2.5)
    text(s, impact, y - 74, "impact event", "s b", anchor="middle", color="var(--z)")
    text(s, impact, y + 76, "its timestamp", "s mono m", anchor="middle")
    by = y + 104
    dbl_arrow(s, (shot, by), (impact, by), "var(--fg)", 1.8, 9)
    text(s, (shot + impact) / 2, by + 22, "shot delay (set per device)", "s", anchor="middle")
    arrow(s, (impact - 10, y - 36), (shot + 14, y - 36), "var(--accent)", 2.5, head=11)
    text(s, impact - 16, y - 46, "use the event closest to timestamp − shot delay", "s b",
         anchor="end", color="var(--accent)")
    s.save("shot-timing.svg")


def data_flow():
    s = Svg(760, 230, "How your app gets tracking data",
            "Odyssey devices connect to Odyssey Desktop over USB, Bluetooth, or the dongle. "
            "Odyssey Desktop runs the hub, which does the tracking. Your app connects to the "
            "hub on the same computer through the client SDK and receives events.")
    boxes = [(16, "Odyssey device", ["camera and", "motion sensors"]),
             (282, "Odyssey Desktop", ["the hub: tracking,", "calibration, zero"]),
             (548, "Your app", ["client SDK:", "C#, Python, C/C++, Rust"])]
    for x, title, sub in boxes:
        ours = title == "Your app"
        rect(s, x, 30, 196, 140, rx=14, fill="var(--accentsoft)" if ours else "var(--surface)",
             stroke="var(--accent)" if ours else "var(--line)", width=2 if ours else 1.5)
        text(s, x + 98, 112, title, "b", anchor="middle")
        for i, sline in enumerate(sub):
            text(s, x + 98, 134 + i * 17, sline, "s m", anchor="middle")
    pistol_2d(s, 96, 66, 0.24, "left")
    mx, my = 380, 64
    rect(s, mx - 30, my - 24, 60, 38, rx=4, fill="var(--panel)", stroke="var(--fg)", width=2.5)
    line(s, (mx, my + 14), (mx, my + 22), "var(--fg)", 2.5)
    line(s, (mx - 14, my + 23), (mx + 14, my + 23), "var(--fg)", 2.5)
    crosshair(s, (mx, my - 5), 6)
    text(s, 646, 76, "&lt;/&gt;", "l b", anchor="middle", color="var(--accent)")
    for x0, lines in ((212, ["USB, Bluetooth", "or dongle"]), (478, ["local connection,", "same computer"])):
        arrow(s, (x0 + 10, 100), (x0 + 60, 100), "var(--fg)", 2.5, head=11)
        for i, t in enumerate(lines):
            text(s, x0 + 35, 196 + i * 17, t, "s m", anchor="middle")
    s.save("data-flow.svg")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--simple-icons", required=True, help="simple-icons icons/ directory")
    parser.add_argument("--emblem", required=True, help="Odyssey Arm emblem PNG")
    parser.add_argument("--out", default=OUT, help="output directory (default: next to this script)")
    args = parser.parse_args()
    SI, EMBLEM, OUT = args.simple_icons, args.emblem, args.out
    for diagram in (coords_3d, engines, aimpoint, device_axes, zeroing, shot_timing, data_flow):
        diagram()
    print("wrote", ", ".join(sorted(n for n in os.listdir(OUT) if n.endswith(".svg"))))
