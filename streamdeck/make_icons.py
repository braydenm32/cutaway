"""Draw the Stream Deck plugin's icons (action list icons, default key
pictures, category and plugin icons). Run from this folder:

    python3 make_icons.py

Needs Pillow. The plugin draws live key pictures itself; these are what
the Stream Deck app shows in its action list and before Cutaway connects.
"""

import math
import os
import sys

from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "assets", "logo"))
import make_logo  # noqa: E402  Cutaway's logo, for the plugin and category icons
OUT = os.path.join(HERE, "app.cutaway.streamdeck.sdPlugin", "imgs")
S = 8  # supersampling

WHITE = (255, 255, 255, 255)
RED = (229, 48, 62, 255)
BG = (21, 23, 28, 255)
DIM = (138, 144, 160, 255)


def font(px):
    for name in ["DejaVuSans-Bold.ttf", "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf", "Arial Bold.ttf", "Helvetica.ttc"]:
        try:
            return ImageFont.truetype(name, px)
        except OSError:
            pass
    return ImageFont.load_default()


def glyph(d, kind, box, color, accent=None):
    """Draw `kind` into box (x0, y0, x1, y1) on a supersampled canvas."""
    x0, y0, x1, y1 = box
    w, h = x1 - x0, y1 - y0
    u = w / 20.0  # one unit of a 20-unit grid
    lw = max(1, int(1.8 * u))
    p = lambda x, y: (x0 + x * u, y0 + y * u)
    accent = accent or color
    if kind == "scene":
        d.rounded_rectangle([p(2, 4), p(14, 13)], radius=1.5 * u, outline=color, width=lw)
        d.rounded_rectangle([p(6, 8), p(18, 17)], radius=1.5 * u, fill=color)
    elif kind == "source":
        d.rounded_rectangle([p(2, 6), p(13, 15)], radius=1.5 * u, fill=color)
        d.polygon([p(14, 9), p(18, 6.5), p(18, 14.5), p(14, 12)], fill=color)
    elif kind == "take":
        d.line([p(3, 10), p(15, 10)], fill=color, width=lw * 2)
        d.polygon([p(12, 5), p(18, 10), p(12, 15)], fill=color)
    elif kind == "transition":
        d.rounded_rectangle([p(2, 5), p(11, 15)], radius=1 * u, outline=color, width=lw)
        d.rounded_rectangle([p(9, 5), p(18, 15)], radius=1 * u, fill=color)
    elif kind == "record":
        d.ellipse([p(4, 4), p(16, 16)], fill=accent)
    elif kind == "live":
        d.ellipse([p(8, 8), p(12, 12)], fill=accent)
        for r in (5, 8):
            for a0 in (-45, 135):
                d.arc([p(10 - r, 10 - r), p(10 + r, 10 + r)], a0, a0 + 90, fill=color, width=lw)
    elif kind == "marker":
        d.polygon([p(10, 2.5), p(17.5, 10), p(10, 17.5), p(2.5, 10)], fill=color)
    elif kind == "graphic":
        d.rounded_rectangle([p(2, 4), p(18, 16)], radius=1.5 * u, outline=color, width=lw)
        d.rounded_rectangle([p(4, 11), p(13, 14)], radius=0.6 * u, fill=color)
    elif kind == "audio":
        for i, hgt in enumerate([5, 9, 13, 8, 11]):
            x = 3 + i * 3.2
            d.rounded_rectangle([p(x, 17 - hgt), p(x + 2, 17)], radius=0.8 * u, fill=color)
    elif kind == "auto":
        d.arc([p(3, 3), p(17, 17)], 200, 340, fill=color, width=lw)
        d.arc([p(3, 3), p(17, 17)], 20, 160, fill=color, width=lw)
        d.polygon([p(15.5, 2.5), p(18.5, 7.5), p(13, 7.5)], fill=color)
        d.polygon([p(4.5, 17.5), p(1.5, 12.5), p(7, 12.5)], fill=color)
        d.ellipse([p(8, 8), p(12, 12)], fill=color)
    elif kind == "captions":
        d.rounded_rectangle([p(2, 4), p(18, 16)], radius=2 * u, outline=color, width=lw)
        f = font(int(7.5 * u))
        d.text(p(10, 10.3), "CC", font=f, fill=color, anchor="mm")
    elif kind == "media":
        d.polygon([p(5, 3.5), p(17, 10), p(5, 16.5)], fill=color)
    elif kind == "command":
        d.rounded_rectangle([p(2, 3), p(18, 17)], radius=1.5 * u, outline=color, width=lw)
        d.line([p(5, 7), p(8.5, 10), p(5, 13)], fill=color, width=lw, joint="curve")
        d.line([p(10, 13), p(15, 13)], fill=color, width=lw)
    elif kind == "pad":
        d.rounded_rectangle([p(2, 2), p(9, 9)], radius=1.2 * u, fill=color)
        d.rounded_rectangle([p(11, 2), p(18, 9)], radius=1.2 * u, outline=color, width=lw)
        d.rounded_rectangle([p(2, 11), p(9, 18)], radius=1.2 * u, outline=color, width=lw)
        d.rounded_rectangle([p(11, 11), p(18, 18)], radius=1.2 * u, fill=color)
    elif kind == "syrinscape":
        # A dragon-ish wave: three rising sound arcs from a point.
        d.ellipse([p(3, 12), p(7, 16)], fill=color)
        for r in (5, 9, 13):
            d.arc([p(5 - r, 14 - r), p(5 + r, 14 + r)], 290, 360, fill=color, width=lw)
    elif kind == "talkback":
        d.rounded_rectangle([p(7, 2), p(13, 12)], radius=3 * u, fill=color)
        d.arc([p(4, 5), p(16, 15)], 0, 180, fill=color, width=lw)
        d.line([p(10, 15), p(10, 18)], fill=color, width=lw)
    elif kind == "clip":
        d.ellipse([p(3, 12), p(8, 17)], outline=color, width=lw)
        d.ellipse([p(12, 12), p(17, 17)], outline=color, width=lw)
        d.line([p(7, 12.5), p(15, 3)], fill=color, width=lw)
        d.line([p(13, 12.5), p(5, 3)], fill=color, width=lw)
    elif kind == "timer":
        d.ellipse([p(3, 4), p(17, 18)], outline=color, width=lw)
        d.line([p(10, 11), p(10, 6.5)], fill=color, width=lw)
        d.line([p(10, 11), p(13, 13)], fill=color, width=lw)
        d.line([p(8, 2), p(12, 2)], fill=color, width=lw)
    elif kind == "rundown":
        for i, y in enumerate((4, 9, 14)):
            d.rounded_rectangle([p(2, y), p(4, y + 2)], radius=0.5 * u, fill=accent if i == 1 else color)
            d.rounded_rectangle([p(6, y), p(18, y + 2)], radius=0.5 * u, fill=accent if i == 1 else color)
    elif kind == "guest":
        d.ellipse([p(6.5, 2.5), p(13.5, 9.5)], fill=color)
        d.rounded_rectangle([p(4, 11), p(16, 18)], radius=3 * u, fill=color)
        d.ellipse([p(14.5, 1.5), p(18.5, 5.5)], fill=accent)
    elif kind == "ftb":
        d.rounded_rectangle([p(2, 4), p(18, 16)], radius=1.5 * u, outline=color, width=lw)
        d.polygon([p(4, 14), p(16, 6), p(16, 14)], fill=color)
    elif kind == "replay":
        d.arc([p(3, 3), p(17, 17)], 60, 330, fill=color, width=lw)
        d.polygon([p(15.5, 1.5), p(18.5, 7), p(12.5, 7.5)], fill=color)
        d.polygon([p(8.5, 7), p(13.5, 10), p(8.5, 13)], fill=accent)
    elif kind == "macro":
        for i, y in enumerate((3.5, 8.5, 13.5)):
            d.rounded_rectangle([p(2 + i * 1.5, y), p(12 + i * 1.5, y + 3)], radius=0.8 * u, fill=color)
        d.polygon([p(14, 7), p(18.5, 10), p(14, 13)], fill=accent)
    elif kind == "ptz":
        d.rounded_rectangle([p(5, 3), p(15, 11)], radius=2 * u, fill=color)
        d.ellipse([p(8, 5), p(12, 9)], fill=BG)
        d.rounded_rectangle([p(8.5, 11), p(11.5, 15)], radius=0.6 * u, fill=color)
        d.rounded_rectangle([p(4, 15), p(16, 17.5)], radius=1 * u, fill=color)
    elif kind == "prompter":
        d.rounded_rectangle([p(3, 2), p(17, 18)], radius=1.5 * u, outline=color, width=lw)
        for y, w in ((6, 10), (9.5, 8), (13, 9)):
            d.rounded_rectangle([p(5.5, y), p(5.5 + w, y + 1.6)], radius=0.6 * u, fill=color)
    elif kind == "comment":
        d.rounded_rectangle([p(2, 3), p(18, 13.5)], radius=2.5 * u, fill=color)
        d.polygon([p(5, 13), p(10, 13), p(5, 17.5)], fill=color)
        for x in (6.5, 10, 13.5):
            d.ellipse([p(x - 1, 7.3), p(x + 1, 9.3)], fill=BG)


def save(img, path, size):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    img.resize((size, size), Image.LANCZOS).save(path)


def icon(kind, path, size, color=WHITE, accent=None):
    """Monochrome glyph on transparent (action list, category)."""
    big = Image.new("RGBA", (size * S, size * S), (0, 0, 0, 0))
    glyph(ImageDraw.Draw(big), kind, (0, 0, size * S, size * S), color, accent)
    save(big, path + ".png", size)
    big2 = Image.new("RGBA", (size * 2 * S, size * 2 * S), (0, 0, 0, 0))
    glyph(ImageDraw.Draw(big2), kind, (0, 0, size * 2 * S, size * 2 * S), color, accent)
    save(big2, path + "@2x.png", size * 2)


def key(kind, path, name):
    """Default key picture: glyph and name on the dark key."""
    for size, suffix in ((72, ""), (144, "@2x")):
        n = size * S
        img = Image.new("RGBA", (n, n), BG)
        d = ImageDraw.Draw(img)
        glyph(d, kind, (n * 0.27, n * 0.12, n * 0.73, n * 0.58), WHITE, RED)
        d.text((n / 2, n * 0.8), name, font=font(int(n * 0.14)), fill=DIM, anchor="mm")
        save(img, path + suffix + ".png", size)


def plugin_icon(path):
    """The app icon, for the Stream Deck store and the plugin list."""
    for size, suffix in ((256, ""), (512, "@2x")):
        make_logo.render("square", size).save(path + suffix + ".png")


def category_icon(path):
    """The letters alone in white, beside the plugin's group of actions."""
    for size, suffix in ((28, ""), (56, "@2x")):
        make_logo.render_mark(size, WHITE, WHITE).save(path + suffix + ".png")


ACTIONS = {
    "scene": "Scene",
    "source": "Source",
    "take": "Take",
    "transition": "Transition",
    "record": "Record",
    "live": "Go live",
    "marker": "Marker",
    "graphic": "Graphic",
    "audio": "Audio",
    "auto": "Auto",
    "captions": "Captions",
    "media": "Media",
    "pad": "Pad",
    "syrinscape": "Syrinscape",
    "talkback": "Talk",
    "clip": "Clip",
    "timer": "Timer",
    "rundown": "Rundown",
    "guest": "Guest",
    "ftb": "Fade",
    "replay": "Replay",
    "macro": "Macro",
    "ptz": "PTZ",
    "prompter": "Prompter",
    "comment": "Comment",
    "command": "Command",
}

if __name__ == "__main__":
    for kind, name in ACTIONS.items():
        icon(kind, os.path.join(OUT, "actions", kind, "icon"), 20, WHITE, WHITE)
        key(kind, os.path.join(OUT, "actions", kind, "key"), name)
    category_icon(os.path.join(OUT, "plugin", "category"))
    plugin_icon(os.path.join(OUT, "plugin", "marketplace"))
    print("icons written to", OUT)
