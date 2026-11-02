# Bundled symbol fonts

Cut-down copies of three Noto fonts, holding only symbols (arrows, ticks and
crosses, keyboard keys, shapes, media keys and a few pictures). The app puts
them right after its letters font, so every symbol it shows draws the same on
a Mac and on Windows instead of depending on the computer's fonts (macOS has no
everyday font with ＋, so it showed an empty box). See `src/ui/fonts.rs`.

- `NotoSansSymbols2-Subset.ttf`, `NotoSansMath-Subset.ttf`, `NotoEmoji-Subset.ttf`
- Licence: SIL Open Font License 1.1, in `OFL.txt`.
- Rebuild or add a symbol: `make_subsets.py` (instructions at its top).

The test `every_symbol_the_app_writes_has_a_bundled_font` fails if text in the
app uses a character none of the bundled fonts has.
