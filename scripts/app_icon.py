#!/usr/bin/env python3
"""Draw the GTK app's icon: three meshed metal gears in a well of a metal plate, the big one with
a sync sign stamped into it, between a cloud and a folder. Writes the full-color icon on the GNOME
128px template grid, a 512px PNG of it next to it (with rsvg-convert) and its 16px symbolic.

    scripts/app_icon.py
"""

import math
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parent.parent
ICONS = ROOT / "data" / "icons" / "hicolor"
APP_ID = "dev.aperte.Conflux"
PNG_SIZE = 512

# Big gear on top, two small ones below it at +-THETA from vertical, in a round well of the plate.
BIG = dict(cx=64, cy=52, n=12, ro=28, rr=22.5)
SMALL = dict(n=8, ro=18.5, rr=13.5)
THETA = math.radians(45)
WELL = (64, 62, 46)
DEPTH = 2  # height of the gears' front profile
# GNOME palette
COLOR = dict(plate_front="#9a9996", wall="#241f31", floor="#3d3846", symbolic="#222222")
# Stamps pressed into the gears: (floor, top wall in shadow), tinted with green, blue and yellow
STAMP = dict(
    sync=("#6f9a7f", "#2f4a3a"),
    cloud=("#7891ad", "#2f3f55"),
    folder=("#a89a6c", "#4f4426"),
    folder_front="#bdae7c",
)
STAMP_WALL = 1.2  # how much of the wall under the top edges shows
# Gradients: id, (x1, y1, x2, y2) in the box of the shape, stops
GRADIENTS = [
    ("plate", (0, 0, 0.35, 1), [(0, "#f6f5f4"), (0.45, "#deddda"), (0.55, "#d0cfcc"),
                                (1, "#c0bfbc")]),
    ("gear", (0, 0, 1, 1), [(0, "#ffffff"), (0.4, "#deddda"), (0.55, "#f6f5f4"),
                            (1, "#c0bfbc")]),
    ("gear-front", (0, 0, 1, 0), [(0, "#3d3846"), (0.5, "#77767b"), (1, "#3d3846")]),
]


def num(v: float) -> str:
    s = f"{v:.2f}".rstrip("0").rstrip(".")
    return "0" if s == "-0" else s


def point(cx: float, cy: float, r: float, a: float) -> str:
    return f"{num(cx + r * math.cos(a))} {num(cy + r * math.sin(a))}"


def circle(cx: float, cy: float, r: float, sweep: int = 1) -> str:
    return (f"M{num(cx - r)} {num(cy)}A{num(r)} {num(r)} 0 1 {sweep} {num(cx + r)} {num(cy)}"
            f"A{num(r)} {num(r)} 0 1 {sweep} {num(cx - r)} {num(cy)}Z")


def rect(x: float, y: float, w: float, h: float, r: float) -> str:
    return (f"M{num(x + r)} {num(y)}h{num(w - 2 * r)}a{num(r)} {num(r)} 0 0 1 {num(r)} {num(r)}"
            f"v{num(h - 2 * r)}a{num(r)} {num(r)} 0 0 1 {num(-r)} {num(r)}h{num(2 * r - w)}"
            f"a{num(r)} {num(r)} 0 0 1 {num(-r)} {num(-r)}v{num(2 * r - h)}"
            f"a{num(r)} {num(r)} 0 0 1 {num(r)} {num(-r)}Z")


def gear(cx: float, cy: float, n: int, ro: float, rr: float, phase: float,
         tip: float = 0.36, base: float = 0.56) -> str:
    """A gear of n trapezoid teeth, tip and base given as parts of the pitch, the first tooth
    pointing at the angle `phase`."""
    step = 2 * math.pi / n
    d = []
    for i in range(n):
        a = phase + i * step
        if i == 0:
            d.append(f"M{point(cx, cy, rr, a - base * step / 2)}")
        d.append(f"L{point(cx, cy, ro, a - tip * step / 2)}")
        d.append(f"A{num(ro)} {num(ro)} 0 0 1 {point(cx, cy, ro, a + tip * step / 2)}")
        d.append(f"L{point(cx, cy, rr, a + base * step / 2)}")
        d.append(f"A{num(rr)} {num(rr)} 0 0 1 {point(cx, cy, rr, a + step - base * step / 2)}")
    return "".join(d) + "Z"


def sync(cx: float, cy: float, ro: float, w: float) -> str:
    """Two half rings of width w and outer radius ro, each ending in a bar pointing inwards: the
    top one runs from the left over the top to a bar on the right, the bottom one is the same
    turned half a circle."""
    ri = ro - w
    bottom = cy + 0.13 * w  # of the right bar
    top = bottom - w
    left = cy - 0.13 * w - 0.35 * w  # end of the top ring on the left
    points = [
        (cx - math.sqrt(ro**2 - (left - cy) ** 2), left),
        (cx + math.sqrt(ro**2 - (bottom - cy) ** 2), bottom),
        (cx + 0.26 * ro, bottom),
        (cx + 0.26 * ro, top),
        (cx + math.sqrt(ri**2 - (top - cy) ** 2), top),
        (cx - math.sqrt(ri**2 - (left - cy) ** 2), left),
    ]

    # the outer arc runs clockwise from the left end over the top to the right bar
    span = math.pi + math.asin((left - cy) / ro) + math.asin((bottom - cy) / ro)
    large = 1 if span > math.pi else 0

    def half(p: list) -> str:
        p = [f"{num(x)} {num(y)}" for x, y in p]
        return (f"M{p[0]}A{num(ro)} {num(ro)} 0 {large} 1 {p[1]}L{p[2]}L{p[3]}L{p[4]}"
                f"A{num(ri)} {num(ri)} 0 0 0 {p[5]}Z")

    return half(points) + half([(2 * cx - x, 2 * cy - y) for x, y in points])


def cloud(cx: float, cy: float, s: float) -> str:
    return (circle(cx - 0.42 * s, cy + 0.12 * s, 0.42 * s)
            + circle(cx + 0.12 * s, cy - 0.12 * s, 0.6 * s)
            + circle(cx + 0.58 * s, cy + 0.2 * s, 0.34 * s)
            + f"M{num(cx - 0.42 * s)} {num(cy + 0.12 * s)}h{num(s)}v{num(0.42 * s)}h{num(-s)}Z")


def folder(cx: float, cy: float, s: float) -> tuple[str, str]:
    """The back of a folder (with its tab) and its front."""
    w, h, r = 2 * s, 1.5 * s, 0.14 * s
    x, y = cx - w / 2, cy - h / 2
    back = rect(x, y - 0.22 * s, 0.8 * s, h, r) + rect(x, y, w, h, r)
    return back, rect(x, y + 0.3 * s, w, h - 0.3 * s, r)


def gears() -> list[dict]:
    """The big gear with a tooth up, and the small ones turned to mesh with it."""
    big = dict(BIG, phase=-math.pi / 2)
    distance = (BIG["ro"] + BIG["rr"]) / 2 + (SMALL["ro"] + SMALL["rr"]) / 2
    result = [big]
    for side in (-1, 1):
        cx = BIG["cx"] + side * distance * math.sin(THETA)
        cy = BIG["cy"] + distance * math.cos(THETA)
        towards = math.atan2(cy - BIG["cy"], cx - BIG["cx"])
        teeth = (towards - big["phase"]) / (2 * math.pi / BIG["n"])
        big_tooth = abs(teeth - round(teeth)) < 0.25
        # a gap of the small gear faces a tooth of the big one, and the other way round
        phase = towards + math.pi + (math.pi / SMALL["n"] if big_tooth else 0)
        result.append(dict(SMALL, cx=cx, cy=cy, phase=phase))
    return result


def gradient(name: str, box: tuple, stops: list) -> str:
    x1, y1, x2, y2 = box
    out = [f'<linearGradient id="{name}" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">']
    for offset, color in stops:
        out.append(f'<stop offset="{offset}" stop-color="{color}"/>')
    return "".join(out) + "</linearGradient>"


def stamp(name: str, d: str, colors: tuple) -> tuple[list, list]:
    """Defs and layers of the shape d pressed into a surface lit from above: the wall under its
    top edges in shadow, the floor below it."""
    floor, shadow = colors
    defs = [
        f'<clipPath id="{name}-down"><path d="{d}" transform="translate(0 {STAMP_WALL})"/></clipPath>',
    ]
    layers = [
        f'<path d="{d}" fill="{shadow}"/>',
        f'<path d="{d}" clip-path="url(#{name}-down)" fill="{floor}"/>',
    ]
    return defs, layers


def icon() -> str:
    wx, wy, wr = WELL
    big, left, right = gears()
    back, front = folder(right["cx"], right["cy"] + 1, 8)
    stamps = [
        stamp("sync", sync(big["cx"], big["cy"], 15.5, 5), STAMP["sync"]),
        stamp("cloud", cloud(left["cx"] - 1, left["cy"] + 1, 10.5), STAMP["cloud"]),
        stamp("folder", back, STAMP["folder"]),
    ]
    c = COLOR
    lines = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">',
        "<defs>",
        *(gradient(*g) for g in GRADIENTS),
        f'<clipPath id="well"><circle cx="{wx}" cy="{wy}" r="{wr}"/></clipPath>',
        *(line for defs, _ in stamps for line in defs),
        "</defs>",
        f'<rect x="12" y="12" width="104" height="104" rx="16" fill="{c["plate_front"]}"/>',
        '<rect x="12" y="12" width="104" height="100" rx="16" fill="url(#plate)"/>',
        f'<circle cx="{wx}" cy="{wy}" r="{wr}" fill="{c["wall"]}"/>',
        '<g clip-path="url(#well)">',
        f'<circle cx="{wx}" cy="{wy + 4}" r="{wr}" fill="{c["floor"]}"/>',
    ]
    for g in (left, right, big):
        d = gear(g["cx"], g["cy"] + DEPTH, g["n"], g["ro"], g["rr"], g["phase"])
        lines.append(f'<path d="{d}" fill="url(#gear-front)"/>')
    for g in (left, right, big):
        d = gear(g["cx"], g["cy"], g["n"], g["ro"], g["rr"], g["phase"])
        lines.append(f'<path d="{d}" fill="url(#gear)"/>')
    lines += [
        *(line for _, layers in stamps for line in layers),
        # the folder's front flap, lying on the floor of its stamp
        f'<path d="{front}" clip-path="url(#folder-down)" fill="{STAMP["folder_front"]}"/>',
        "</g>",
        "</svg>",
    ]
    return "\n".join(lines) + "\n"


def symbolic() -> str:
    """The big gear alone, as a ring around the sync sign."""
    ring = gear(8, 8, 8, 8, 6.6, -math.pi / 8, tip=0.42, base=0.62) + circle(8, 8, 5.3, sweep=0)
    fill = COLOR["symbolic"]
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">\n'
            f'<path d="{ring}" fill="{fill}"/>\n'
            f'<path d="{sync(8, 8, 4.4, 1.5)}" fill="{fill}"/>\n'
            '</svg>\n')


def main() -> None:
    for kind, name, text in (("scalable", f"{APP_ID}.svg", icon()),
                             ("symbolic", f"{APP_ID}-symbolic.svg", symbolic())):
        path = ICONS / kind / "apps" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        print(path.relative_to(ROOT))
    # a PNG of the full-color icon, for what can't use the SVG
    svg = ICONS / "scalable" / "apps" / f"{APP_ID}.svg"
    path = svg.with_suffix(".png")
    size = str(PNG_SIZE)
    subprocess.run(["rsvg-convert", "-w", size, "-h", size, "-o", str(path), str(svg)], check=True)
    print(path.relative_to(ROOT))


if __name__ == "__main__":
    main()
