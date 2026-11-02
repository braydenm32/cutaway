#!/usr/bin/env python3
"""Rebuild the cut-down symbol fonts the app bundles (see src/ui/fonts.rs).

Needs fontTools (pip install fonttools) and the three full Noto fonts in the
folder given as the first argument:

  NotoSansSymbols2-Regular.ttf  github.com/notofonts/notofonts.github.io  fonts/NotoSansSymbols2/hinted/ttf/
  NotoSansMath-Regular.ttf      github.com/notofonts/notofonts.github.io  fonts/NotoSansMath/hinted/ttf/
  NotoEmoji[wght].ttf           github.com/google/fonts                    ofl/notoemoji/

To add a symbol, put it in EXTRA below (or use one from the blocks already
included) and run:  python3 assets/fonts/make_subsets.py ~/Downloads/noto
"""
import os
import sys

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.varLib import instancer

HERE = os.path.dirname(os.path.abspath(__file__))

# Whole Unicode blocks, so most symbols someone reaches for already work.
BLOCKS = [
    (0x2190, 0x21FF),  # Arrows
    (0x2300, 0x23FF),  # Miscellaneous Technical (⌘ ⌥ ⌂ ⏯ …)
    (0x25A0, 0x25FF),  # Geometric Shapes
    (0x2600, 0x26FF),  # Miscellaneous Symbols
    (0x2700, 0x27BF),  # Dingbats (✓ ✕ ✎ …)
    (0x27F0, 0x27FF),  # Supplemental Arrows-A (⟲ ⟳)
    (0x2800, 0x28FF),  # Braille (⠿ drag handle)
    (0x2B00, 0x2BFF),  # Miscellaneous Symbols and Arrows
]
# Single characters the app uses outside those blocks.
EXTRA = "⋯⇧⧉🎧👋🖨"
# Full-width plus and minus, drawn with the font's own + and −.
ALIASES = {0xFF0B: 0x002B, 0xFF0D: 0x2212}


def cut(src, dst, wanted):
    font = TTFont(src)
    if "fvar" in font:
        font = instancer.instantiateVariableFont(font, {"wght": 400})
    have = font.getBestCmap()
    keep = sorted(u for u in wanted if u in have)
    opts = subset.Options()
    opts.hinting = False
    opts.desubroutinize = True
    opts.layout_features = []
    opts.name_IDs = ["*"]
    opts.notdef_outline = True
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=keep)
    sub.subset(font)
    font.save(dst)
    return set(keep)


def main():
    src = sys.argv[1]
    wanted = {u for a, b in BLOCKS for u in range(a, b + 1)} | {ord(c) for c in EXTRA}
    got = cut(os.path.join(src, "NotoSansSymbols2-Regular.ttf"), os.path.join(HERE, "NotoSansSymbols2-Subset.ttf"), wanted)
    rest = wanted - got
    math = os.path.join(HERE, "NotoSansMath-Subset.ttf")
    rest -= cut(os.path.join(src, "NotoSansMath-Regular.ttf"), math, rest | set(ALIASES.values()))
    font = TTFont(math)
    for table in font["cmap"].tables:
        if table.isUnicode():
            for alias, base in ALIASES.items():
                table.cmap[alias] = table.cmap[base]
    font.save(math)
    rest -= cut(os.path.join(src, "NotoEmoji[wght].ttf"), os.path.join(HERE, "NotoEmoji-Subset.ttf"), rest)
    picked = {ord(c) for c in EXTRA} & rest
    if picked:
        sys.exit("No font has: " + "".join(chr(u) for u in sorted(picked)))


if __name__ == "__main__":
    main()
