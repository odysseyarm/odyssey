#!/usr/bin/env python3
"""Writes the print artwork for the scale board described by ../board.json.

    python3 board_artwork.py [--out-dir DIR]
    python3 board_artwork.py --screen-px 3200x2400 --screen-diagonal-in 13.3 [--out-dir DIR]

Four files, each as a vector PDF and SVG at exactly its physical size (print
at 100%, "actual size", never "fit to page"):

  scale-board-rigid     12 x 8.5 in, for a print shop to print straight onto a
                        rigid board (white background laid down explicitly)
  scale-board-paper-letter   US Letter landscape, for an office printer
  scale-board-paper-a4       A4 landscape, for an office printer
  scale-board-rigid-large    the rigid board at twice the size, 24 x 17 in

With --screen-px and --screen-diagonal-in it instead writes one file, sized for
showing the board on a screen of that resolution and nominal diagonal (its
physical size follows from those two; it is only as exact as the diagonal is,
so check the bar with a rule, as for any print). Render it to a PNG at exactly
the screen's resolution and show it at 1:1.

All of them carry the same dots: the grid in board.json, the same pitch and dot
size, with the dots listed under "missing" left out (that breaks the grid's 180
degree symmetry), and a scale bar to check the print against a rule. Only the
space around the dots differs, so one detector reads all of them. No text and
no embedded fonts, so nothing can be substituted at the print shop. Pure
standard library.
"""

import argparse
import json
import math
import pathlib
import sys

MM_PER_INCH = 25.4
PT_PER_INCH = 72.0
# Control-point distance that makes four cubic Beziers a close circle.
KAPPA = 0.5522847498307936

BAR_THICKNESS_MM = 0.8
BAR_TICK_MM = 6.0
# Gap kept between the dots and the scale bar, mm.
BAR_CLEARANCE_MM = 3.0
# Office printers cannot print to the paper's edge; most leave about 6 mm.
PAPER_MARGIN_MM = 6.35

# name -> page (w, h) mm, white background laid down, margin that must stay clear,
# and (dots' top edge, scale bar centerline) measured from the page's top, or None
# to center the layout the way board.json does.
LAYOUTS = {
    "scale-board-rigid": {"page": None, "white": True, "margin": 2.0, "top": None, "bar_y": None},
    # Twice the size, for a camera that sees the board from farther away: the dots
    # must be about 12 px across in the color image (see BOARD.md).
    "scale-board-rigid-large": {
        "page": None,
        "white": True,
        "margin": 2.0,
        "top": None,
        "bar_y": None,
        "scale": 2.0,
    },
    "scale-board-paper-letter": {
        "page": (11 * MM_PER_INCH, 8.5 * MM_PER_INCH),
        "white": False,
        "margin": PAPER_MARGIN_MM,
        "top": 12.0,
        "bar_y": 203.0,
    },
    "scale-board-paper-a4": {
        "page": (297.0, 210.0),
        "white": False,
        "margin": PAPER_MARGIN_MM,
        "top": 8.0,
        "bar_y": 198.0,
    },
}


def load_spec(path):
    spec = json.loads(pathlib.Path(path).read_text())
    spec["missing"] = {tuple(m) for m in spec["missing"]}
    return spec


def scaled(spec, factor):
    """The same board with every length multiplied by `factor`."""
    out = dict(spec)
    for key in ("pitch_mm", "dot_diameter_mm", "scale_bar_mm"):
        out[key] = spec[key] * factor
    out["board_mm"] = [v * factor for v in spec["board_mm"]]
    return out


def layout(spec, name):
    """Everything the art needs for one file, in millimeters from the page's top-left."""
    lay = LAYOUTS[name]
    cols, rows, pitch = spec["cols"], spec["rows"], spec["pitch_mm"]
    radius = spec["dot_diameter_mm"] / 2.0
    page_w, page_h = lay["page"] or tuple(spec["board_mm"])
    top_edge = lay["top"] if lay["top"] is not None else (page_h - ((rows - 1) * pitch + 2 * radius)) / 2.0
    left = (page_w - (cols - 1) * pitch) / 2.0
    centers = [
        (left + c * pitch, top_edge + radius + r * pitch)
        for r in range(rows)
        for c in range(cols)
        if (c, r) not in spec["missing"]
    ]
    length = spec["scale_bar_mm"]
    bar_y = lay["bar_y"] if lay["bar_y"] is not None else page_h - 8.0
    bar = ((page_w - length) / 2.0, (page_w + length) / 2.0, bar_y)
    return {"page": (page_w, page_h), "dots": centers, "bar": bar, "white": lay["white"], "margin": lay["margin"]}


def check(spec, name, lay):
    """Refuses art that would not fit, would be clipped by a printer, or is ambiguous."""
    page_w, page_h = lay["page"]
    radius = spec["dot_diameter_mm"] / 2.0
    margin = lay["margin"]
    expected = spec["cols"] * spec["rows"] - len(spec["missing"])
    assert len(lay["dots"]) == expected, f"{name}: a missing dot lies outside the grid"
    assert spec["missing"], "without a missing dot the board looks the same turned 180 degrees"
    assert spec["dot_diameter_mm"] < spec["pitch_mm"], "dots would touch"
    for x, y in lay["dots"]:
        assert margin <= x - radius and x + radius <= page_w - margin, f"{name}: a dot is too close to a side"
        assert margin <= y - radius and y + radius <= page_h - margin, f"{name}: a dot is too close to the top or bottom"
    x0, x1, y = lay["bar"]
    lowest = max(cy for _, cy in lay["dots"]) + radius
    assert x0 >= margin and x1 <= page_w - margin, f"{name}: the scale bar is too wide for the page"
    assert y - BAR_TICK_MM / 2.0 >= lowest + BAR_CLEARANCE_MM, f"{name}: the scale bar crowds the dots"
    assert y + BAR_TICK_MM / 2.0 <= page_h - margin, f"{name}: the scale bar is too close to the bottom"


def svg(spec, lay):
    width, height = lay["page"]
    radius = spec["dot_diameter_mm"] / 2.0
    x0, x1, y = lay["bar"]
    t = BAR_THICKNESS_MM
    out = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width:.4f}mm" height="{height:.4f}mm" '
        f'viewBox="0 0 {width:.4f} {height:.4f}">',
    ]
    if lay["white"]:
        out.append(f'<rect x="0" y="0" width="{width:.4f}" height="{height:.4f}" fill="#ffffff"/>')
    out.append('<g fill="#000000" stroke="none">')
    out += [f'<circle cx="{x:.4f}" cy="{cy:.4f}" r="{radius}"/>' for x, cy in lay["dots"]]
    out.append(f'<rect x="{x0:.4f}" y="{y - t / 2:.4f}" width="{x1 - x0:.4f}" height="{t}"/>')
    for x in (x0, x1):
        out.append(
            f'<rect x="{x - t / 2:.4f}" y="{y - BAR_TICK_MM / 2:.4f}" width="{t}" height="{BAR_TICK_MM}"/>'
        )
    out += ["</g>", "</svg>", ""]
    return "\n".join(out)


def pdf(spec, lay):
    width, height = lay["page"]
    pt = PT_PER_INCH / MM_PER_INCH
    radius = spec["dot_diameter_mm"] / 2.0 * pt
    page_w, page_h = width * pt, height * pt
    ops = []
    if lay["white"]:
        ops += ["1 g", f"0 0 {page_w:.4f} {page_h:.4f} re f"]
    ops.append("0 g")  # DeviceGray black

    def flip(y_mm):
        return page_h - y_mm * pt

    for x_mm, y_mm in lay["dots"]:
        cx, cy, k = x_mm * pt, flip(y_mm), radius * KAPPA
        ops += [
            f"{cx + radius:.4f} {cy:.4f} m",
            f"{cx + radius:.4f} {cy + k:.4f} {cx + k:.4f} {cy + radius:.4f} {cx:.4f} {cy + radius:.4f} c",
            f"{cx - k:.4f} {cy + radius:.4f} {cx - radius:.4f} {cy + k:.4f} {cx - radius:.4f} {cy:.4f} c",
            f"{cx - radius:.4f} {cy - k:.4f} {cx - k:.4f} {cy - radius:.4f} {cx:.4f} {cy - radius:.4f} c",
            f"{cx + k:.4f} {cy - radius:.4f} {cx + radius:.4f} {cy - k:.4f} {cx + radius:.4f} {cy:.4f} c",
            "f",
        ]
    x0, x1, y = lay["bar"]
    t = BAR_THICKNESS_MM
    rect = lambda x, y_top, w, h: f"{x * pt:.4f} {flip(y_top + h):.4f} {w * pt:.4f} {h * pt:.4f} re f"
    ops.append(rect(x0, y - t / 2, x1 - x0, t))
    for x in (x0, x1):
        ops.append(rect(x - t / 2, y - BAR_TICK_MM / 2, t, BAR_TICK_MM))
    stream = "\n".join(ops).encode("ascii")

    objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_w:.4f} {page_h:.4f}] "
        f"/TrimBox [0 0 {page_w:.4f} {page_h:.4f}] /Contents 4 0 R /Resources << >> >>".encode("ascii"),
        b"<< /Length " + str(len(stream)).encode() + b" >>\nstream\n" + stream + b"\nendstream",
    ]
    body = b"%PDF-1.4\n"
    offsets = []
    for number, obj in enumerate(objects, start=1):
        offsets.append(len(body))
        body += f"{number} 0 obj\n".encode() + obj + b"\nendobj\n"
    xref = len(body)
    body += f"xref\n0 {len(objects) + 1}\n0000000000 65535 f \n".encode()
    for offset in offsets:
        body += f"{offset:010d} 00000 n \n".encode()
    body += f"trailer\n<< /Size {len(objects) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return body


def screen_layout(spec, px_w, px_h, diagonal_in):
    """Layout for showing the board on a screen, whose size follows from its resolution and nominal diagonal."""
    hyp = math.hypot(px_w, px_h)
    page = (diagonal_in * MM_PER_INCH * px_w / hyp, diagonal_in * MM_PER_INCH * px_h / hyp)
    top = 4.0
    dots_bottom = top + (spec["rows"] - 1) * spec["pitch_mm"] + spec["dot_diameter_mm"]
    bar_y = dots_bottom + BAR_CLEARANCE_MM + BAR_TICK_MM / 2.0 + 1.0
    return {"page": page, "white": True, "margin": 3.0, "top": top, "bar_y": bar_y}


def main():
    here = pathlib.Path(__file__).resolve().parent
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--spec", default=here.parent / "board.json")
    parser.add_argument("--out-dir", default=".")
    parser.add_argument("--screen-px", metavar="WxH", help="resolution of a screen to show the board on")
    parser.add_argument("--screen-diagonal-in", type=float, help="that screen's nominal diagonal, inches")
    args = parser.parse_args()
    if bool(args.screen_px) != bool(args.screen_diagonal_in):
        parser.error("--screen-px and --screen-diagonal-in go together")

    base = load_spec(args.spec)
    out = pathlib.Path(args.out_dir)
    out.mkdir(parents=True, exist_ok=True)
    names = list(LAYOUTS)
    if args.screen_px:
        try:
            px_w, px_h = (int(v) for v in args.screen_px.lower().split("x"))
        except ValueError:
            parser.error("--screen-px takes a resolution like 3200x2400")
        name = f"scale-board-screen-{px_w}x{px_h}"
        LAYOUTS[name] = screen_layout(base, px_w, px_h, args.screen_diagonal_in)
        names = [name]
    for name in names:
        spec = scaled(base, LAYOUTS[name].get("scale", 1.0))
        lay = layout(spec, name)
        try:
            check(spec, name, lay)
        except AssertionError as refusal:
            sys.exit(f"cannot make {name}: {refusal}")
        (out / f"{name}.svg").write_text(svg(spec, lay))
        (out / f"{name}.pdf").write_bytes(pdf(spec, lay))
        w, h = lay["page"]
        print(
            f"{name}: {len(lay['dots'])} dots of {spec['dot_diameter_mm']:g} mm at {spec['pitch_mm']:g} mm pitch, "
            f"page {w:.1f} x {h:.1f} mm ({w / MM_PER_INCH:.2f} x {h / MM_PER_INCH:.2f} in), "
            f"{spec['scale_bar_mm']:g} mm scale bar"
        )


if __name__ == "__main__":
    sys.exit(main())
