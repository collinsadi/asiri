#!/usr/bin/env python3
"""Draws Asiri's app icon and menu bar glyphs (architecture §13.8).

The mark is a round speech bubble whose tail is a keyhole's slot, so the whole bubble reads as a
keyhole, with three "typing" dots cut through it, on a deep indigo squircle.

    tools/design/make-icon.py            # writes the SVG masters and every PNG size
    tools/design/make-icon.py --check    # fails if the committed files are out of date

Needs rsvg-convert (librsvg) to render PNGs.
"""
import math
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
ICONSET = ROOT / "apps/macos/Asiri/Resources/Assets.xcassets/AppIcon.appiconset"
ASSETS = ROOT / "apps/macos/Asiri/Resources/Assets.xcassets"
MASTER = ROOT / "design/icon"

# macOS app icon sizes: (points, scale)
SIZES = [(16, 1), (16, 2), (32, 1), (32, 2), (128, 1), (128, 2), (256, 1), (256, 2), (512, 1), (512, 2)]


def fmt(v: float) -> str:
    return f"{v:.2f}".rstrip("0").rstrip(".")


def squircle(cx: float, cy: float, half: float, n: float = 5.0, steps: int = 360) -> str:
    """A superellipse |x|^n + |y|^n = 1, close to Apple's continuous-corner icon shape."""
    pts = []
    for i in range(steps):
        t = 2 * math.pi * i / steps
        c, s = math.cos(t), math.sin(t)
        x = cx + half * math.copysign(abs(c) ** (2 / n), c)
        y = cy + half * math.copysign(abs(s) ** (2 / n), s)
        pts.append(f"{fmt(x)},{fmt(y)}")
    return "M" + " L".join(pts) + " Z"


def keyhole_bubble(cx: float, cy: float, r: float, slot_top: float, slot_bottom: float, bottom_y: float, corner: float,
                   ry: float | None = None) -> str:
    """The bubble: a round head (the keyhole's, slightly wider than tall, like a speech bubble)
    and a flared slot below it (the bubble's tail).

    slot_top and slot_bottom are the slot's half-widths where it leaves the head and at its foot.
    """
    ry = r if ry is None else ry
    # Where each slanted side meets the circle: walk up the side until it's inside the circle.
    def side_point(sign: float) -> tuple[float, float]:
        x0, y0 = cx + sign * slot_bottom, bottom_y
        x1, y1 = cx + sign * slot_top, cy
        lo, hi = 0.0, 1.0
        for _ in range(60):
            mid = (lo + hi) / 2
            x, y = x0 + (x1 - x0) * mid, y0 + (y1 - y0) * mid
            if ((x - cx) / r) ** 2 + ((y - cy) / ry) ** 2 > 1:
                lo = mid
            else:
                hi = mid
        return x0 + (x1 - x0) * hi, y0 + (y1 - y0) * hi

    rx, jy = side_point(1)
    lx, ly = side_point(-1)
    # Rounded foot corners.
    dx = slot_bottom - slot_top
    length = math.hypot(dx, bottom_y - cy)
    ux, uy = dx / length, (bottom_y - cy) / length  # unit vector down the right side
    p = []
    p.append(f"M{fmt(lx)},{fmt(ly)}")
    # The head: the long way round the top of the circle, from left junction to right junction.
    p.append(f"A{fmt(r)},{fmt(ry)} 0 1 1 {fmt(rx)},{fmt(jy)}")
    # Right side down to the corner.
    ax, ay = cx + slot_bottom - ux * corner, bottom_y - uy * corner
    p.append(f"L{fmt(ax)},{fmt(ay)}")
    p.append(f"Q{fmt(cx + slot_bottom)},{fmt(bottom_y)} {fmt(cx + slot_bottom - corner)},{fmt(bottom_y)}")
    # Foot.
    p.append(f"L{fmt(cx - slot_bottom + corner)},{fmt(bottom_y)}")
    bx, by = cx - slot_bottom + ux * corner, bottom_y - uy * corner
    p.append(f"Q{fmt(cx - slot_bottom)},{fmt(bottom_y)} {fmt(bx)},{fmt(by)}")
    p.append("Z")
    return " ".join(p)


def dots(cx: float, cy: float, radius: float, gap: float) -> list[tuple[float, float, float]]:
    return [(cx - gap, cy, radius), (cx, cy, radius), (cx + gap, cy, radius)]


def app_icon_svg() -> str:
    body = squircle(512, 512, 412)
    # Keep in step with KeyholeBubble in AsiriDesign/Imaging.swift (same proportions).
    bubble = keyhole_bubble(cx=512, cy=432, r=238, slot_top=94, slot_bottom=138, bottom_y=796, corner=46)
    holes = "".join(f'<circle cx="{fmt(x)}" cy="{fmt(y)}" r="{fmt(r)}" fill="#000"/>' for x, y, r in dots(512, 432, 39, 114))
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <title>Asiri</title>
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0.35" y2="1">
      <stop offset="0" stop-color="#5A51F2"/>
      <stop offset="0.55" stop-color="#3B32C4"/>
      <stop offset="1" stop-color="#1F1873"/>
    </linearGradient>
    <radialGradient id="glow" cx="0.5" cy="0.08" r="0.75">
      <stop offset="0" stop-color="#A9A4FF" stop-opacity="0.55"/>
      <stop offset="1" stop-color="#A9A4FF" stop-opacity="0"/>
    </radialGradient>
    <radialGradient id="depth" cx="0.5" cy="1.05" r="0.7">
      <stop offset="0" stop-color="#0B0826" stop-opacity="0.55"/>
      <stop offset="1" stop-color="#0B0826" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="paper" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#FFFFFF"/>
      <stop offset="1" stop-color="#E3E1FF"/>
    </linearGradient>
    <linearGradient id="rim" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#FFFFFF" stop-opacity="0.35"/>
      <stop offset="0.2" stop-color="#FFFFFF" stop-opacity="0"/>
    </linearGradient>
    <filter id="iconShadow" x="-10%" y="-10%" width="120%" height="125%">
      <feGaussianBlur in="SourceAlpha" stdDeviation="14"/>
      <feOffset dy="12"/>
      <feComponentTransfer><feFuncA type="linear" slope="0.32"/></feComponentTransfer>
      <feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
    </filter>
    <filter id="liftShadow" x="-20%" y="-20%" width="140%" height="150%">
      <feGaussianBlur in="SourceAlpha" stdDeviation="24" result="blur"/>
      <feOffset in="blur" dy="22" result="offset"/>
      <feFlood flood-color="#0B0830" flood-opacity="0.5" result="ink"/>
      <feComposite in="ink" in2="offset" operator="in" result="shadow"/>
      <feMerge><feMergeNode in="shadow"/><feMergeNode in="SourceGraphic"/></feMerge>
    </filter>
    <mask id="cut" maskUnits="userSpaceOnUse" x="0" y="0" width="1024" height="1024">
      <rect width="1024" height="1024" fill="#fff"/>
      {holes}
    </mask>
    <clipPath id="body"><path d="{body}"/></clipPath>
  </defs>
  <g filter="url(#iconShadow)">
    <path d="{body}" fill="url(#bg)"/>
  </g>
  <g clip-path="url(#body)">
    <rect width="1024" height="1024" fill="url(#glow)"/>
    <rect width="1024" height="1024" fill="url(#depth)"/>
    <path d="{body}" fill="none" stroke="url(#rim)" stroke-width="6"/>
    <g filter="url(#liftShadow)">
      <path d="{bubble}" fill="url(#paper)" mask="url(#cut)"/>
    </g>
  </g>
</svg>
"""


def glyph_svg(unread: bool) -> str:
    """The menu bar glyph: the same bubble as a template image (black, 18×18 points)."""
    s = 18 / 1024
    bubble = keyhole_bubble(cx=512, cy=410, r=318, slot_top=124, slot_bottom=184, bottom_y=968, corner=60)
    holes = "".join(f'<circle cx="{fmt(x)}" cy="{fmt(y)}" r="{fmt(r)}" fill="#000"/>' for x, y, r in dots(512, 410, 54, 150))
    badge = ""
    if unread:
        # A dot at the top right, with a clear ring cut around it.
        holes += '<circle cx="880" cy="150" r="190" fill="#000"/>'
        badge = '<circle cx="880" cy="150" r="130" fill="#000"/>'
    return f"""<svg xmlns="http://www.w3.org/2000/svg" width="18" height="18" viewBox="0 0 18 18">
  <defs>
    <mask id="cut" maskUnits="userSpaceOnUse" x="0" y="0" width="1024" height="1024">
      <rect width="1024" height="1024" fill="#fff"/>
      {holes}
    </mask>
  </defs>
  <g transform="scale({s:.6f})">
    <path d="{bubble}" fill="#000" mask="url(#cut)"/>
    {badge}
  </g>
</svg>
"""


def imageset(name: str, svg: str) -> dict:
    return {name + ".svg": svg, "Contents.json": IMAGESET_JSON.replace("@NAME@", name)}


IMAGESET_JSON = """{
  "images" : [
    {
      "filename" : "@NAME@.svg",
      "idiom" : "universal"
    }
  ],
  "info" : {
    "author" : "xcode",
    "version" : 1
  },
  "properties" : {
    "preserves-vector-representation" : true,
    "template-rendering-intent" : "template"
  }
}
"""

APPICONSET_JSON = """{
  "images" : [
@IMAGES@
  ],
  "info" : {
    "author" : "xcode",
    "version" : 1
  }
}
"""


def appiconset_json() -> str:
    images = []
    for pt, scale in SIZES:
        suffix = "" if scale == 1 else "@2x"
        images.append(
            "    {\n"
            f'      "filename" : "icon_{pt}x{pt}{suffix}.png",\n'
            '      "idiom" : "mac",\n'
            f'      "scale" : "{scale}x",\n'
            f'      "size" : "{pt}x{pt}"\n'
            "    }"
        )
    return APPICONSET_JSON.replace("@IMAGES@", ",\n".join(images))


def render(svg_path: pathlib.Path, png_path: pathlib.Path, pixels: int) -> None:
    subprocess.run(["rsvg-convert", "-w", str(pixels), "-h", str(pixels), "-o", str(png_path), str(svg_path)], check=True)


def build(out: pathlib.Path) -> None:
    (out / "design/icon").mkdir(parents=True, exist_ok=True)
    iconset = out / ICONSET.relative_to(ROOT)
    iconset.mkdir(parents=True, exist_ok=True)
    master = out / "design/icon/asiri-icon.svg"
    master.write_text(app_icon_svg())
    for pt, scale in SIZES:
        suffix = "" if scale == 1 else "@2x"
        render(master, iconset / f"icon_{pt}x{pt}{suffix}.png", pt * scale)
    (iconset / "Contents.json").write_text(appiconset_json())
    shutil.copy(iconset / "icon_512x512@2x.png", out / "design/icon/asiri-icon-1024.png")
    assets = out / ASSETS.relative_to(ROOT)
    for name, unread in (("MenuBarIcon", False), ("MenuBarIconUnread", True)):
        folder = assets / f"{name}.imageset"
        folder.mkdir(parents=True, exist_ok=True)
        for file, text in imageset(name, glyph_svg(unread)).items():
            (folder / file).write_text(text)


def main() -> int:
    if shutil.which("rsvg-convert") is None:
        print("make-icon.py needs rsvg-convert (brew install librsvg / apt install librsvg2-bin)", file=sys.stderr)
        return 1
    if "--check" in sys.argv:
        with tempfile.TemporaryDirectory() as tmp:
            build(pathlib.Path(tmp))
            # PNG bytes can differ between librsvg versions, so compare the SVG masters only.
            fresh = (pathlib.Path(tmp) / "design/icon/asiri-icon.svg").read_text()
            if fresh != (MASTER / "asiri-icon.svg").read_text():
                print("design/icon/asiri-icon.svg is out of date: run tools/design/make-icon.py", file=sys.stderr)
                return 1
        print("icon sources are up to date")
        return 0
    build(ROOT)
    print(f"wrote {MASTER.relative_to(ROOT)} and {ICONSET.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
