"""Draw Cutaway's Windows icon (windows/cutaway.ico). Run from this folder:

    python3 make_icon.py

Same picture as the Stream Deck plugin's icon. Needs Pillow.
"""

import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "streamdeck"))
from make_icons import BG, RED, S, WHITE, glyph  # noqa: E402
from PIL import Image, ImageDraw  # noqa: E402

n = 256 * S
img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
d = ImageDraw.Draw(img)
d.rounded_rectangle([0, 0, n - 1, n - 1], radius=n * 0.22, fill=BG)
glyph(d, "logo", (n * 0.16, n * 0.16, n * 0.84, n * 0.84), WHITE, RED)
img = img.resize((256, 256), Image.LANCZOS)
img.save(os.path.join(HERE, "cutaway.ico"), sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)])
print("wrote cutaway.ico")
