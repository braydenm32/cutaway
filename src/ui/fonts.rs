//! The fonts the app draws its own words and symbols with.
//!
//! egui's built-in font (Ubuntu Light) has letters but almost no symbols, so
//! for ＋, ✓, ✕, ⌥, ↶ and friends egui asks the computer for a font that has
//! them. That works for some symbols and not others, and differently on a Mac
//! and on Windows: macOS has no everyday font with ＋ (U+FF0B) in it, so the
//! "＋ Make a name title" buttons showed an empty box instead.
//!
//! So every symbol the app uses ships with it, in three small cut-down Noto
//! fonts (SIL Open Font License, see assets/fonts/OFL.txt), placed right after
//! Ubuntu Light. The computer's fonts are only asked for what is left (names
//! people type in other scripts, an emoji from a viewer's comment), and the
//! test below fails if a symbol in the app's own text has no bundled font.

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

/// Arrows, keyboard keys, shapes, ticks and crosses, braille dots, media keys.
const SYMBOLS: &[u8] = include_bytes!("../../assets/fonts/NotoSansSymbols2-Subset.ttf");
/// The arrows, dots and shapes Symbols 2 lacks (↑ ↶ ⋯ ⌂ ⧉ ⟲), and ＋ and －
/// (the plus and minus drawn at symbol weight).
const MATH: &[u8] = include_bytes!("../../assets/fonts/NotoSansMath-Subset.ttf");
/// The few pictures the app uses (🎧 🖨 👋 ✂ ⚠), in one colour like the rest.
const EMOJI: &[u8] = include_bytes!("../../assets/fonts/NotoEmoji-Subset.ttf");

/// The bundled fonts, in the order they are tried after the letters font.
pub const BUNDLED: [(&str, &[u8]); 3] = [("cutaway-symbols", SYMBOLS), ("cutaway-math", MATH), ("cutaway-emoji", EMOJI)];

/// egui's fonts plus the bundled symbol fonts, right after the letters font
/// of each family (before anything egui would look up on the computer).
pub fn definitions() -> FontDefinitions {
    let mut defs = FontDefinitions::default();
    for (name, data) in BUNDLED {
        defs.font_data.insert(name.to_owned(), Arc::new(FontData::from_static(data)));
    }
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        let list = defs.families.entry(family).or_default();
        // After Ubuntu Light (in Monospace it comes after Hack): letters stay
        // as they were, symbols come from the bundled fonts.
        let at = list.iter().position(|n| n == "Ubuntu-Light").map_or(list.len(), |i| i + 1);
        for (i, (name, _)) in BUNDLED.iter().enumerate() {
            list.insert(at + i, (*name).to_owned());
        }
    }
    defs
}

/// Use the app's fonts in this window.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions());
}

#[cfg(test)]
mod tests {
    use super::*;
    use ab_glyph::Font as _;

    /// Every non-ASCII character in a string or char literal of `src`
    /// (comments are skipped: nobody sees those), with the line it is on.
    fn literal_chars(src: &str) -> Vec<(char, usize)> {
        let b: Vec<char> = src.chars().collect();
        let mut out = Vec::new();
        let mut line = 1;
        let mut i = 0;
        let mut take = |c: char, line: usize| {
            if !c.is_ascii() {
                out.push((c, line));
            }
        };
        while i < b.len() {
            let c = b[i];
            let next = b.get(i + 1).copied();
            if c == '\n' {
                line += 1;
                i += 1;
            } else if c == '/' && next == Some('/') {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') {
                    if b[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            } else if c == 'r' && matches!(next, Some('#' | '"')) && (i == 0 || !(b[i - 1].is_alphanumeric() || b[i - 1] == '_')) {
                // A raw string: r"…" or r#"…"#.
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                loop {
                    if j >= b.len() {
                        break;
                    }
                    if b[j] == '"' && (1..=hashes).all(|k| b.get(j + k) == Some(&'#')) {
                        j += 1 + hashes;
                        break;
                    }
                    if b[j] == '\n' {
                        line += 1;
                    }
                    take(b[j], line);
                    j += 1;
                }
                i = j;
            } else if c == '"' {
                let mut j = i + 1;
                while j < b.len() && b[j] != '"' {
                    if b[j] == '\\' {
                        j += 1;
                    } else {
                        if b[j] == '\n' {
                            line += 1;
                        }
                        take(b[j], line);
                    }
                    j += 1;
                }
                i = j + 1;
            } else if c == '\'' {
                // A char literal ('x', '\n'), or a lifetime ('a) left alone.
                if next == Some('\\') {
                    i += 3;
                    while i < b.len() && b[i] != '\'' {
                        i += 1;
                    }
                    i += 1;
                } else if b.get(i + 2) == Some(&'\'') {
                    take(b[i + 1], line);
                    i += 3;
                } else {
                    i += 1;
                }
            } else {
                i += 1;
            }
        }
        out
    }

    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn the_lexer_finds_strings_and_skips_comments() {
        let src = "// ✓ not this\nlet a = \"＋ Add\"; /* ✕ nor this */ let b = '↶'; let c = r#\"⋯\"#; fn f<'a>() {}\n";
        let got: String = literal_chars(src).into_iter().map(|(c, _)| c).collect();
        assert_eq!(got, "＋↶⋯");
    }

    /// Every symbol in the app's own text has a bundled font, so it draws the
    /// same on every computer and never as an empty box.
    #[test]
    fn every_symbol_the_app_writes_has_a_bundled_font() {
        let defs = definitions();
        let fonts: Vec<(String, ab_glyph::FontRef<'_>)> = defs.families[&FontFamily::Proportional]
            .iter()
            .map(|name| (name.clone(), ab_glyph::FontRef::try_from_slice((*defs.font_data[name].font).as_ref()).expect(name)))
            .collect();
        let has = |c: char| fonts.iter().any(|(_, f)| f.glyph_id(c).0 != 0);

        let mut files = Vec::new();
        rust_files(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        files.sort();
        let mut missing = Vec::new();
        for path in files {
            let src = std::fs::read_to_string(&path).unwrap();
            for (c, line) in literal_chars(&src) {
                // Joiners and the emoji-style selector are not drawn themselves.
                if matches!(c, '\u{200d}' | '\u{fe0f}' | '\u{fe0e}') || has(c) {
                    continue;
                }
                missing.push(format!("{c:?} (U+{:04X}) at {}:{line}", c as u32, path.display()));
            }
        }
        assert!(
            missing.is_empty(),
            "These characters have no bundled font, so they'd show as an empty box on some computers. \
             Add them to the fonts in assets/fonts (see assets/fonts/README.md) or use a different character:\n{}",
            missing.join("\n")
        );
    }

    /// The symbol fonts come straight after the letters font, so the
    /// computer's own fonts never get asked for the app's symbols.
    #[test]
    fn symbols_come_before_the_computers_fonts() {
        let defs = definitions();
        let list = &defs.families[&FontFamily::Proportional];
        assert_eq!(&list[..4], ["Ubuntu-Light", "cutaway-symbols", "cutaway-math", "cutaway-emoji"]);
        let mono = &defs.families[&FontFamily::Monospace];
        assert_eq!(&mono[..2], ["Hack", "Ubuntu-Light"]);
        assert_eq!(&mono[2..5], ["cutaway-symbols", "cutaway-math", "cutaway-emoji"]);
    }
}
