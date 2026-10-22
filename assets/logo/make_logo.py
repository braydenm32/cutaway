"""Draw Cutaway's logo and the app icons made from it. Run from anywhere:

    python3 assets/logo/make_logo.py

Needs Pillow. The logo: a squared, forward-leaning C and A whose left leg is
a red cut, inside a camera's corner brackets with a red recording light.
This file is the one place it's drawn; everything below is made from it:

    assets/logo/cutaway-mac.svg       the Mac icon (rounded tile, margin, shadow)
    assets/logo/cutaway-square.svg    the Windows icon (tile filling the square)
    assets/logo/cutaway-mark.svg      the letters alone, for dark backgrounds
    assets/logo/cutaway-mark-light.svg  the letters alone, for light backgrounds
    assets/logo/window-mac.png        what the running app shows in the Dock
    assets/logo/window-square.png     what it shows on the Windows taskbar
    assets/logo/mark.png              the letters in the app's top bar and About window
    assets/logo/remote.png            the phone remote's home-screen icon
    assets/logo/social-preview.png    the picture GitHub shows when the repo is linked
                                      (needs the Inter font; skipped without it)
    macos/Cutaway.icns                Cutaway.app's icon
    windows/cutaway.ico               Cutaway.exe's icon

The Stream Deck plugin's icons use it too (streamdeck/make_icons.py).
"""

import math
import os

from PIL import Image, ImageDraw, ImageFilter, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.join(HERE, "..", "..")

TILE = (23, 25, 31, 255)  # #17191F
WHITE = (255, 255, 255, 255)
RED = (232, 52, 64, 255)  # the app's record red, #E83440

# The letters, on a 100-unit square, drawn upright and then leaned forward 12°.
SLANT = math.tan(math.radians(12))
LEAN_SHIFT = 10.6  # keeps the leaned letters centred


def _c_outline():
    """The C: a squared bowl with small outer corners and sharp inner ones."""
    pts = [(42, 34), (25.5, 34)]
    for a in range(-90, -181, -10):  # top-left corner
        pts.append((25.5 + 6.5 * math.cos(math.radians(a)), 40.5 + 6.5 * math.sin(math.radians(a))))
    for a in range(180, 89, -10):  # bottom-left corner
        pts.append((25.5 + 6.5 * math.cos(math.radians(a)), 59.5 + 6.5 * math.sin(math.radians(a))))
    pts += [(42, 66), (42, 59), (26, 59), (26, 41), (42, 41)]
    return pts


def mark(letters=WHITE, cut=RED):
    """[(points, colour)], back to front, in 100-unit letter space."""
    shapes = [
        (_c_outline(), letters),
        ([(60.05, 31), (67.45, 31), (81, 66), (73.6, 66)], letters),  # the A's right leg
        ([(60, 55), (69.4, 55), (71.3, 60), (60, 60)], letters),  # its crossbar
        ([(46.4, 78), (55.7, 78), (69.7, 22), (60.4, 22)], cut),  # the cut: the A's left leg
    ]
    return [([(x - SLANT * y + LEAN_SHIFT, y) for x, y in pts], colour) for pts, colour in shapes]


# Where things sit on each icon, in 100-unit icon space. The Mac icon follows
# Apple's grid (the tile is 80% of the canvas, with room for a shadow); the
# Windows one fills the square, as Windows doesn't add a tile of its own.
STYLES = {
    "mac": dict(tile=(10, 10, 90, 90), radius=18, shadow=True, edge=0.08, edge_w=0.6,
                brackets=(18, 82, 11), bracket_w=3.5, dot=(74.5, 25.5, 3.2), letters=(22, 56)),
    "square": dict(tile=(3, 3, 97, 97), radius=17, shadow=False, edge=0.12, edge_w=1.2,
                   brackets=(12, 88, 14), bracket_w=4.5, dot=(78.5, 21.5, 4.2), letters=(13, 74)),
}


def _brackets(lo, hi, arm):
    return [
        [(lo, lo + arm), (lo, lo), (lo + arm, lo)],
        [(hi - arm, lo), (hi, lo), (hi, lo + arm)],
        [(hi, hi - arm), (hi, hi), (hi - arm, hi)],
        [(lo + arm, hi), (lo, hi), (lo, hi - arm)],
    ]


def _place(pts, at, size):
    return [(at + x * size / 100, at + y * size / 100) for x, y in pts]


# ---- pictures ----

def render(style, size, ss=8):
    """The icon as a size×size RGBA picture."""
    s = STYLES[style]
    n = size * ss
    u = n / 100  # pixels per unit
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    x0, y0, x1, y1 = (v * u for v in s["tile"])
    if s["shadow"]:
        shadow = Image.new("RGBA", (n, n), (0, 0, 0, 0))
        ImageDraw.Draw(shadow).rounded_rectangle([x0, y0 + 2 * u, x1, y1 + 2 * u], radius=s["radius"] * u, fill=(0, 0, 0, 90))
        img = Image.alpha_composite(img, shadow.filter(ImageFilter.GaussianBlur(1.5 * u)))
    d = ImageDraw.Draw(img)
    edge = tuple(round(c + (255 - c) * s["edge"]) for c in TILE[:3]) + (255,)  # a faint light rim
    d.rounded_rectangle([x0, y0, x1, y1], radius=s["radius"] * u, fill=edge)
    inset = s["edge_w"] * u
    d.rounded_rectangle([x0 + inset, y0 + inset, x1 - inset, y1 - inset], radius=s["radius"] * u - inset, fill=TILE)
    w = s["bracket_w"] * u
    for line in _brackets(*s["brackets"]):
        pts = [(x * u, y * u) for x, y in line]
        d.line(pts, fill=WHITE, width=round(w), joint="curve")
        for x, y in (pts[0], pts[-1]):
            d.ellipse([x - w / 2, y - w / 2, x + w / 2, y + w / 2], fill=WHITE)
    cx, cy, r = s["dot"]
    d.ellipse([(cx - r) * u, (cy - r) * u, (cx + r) * u, (cy + r) * u], fill=RED)
    at, span = s["letters"]
    for pts, colour in mark():
        d.polygon([(x * u, y * u) for x, y in _place(pts, at, span)], fill=colour)
    return img.resize((size, size), Image.LANCZOS)


def render_mark(size, letters=WHITE, cut=RED, ss=8):
    """The letters alone on transparent, filling the square."""
    n = size * ss
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    for pts, colour in mark(letters, cut):
        d.polygon([(x * n / 100, y * n / 100) for x, y in pts], fill=colour)
    return img.resize((size, size), Image.LANCZOS)


# ---- vectors ----

def _hex(c):
    return "#%02X%02X%02X" % c[:3]


def _poly(pts, colour):
    return '<polygon points="%s" fill="%s"/>' % (" ".join("%.2f,%.2f" % p for p in pts), _hex(colour))


def svg(style):
    s = STYLES[style]
    x0, y0, x1, y1 = s["tile"]
    out = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="1024" height="1024">', "<title>Cutaway</title>"]
    if s["shadow"]:
        out.append('<filter id="shadow" x="-20%" y="-20%" width="140%" height="140%"><feGaussianBlur stdDeviation="1.5"/></filter>')
        out.append('<rect x="%g" y="%g" width="%g" height="%g" rx="%g" fill="#000" fill-opacity="0.35" filter="url(#shadow)"/>' % (x0, y0 + 2, x1 - x0, y1 - y0, s["radius"]))
    h = s["edge_w"] / 2
    out.append('<rect x="%g" y="%g" width="%g" height="%g" rx="%g" fill="%s" stroke="#FFF" stroke-opacity="%g" stroke-width="%g"/>'
               % (x0 + h, y0 + h, x1 - x0 - 2 * h, y1 - y0 - 2 * h, s["radius"] - h, _hex(TILE), s["edge"], s["edge_w"]))
    d = " ".join("M%g %g L%g %g L%g %g" % (a + b + c) for a, b, c in _brackets(*s["brackets"]))
    out.append('<path d="%s" fill="none" stroke="#FFF" stroke-width="%g" stroke-linecap="round" stroke-linejoin="round"/>' % (d, s["bracket_w"]))
    out.append('<circle cx="%g" cy="%g" r="%g" fill="%s"/>' % (*s["dot"], _hex(RED)))
    at, span = s["letters"]
    out += [_poly(_place(pts, at, span), colour) for pts, colour in mark()]
    out.append("</svg>")
    return "\n".join(out) + "\n"


def svg_mark(letters=WHITE):
    out = ['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100" width="512" height="512">', "<title>Cutaway</title>"]
    out += [_poly(pts, colour) for pts, colour in mark(letters)]
    out.append("</svg>")
    return "\n".join(out) + "\n"


def _font(names, px):
    for name in names:
        try:
            return ImageFont.truetype(name, px)
        except OSError:
            pass
    return None


def social_preview():
    """1280×640, for GitHub's Settings → Social preview. None without Inter."""
    bold = _font(["Inter-Bold.otf", "/usr/share/fonts/opentype/inter/Inter-Bold.otf", "Inter Bold.ttf", "Inter-Bold.ttf"], 128)
    regular = _font(["Inter-Regular.otf", "/usr/share/fonts/opentype/inter/Inter-Regular.otf", "Inter.ttc", "Inter-Regular.ttf"], 34)
    if not bold or not regular:
        return None
    img = Image.new("RGBA", (1280, 640), (14, 16, 20, 255))  # the app's background
    img.alpha_composite(render("mac", 400), (80, 120))
    d = ImageDraw.Draw(img)
    d.text((520, 330), "Cutaway", font=bold, fill=WHITE, anchor="ls")
    d.text((526, 392), "Record, switch and stream your show.", font=regular, fill=(200, 204, 214, 255), anchor="ls")
    d.text((526, 442), "Mac and Windows", font=regular, fill=(140, 147, 160, 255), anchor="ls")
    return img.convert("RGB")


def write(path, text):
    with open(path, "w", newline="\n") as f:
        f.write(text)


if __name__ == "__main__":
    write(os.path.join(HERE, "cutaway-mac.svg"), svg("mac"))
    write(os.path.join(HERE, "cutaway-square.svg"), svg("square"))
    write(os.path.join(HERE, "cutaway-mark.svg"), svg_mark())
    write(os.path.join(HERE, "cutaway-mark-light.svg"), svg_mark(TILE))
    render("mac", 512).save(os.path.join(HERE, "window-mac.png"), optimize=True)
    render("square", 256).save(os.path.join(HERE, "window-square.png"), optimize=True)
    letters = render_mark(256)
    letters.crop(letters.getbbox()).save(os.path.join(HERE, "mark.png"), optimize=True)
    # A phone puts its own mask on a home-screen icon, so the tile fills it.
    full = Image.new("RGBA", (180, 180), TILE)
    full.alpha_composite(render("square", 196).crop((8, 8, 188, 188)))
    full.convert("RGB").save(os.path.join(HERE, "remote.png"), optimize=True)
    preview = social_preview()
    if preview:
        preview.save(os.path.join(HERE, "social-preview.png"), optimize=True)
    else:
        print("social-preview.png skipped: install the Inter font to redraw it")

    mac = {n: render("mac", n) for n in (16, 32, 64, 128, 256, 512, 1024)}
    mac[1024].save(os.path.join(ROOT, "macos", "Cutaway.icns"), append_images=[m for n, m in mac.items() if n != 1024])

    win = {n: render("square", n) for n in (16, 20, 24, 32, 40, 48, 64, 128, 256)}
    win[256].save(os.path.join(ROOT, "windows", "cutaway.ico"), sizes=[(n, n) for n in win], append_images=[m for n, m in win.items() if n != 256])
    print("logo and icons written")
