#!/usr/bin/env python3
"""Draw the GTK app's icon: three meshed gears in a well of a rounded plate, the big one carrying
sync arrows between a cloud and a folder. Writes the full-color icon on the GNOME 128px template
grid and its 16px symbolic.

    scripts/app_icon.py
"""

import math
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
ICONS = ROOT / "data" / "icons" / "hicolor"
APP_ID = "dev.aperte.Conflux"

# Big gear on top, two small ones below it at +-THETA from vertical, in a round well of the plate.
BIG = dict(cx=64, cy=52, n=12, ro=28, rr=22.5)
SMALL = dict(n=8, ro=18.5, rr=13.5)
THETA = math.radians(45)
WELL = (64, 62, 46)
DEPTH = 3  # height of the gears' front profile
# GNOME palette
COLOR = dict(
    plate="#3584e4", plate_front="#1a5fb4", wall="#241f31", floor="#3d3846",
    gear="#f6f5f4", gear_front="#9a9996", hub="#deddda",
    sync="#1c71d8", cloud="#3584e4", folder_back="#1a5fb4", folder="#3584e4",
    symbolic="#222222",
)


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


def arrow(cx: float, cy: float, r: float, w: float, a0: float, a1: float, head: float) -> str:
    """An arc of width w around (cx, cy) from a0 clockwise to a1, ending in an arrowhead."""
    ri, ro = r - w / 2, r + w / 2
    large = 1 if a1 - a0 > math.pi else 0
    return (f"M{point(cx, cy, ro, a0)}A{num(ro)} {num(ro)} 0 {large} 1 {point(cx, cy, ro, a1)}"
            f"L{point(cx, cy, r + head, a1)}L{point(cx, cy, r, a1 + head * 1.15 / r)}"
            f"L{point(cx, cy, r - head, a1)}L{point(cx, cy, ri, a1)}"
            f"A{num(ri)} {num(ri)} 0 {large} 0 {point(cx, cy, ri, a0)}Z")


def sync(cx: float, cy: float, r: float, w: float, head: float) -> str:
    gap = math.radians(34)
    return (arrow(cx, cy, r, w, math.pi + gap / 2, 2 * math.pi - gap, head)
            + arrow(cx, cy, r, w, gap / 2, math.pi - gap, head))


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


def icon() -> str:
    wx, wy, wr = WELL
    big, left, right = gears()
    back, front = folder(right["cx"], right["cy"] + 1, 8)
    c = COLOR
    lines = [
        '<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128">',
        f'<defs><clipPath id="well"><circle cx="{wx}" cy="{wy}" r="{wr}"/></clipPath></defs>',
        f'<rect x="12" y="12" width="104" height="104" rx="16" fill="{c["plate_front"]}"/>',
        f'<rect x="12" y="12" width="104" height="100" rx="16" fill="{c["plate"]}"/>',
        f'<circle cx="{wx}" cy="{wy}" r="{wr}" fill="{c["wall"]}"/>',
        '<g clip-path="url(#well)">',
        f'<circle cx="{wx}" cy="{wy + 4}" r="{wr}" fill="{c["floor"]}"/>',
    ]
    for g in (left, right, big):
        d = gear(g["cx"], g["cy"] + DEPTH, g["n"], g["ro"], g["rr"], g["phase"])
        lines.append(f'<path d="{d}" fill="{c["gear_front"]}"/>')
    for g in (left, right, big):
        d = gear(g["cx"], g["cy"], g["n"], g["ro"], g["rr"], g["phase"])
        lines.append(f'<path d="{d}" fill="{c["gear"]}"/>')
    lines += [
        '</g>',
        f'<path d="{circle(big["cx"], big["cy"], 16.5)}" fill="{c["hub"]}"/>',
        f'<path d="{circle(big["cx"], big["cy"], 15.5)}" fill="{c["gear"]}"/>',
        f'<path d="{sync(big["cx"], big["cy"], 10.5, 5, 6)}" fill="{c["sync"]}"/>',
        f'<path d="{cloud(left["cx"] + 0.5, left["cy"] + 1, 9.5)}" fill="{c["cloud"]}"/>',
        f'<path d="{back}" fill="{c["folder_back"]}"/>',
        f'<path d="{front}" fill="{c["folder"]}"/>',
        '</svg>',
    ]
    return "\n".join(lines) + "\n"


def symbolic() -> str:
    """The big gear alone, as a ring around the sync arrows."""
    ring = gear(8, 8, 8, 8, 6.6, -math.pi / 8, tip=0.42, base=0.62) + circle(8, 8, 5.3, sweep=0)
    fill = COLOR["symbolic"]
    return ('<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" viewBox="0 0 16 16">\n'
            f'<path d="{ring}" fill="{fill}"/>\n'
            f'<path d="{sync(8, 8, 3.1, 1.4, 1.5)}" fill="{fill}"/>\n'
            '</svg>\n')


def main() -> None:
    for kind, name, text in (("scalable", f"{APP_ID}.svg", icon()),
                             ("symbolic", f"{APP_ID}-symbolic.svg", symbolic())):
        path = ICONS / kind / "apps" / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        print(path.relative_to(ROOT))


if __name__ == "__main__":
    main()
