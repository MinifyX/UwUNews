#!/usr/bin/env python3
"""Draws Nyu as a newspaper and the app icon.

Writes the three empty-state stickers as vector SVGs into the asset catalog
and renders the 1024 px icon. Needs cairosvg (and libcairo) for the icon:

    python3 -m venv .venv && .venv/bin/pip install cairosvg
    DYLD_FALLBACK_LIBRARY_PATH=$(brew --prefix)/lib .venv/bin/python scripts/art.py

Shapes follow the UwUSuite terminal cat on its 256 grid; the white die-cut
edge is spelled out as its own pass because asset catalogs ignore CSS.
"""

from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ASSETS = ROOT / "ios/UwUNews/Resources/Assets.xcassets"

INK = "#4B1D3F"
BODY = "#FF6FA6"
FLAP = "#FFB8D3"
BLUSH = "#FF4D8D"
PAPER = "#FFFFFF"
STAR = "#FFD66E"
TEAR = "#9ED8FF"
SKY = "#BDE6FF"
EDGE = 20


class El:
    """One shape; `edge=False` keeps it out of the white die-cut pass."""

    def __init__(self, tag, edge=True, **attrs):
        self.tag, self.edge, self.attrs = tag, edge, attrs

    def svg(self, cut=False):
        attrs = dict(self.attrs)
        if cut:
            stroke = float(attrs.get("stroke_width", 0) if attrs.get("stroke") else 0)
            attrs["stroke_width"] = stroke + EDGE
            attrs["stroke"] = PAPER
            if attrs.get("fill", "black") != "none":
                attrs["fill"] = PAPER
            attrs.pop("opacity", None)
        body = " ".join(f'{k.replace("_", "-")}="{v}"' for k, v in attrs.items())
        return f"<{self.tag} {body}/>"


def path(d, edge=True, **a):
    return El("path", edge, d=d, **a)


def line(d, color=INK, width=8, edge=True):
    return path(d, edge, fill="none", stroke=color, stroke_width=width)


def ears():
    return [
        path("M62 88 L80 36 L114 88 Z", fill=BODY, stroke=INK, stroke_width=9),
        path("M76 82 L84 52 L100 82 Z", fill=FLAP),
        path("M142 88 L176 36 L194 88 Z", fill=BODY, stroke=INK, stroke_width=9),
        path("M156 82 L172 52 L180 82 Z", fill=FLAP),
    ]


def face(mood):
    parts = [
        El("ellipse", False, cx=74, cy=168, rx=12, ry=7.5, fill=BLUSH, opacity=0.5),
        El("ellipse", False, cx=182, cy=168, rx=12, ry=7.5, fill=BLUSH, opacity=0.5),
    ]
    if mood == "happy":
        parts += [
            El("ellipse", cx=102, cy=148, rx=8, ry=10, fill=INK),
            El("ellipse", cx=154, cy=148, rx=8, ry=10, fill=INK),
            El("circle", False, cx=105, cy=144, r=3, fill=PAPER),
            El("circle", False, cx=157, cy=144, r=3, fill=PAPER),
        ]
    elif mood == "sleepy":
        parts += [line("M90 150 q12 8 24 0"), line("M142 150 q12 8 24 0")]
    else:
        parts += [line("M90 152 Q102 142 114 152"), line("M142 152 Q154 142 166 152")]
        for x in (94, 162):
            parts.append(path(f"M{x} 158 q-6 9 0 13 q6 -4 0 -13Z", False, fill=TEAR, stroke=INK, stroke_width=4))
    if mood == "sad":
        parts.append(line("M114 184 Q128 172 142 184"))
    elif mood == "sleepy":
        parts.append(line("M120 180 q4 5 8 0 q4 5 8 0", width=6))
    else:
        parts.append(line("M112 170 q8 13 16 0 q8 13 16 0"))
    return parts


def newspaper(mood):
    """Nyu as a newspaper: a second page peeks out behind her, the masthead
    runs across her forehead and the front page is her face."""
    shapes = [
        # The page underneath, shifted down and right.
        El("rect", x=40, y=88, width=200, height=146, rx=24, fill=FLAP, stroke=INK, stroke_width=9),
        *ears(),
        El("rect", x=28, y=74, width=200, height=146, rx=24, fill=BODY, stroke=INK, stroke_width=9),
        # Masthead: the paper's name and the date line.
        El("rect", False, x=48, y=88, width=92, height=12, rx=6, fill=INK),
        El("rect", False, x=156, y=88, width=52, height=6, rx=3, fill=FLAP),
        El("rect", False, x=172, y=98, width=36, height=6, rx=3, fill=FLAP),
        # Front page.
        El("rect", x=44, y=114, width=168, height=92, rx=16, fill=PAPER, stroke=INK, stroke_width=7),
        line("M58 194 H92", FLAP, 6, False),
        line("M164 194 H198", FLAP, 6, False),
        *face(mood),
    ]
    if mood == "sleepy":
        shapes += [
            path("M206 50 h14 l-14 16 h14", False, fill="none", stroke=INK, stroke_width=5),
            path("M226 26 h10 l-10 11 h10", False, fill="none", stroke=INK, stroke_width=4),
        ]
    return shapes


def sticker(shapes):
    cut = "".join(s.svg(cut=True) for s in shapes if s.edge)
    drawn = "".join(s.svg() for s in shapes)
    return f'<g stroke-linecap="round" stroke-linejoin="round">{cut}{drawn}</g>'


def sparkle(cx, cy, r, color=STAR):
    q = r * 0.18
    d = (
        f"M{cx} {cy - r} Q{cx + q} {cy - q} {cx + r} {cy} Q{cx + q} {cy + q} {cx} {cy + r} "
        f"Q{cx - q} {cy + q} {cx - r} {cy} Q{cx - q} {cy - q} {cx} {cy - r}Z"
    )
    return f'<path d="{d}" fill="{color}" stroke="{INK}" stroke-width="7" stroke-linejoin="round"/>'


def write_sticker(mood):
    svg = (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="6 14 254 244" width="254" height="244">'
        f"{sticker(newspaper(mood))}</svg>"
    )
    folder = ASSETS / f"Nyu-{mood}.imageset"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / f"Nyu-{mood}.svg").write_text(svg)
    (folder / "Contents.json").write_text(
        '{\n  "images" : [\n    {\n      "filename" : "Nyu-%s.svg",\n      "idiom" : "universal"\n    }\n  ],\n'
        '  "info" : {\n    "author" : "xcode",\n    "version" : 1\n  },\n'
        '  "properties" : {\n    "preserves-vector-representation" : true\n  }\n}\n' % mood
    )


def icon_svg():
    # A full-bleed square: iOS draws its own rounded corners.
    nyu = sticker(newspaper("happy"))
    return (
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 512 512" width="1024" height="1024">'
        '<defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1">'
        '<stop offset="0" stop-color="#FFF3F8"/><stop offset="1" stop-color="#FFD3E5"/></linearGradient>'
        '<filter id="s" x="-20%" y="-20%" width="140%" height="140%">'
        '<feDropShadow dx="0" dy="10" stdDeviation="10" flood-color="#C2306F" flood-opacity="0.25"/></filter></defs>'
        '<rect width="512" height="512" fill="url(#g)"/>'
        f'<circle cx="402" cy="128" r="54" fill="{SKY}" opacity="0.9"/>'
        f'<g filter="url(#s)"><g transform="translate(256 278) rotate(-7) scale(1.32) translate(-133 -136)">{nyu}</g></g>'
        f"{sparkle(408, 116, 34)}{sparkle(98, 104, 20)}{sparkle(440, 396, 16, PAPER)}"
        "</svg>"
    )


def write_icon():
    import cairosvg

    folder = ASSETS / "AppIcon.appiconset"
    folder.mkdir(parents=True, exist_ok=True)
    svg = icon_svg()
    (ROOT / "docs/icon.svg").write_text(svg)
    cairosvg.svg2png(bytestring=svg.encode(), write_to=str(folder / "AppIcon.png"), output_width=1024, output_height=1024)
    (folder / "Contents.json").write_text(
        '{\n  "images" : [\n    {\n      "filename" : "AppIcon.png",\n      "idiom" : "universal",\n'
        '      "platform" : "ios",\n      "size" : "1024x1024"\n    }\n  ],\n'
        '  "info" : {\n    "author" : "xcode",\n    "version" : 1\n  }\n}\n'
    )


if __name__ == "__main__":
    for mood in ("happy", "sleepy", "sad"):
        write_sticker(mood)
    write_icon()
