//! Text drawn into pictures: titles, captions, Stream Deck keys.

use ab_glyph::{Font, FontArc, PxScale, ScaleFont};
use image::RgbaImage;
use std::path::Path;
use std::sync::OnceLock;

/// Fonts for titles: the one chosen, else a system font, else built in.
pub fn fonts(custom: &Path) -> (FontArc, FontArc) {
    let load = |p: &Path| std::fs::read(p).ok().and_then(|b| FontArc::try_from_vec(b).ok());
    if !custom.as_os_str().is_empty() {
        if let Some(f) = load(custom) {
            return (f.clone(), f);
        }
    }
    let bold = [
        "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        "/Library/Fonts/Arial Bold.ttf",
        "C:\\Windows\\Fonts\\arialbd.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
    ];
    let regular = [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/Library/Fonts/Arial.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
    ];
    let builtin = || FontArc::try_from_slice(epaint_default_fonts::UBUNTU_LIGHT).expect("built-in font");
    let b = bold.iter().find_map(|p| load(Path::new(p))).unwrap_or_else(builtin);
    let r = regular.iter().find_map(|p| load(Path::new(p))).unwrap_or_else(builtin);
    (b, r)
}

pub fn text_width(font: &FontArc, size: f32, text: &str) -> f32 {
    let f = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for c in text.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern(p, id);
        }
        w += f.h_advance(id);
        prev = Some(id);
    }
    w
}

pub fn draw_text(img: &mut RgbaImage, font: &FontArc, size: f32, x: f32, baseline: f32, text: &str, rgb: [u8; 3]) {
    let f = font.as_scaled(PxScale::from(size));
    let mut pen = x;
    let mut prev = None;
    for c in text.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            pen += f.kern(p, id);
        }
        let glyph = id.with_scale_and_position(PxScale::from(size), ab_glyph::point(pen, baseline));
        pen += f.h_advance(id);
        prev = Some(id);
        if let Some(outlined) = font.outline_glyph(glyph) {
            let b = outlined.px_bounds();
            outlined.draw(|gx, gy, cov| {
                let (px, py) = (b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32);
                if px < 0 || py < 0 || px >= img.width() as i32 || py >= img.height() as i32 {
                    return;
                }
                let dst = img.get_pixel_mut(px as u32, py as u32);
                let a = cov.clamp(0.0, 1.0);
                // Over-blend in straight alpha.
                let da = dst[3] as f32 / 255.0;
                let oa = a + da * (1.0 - a);
                for k in 0..3 {
                    let s = rgb[k] as f32;
                    let d = dst[k] as f32;
                    dst[k] = if oa > 0.0 { ((s * a + d * da * (1.0 - a)) / oa).round() as u8 } else { 0 };
                }
                dst[3] = (oa * 255.0).round() as u8;
            });
        }
    }
}


/// The bold and regular system fonts (or the built-in one), loaded once.
pub fn system_fonts() -> &'static (FontArc, FontArc) {
    static FONTS: OnceLock<(FontArc, FontArc)> = OnceLock::new();
    FONTS.get_or_init(|| fonts(Path::new("")))
}

/// Break `text` into lines no wider than `max_w` pixels (long words are
/// left whole).
pub fn wrap(font: &FontArc, size: f32, text: &str, max_w: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if !line.is_empty() && text_width(font, size, &candidate) > max_w {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        } else {
            line = candidate;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Shorten `text` with an ellipsis to fit `max_w` pixels.
pub fn ellipsize(font: &FontArc, size: f32, text: &str, max_w: f32) -> String {
    if text_width(font, size, text) <= max_w {
        return text.to_string();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while !chars.is_empty() {
        chars.pop();
        let s: String = chars.iter().collect::<String>().trim_end().to_string() + "…";
        if text_width(font, size, &s) <= max_w {
            return s;
        }
    }
    String::new()
}

/// Ascent of a font at a size, in pixels.
pub fn ascent(font: &FontArc, size: f32) -> f32 {
    font.as_scaled(PxScale::from(size)).ascent()
}

/// Fill a rectangle (clipped), blending straight alpha over what's there.
pub fn fill_rect(img: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, rgba: [u8; 4]) {
    let a = rgba[3] as f32 / 255.0;
    for py in y.max(0)..(y + h).min(img.height() as i32) {
        for px in x.max(0)..(x + w).min(img.width() as i32) {
            let d = img.get_pixel_mut(px as u32, py as u32);
            let da = d[3] as f32 / 255.0;
            let oa = a + da * (1.0 - a);
            for k in 0..3 {
                d[k] = if oa > 0.0 { ((rgba[k] as f32 * a + d[k] as f32 * da * (1.0 - a)) / oa).round() as u8 } else { 0 };
            }
            d[3] = (oa * 255.0).round() as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_and_shortens() {
        let (_, f) = system_fonts();
        let lines = wrap(f, 20.0, "the quick brown fox jumps over the lazy dog", 120.0);
        assert!(lines.len() >= 3, "{lines:?}");
        assert!(lines.iter().all(|l| text_width(f, 20.0, l) <= 120.0 || !l.contains(' ')));
        let s = ellipsize(f, 20.0, "a very long scene name indeed", 80.0);
        assert!(s.ends_with('…') && text_width(f, 20.0, &s) <= 80.0, "{s}");
    }
}
