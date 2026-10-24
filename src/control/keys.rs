//! Pictures for Stream Deck keys: a live thumbnail of a scene or source
//! with its name, outlined red when it's on air and green in preview.

use super::state::ShowState;
use crate::engine::text::{ascent, draw_text, ellipsize, fill_rect, system_fonts, text_width};
use crate::engine::{Frame, FrameSlot};
use crate::scene::{place, Scene};
use image::{Rgba, RgbaImage};
use std::sync::Arc;

/// What key pictures are made from (kept up to date by the app).
#[derive(Clone, Default)]
pub struct KeySources {
    pub canvas: (u32, u32),
    pub background: [u8; 3],
    pub scenes: Vec<Scene>,
    /// Each source's multiview picture.
    pub previews: Vec<Arc<FrameSlot>>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KeyKind {
    Scene(usize),
    Source(usize),
}

const PANEL: [u8; 4] = [22, 25, 31, 255];
const RAISED: [u8; 4] = [34, 38, 46, 255];
const RED: [u8; 4] = [232, 52, 64, 255];
const GREEN: [u8; 4] = [58, 196, 112, 255];
const GREEN_DIM: [u8; 4] = [28, 110, 64, 255];
const AMBER: [u8; 3] = [245, 166, 35];

fn sample(f: &Frame, u: f32, v: f32) -> [f32; 4] {
    let x = (u * f.width as f32 - 0.5).clamp(0.0, f.width as f32 - 1.0);
    let y = (v * f.height as f32 - 0.5).clamp(0.0, f.height as f32 - 1.0);
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(f.width - 1), (y0 + 1).min(f.height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let px = |x: usize, y: usize| {
        let i = (y * f.width + x) * 4;
        [f.rgba[i] as f32, f.rgba[i + 1] as f32, f.rgba[i + 2] as f32, f.rgba[i + 3] as f32]
    };
    let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
    let mut out = [0.0; 4];
    for k in 0..4 {
        out[k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
    }
    out
}

/// Draw `f` scaled into the rectangle (clipped to `clip`).
fn draw_frame(img: &mut RgbaImage, f: &Frame, rect: [f32; 4], clip: [i32; 4]) {
    draw_part(img, f, rect, [0.0, 0.0, 1.0, 1.0], clip)
}

/// The part `part` ([x0, y0, x1, y1] as fractions) of the picture in `rect`.
fn draw_part(img: &mut RgbaImage, f: &Frame, rect: [f32; 4], part: [f32; 4], clip: [i32; 4]) {
    if f.width == 0 || f.height == 0 || f.rgba.len() < f.width * f.height * 4 {
        return;
    }
    let [rx, ry, rw, rh] = rect;
    let x0 = (rx.floor() as i32).max(clip[0]);
    let y0 = (ry.floor() as i32).max(clip[1]);
    let x1 = ((rx + rw).ceil() as i32).min(clip[2]).min(img.width() as i32);
    let y1 = ((ry + rh).ceil() as i32).min(clip[3]).min(img.height() as i32);
    for py in y0.max(0)..y1 {
        for px in x0.max(0)..x1 {
            let u = (px as f32 + 0.5 - rx) / rw;
            let v = (py as f32 + 0.5 - ry) / rh;
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                continue;
            }
            let s = sample(f, part[0] + u * (part[2] - part[0]), part[1] + v * (part[3] - part[1]));
            let a = s[3] / 255.0;
            let d = img.get_pixel_mut(px as u32, py as u32);
            for k in 0..3 {
                d[k] = (s[k] * a + d[k] as f32 * (1.0 - a)).round() as u8;
            }
        }
    }
}

/// A key picture, `size` pixels square (144 for Stream Deck at 2x).
pub fn render(src: &KeySources, state: &ShowState, kind: KeyKind, size: u32, label: bool) -> Option<RgbaImage> {
    let size = size.clamp(32, 512);
    let s = size as f32;
    let (name, tally, offline) = match kind {
        KeyKind::Scene(i) => {
            let sc = state.scenes.get(i)?;
            (sc.name.clone(), if sc.program { "program" } else if sc.preview { "preview" } else { "off" }, false)
        }
        KeyKind::Source(i) => {
            let so = state.sources.get(i)?;
            (so.name.clone(), so.tally.as_str(), so.offline)
        }
    };
    let mut img = RgbaImage::from_pixel(size, size, Rgba(PANEL));
    let border = (s / 22.0).round().max(3.0);
    let label_h = if label { (s * 0.27).round() } else { 0.0 };
    let area = [border, border, s - 2.0 * border, s - 2.0 * border - label_h];
    let aspect = src.canvas.0.max(1) as f32 / src.canvas.1.max(1) as f32;
    // The picture, fitted into its area.
    let (pw, ph) = if area[2] / area[3].max(1.0) > aspect { (area[3] * aspect, area[3]) } else { (area[2], area[2] / aspect) };
    let pic = [area[0] + (area[2] - pw) / 2.0, area[1] + (area[3] - ph) / 2.0, pw, ph];
    let clip = [pic[0].floor() as i32, pic[1].floor() as i32, (pic[0] + pic[2]).ceil() as i32, (pic[1] + pic[3]).ceil() as i32];
    match kind {
        KeyKind::Scene(i) => {
            let bg = src.background;
            fill_rect(&mut img, clip[0], clip[1], clip[2] - clip[0], clip[3] - clip[1], [bg[0], bg[1], bg[2], 255]);
            let scene = src.scenes.get(i);
            for layer in scene.map(|s| s.layers.as_slice()).unwrap_or(&[]) {
                let Some((_, f)) = src.previews.get(layer.source).and_then(|p| p.latest()) else { continue };
                // Placed as on air: filled or fitted, zoomed.
                let ((x, y, w, h), part) = place(layer, src.canvas, f.width as f32 / f.height.max(1) as f32);
                let k = pic[2] / src.canvas.0.max(1) as f32;
                draw_part(&mut img, &f, [pic[0] + x * k, pic[1] + y * k, w * k, h * k], part, clip);
            }
        }
        KeyKind::Source(i) => {
            fill_rect(&mut img, clip[0], clip[1], clip[2] - clip[0], clip[3] - clip[1], [0, 0, 0, 255]);
            if let Some((_, f)) = src.previews.get(i).and_then(|p| p.latest()) {
                let fa = f.width as f32 / f.height.max(1) as f32;
                let (w, h) = if pic[2] / pic[3] > fa { (pic[3] * fa, pic[3]) } else { (pic[2], pic[2] / fa) };
                draw_frame(&mut img, &f, [pic[0] + (pic[2] - w) / 2.0, pic[1] + (pic[3] - h) / 2.0, w, h], clip);
            }
            if offline {
                fill_rect(&mut img, clip[0], clip[1], clip[2] - clip[0], clip[3] - clip[1], [0, 0, 0, 170]);
                let (bold, _) = system_fonts();
                let px = s * 0.12;
                let t = "NO SIGNAL";
                let w = text_width(bold, px, t);
                draw_text(&mut img, bold, px, pic[0] + (pic[2] - w) / 2.0, pic[1] + pic[3] / 2.0 + px * 0.35, t, AMBER);
            }
        }
    }
    let edge = match tally {
        "program" => Some(RED),
        "preview" => Some(GREEN),
        _ => None,
    };
    if let Some(c) = edge {
        let b = border as i32;
        let n = size as i32;
        fill_rect(&mut img, 0, 0, n, b, c);
        fill_rect(&mut img, 0, n - b, n, b, c);
        fill_rect(&mut img, 0, 0, b, n, c);
        fill_rect(&mut img, n - b, 0, b, n, c);
    }
    if label {
        let fill = match tally {
            "program" => RED,
            "preview" => GREEN_DIM,
            _ => RAISED,
        };
        let top = s - border - label_h;
        fill_rect(&mut img, border as i32, top as i32, (s - 2.0 * border) as i32, label_h as i32, fill);
        let (bold, _) = system_fonts();
        let mut px = s * 0.15;
        let max_w = s - 2.0 * border - s * 0.06;
        // Shrink a little before shortening.
        while px > s * 0.11 && text_width(bold, px, &name) > max_w {
            px -= 1.0;
        }
        let text = ellipsize(bold, px, &name, max_w);
        let w = text_width(bold, px, &text);
        let baseline = top + (label_h - px) / 2.0 + ascent(bold, px) * 0.92;
        draw_text(&mut img, bold, px, (s - w) / 2.0, baseline, &text, [255, 255, 255]);
    }
    Some(img)
}

pub fn png(img: &RgbaImage) -> Option<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::super::state::{SceneState, SourceState};
    use super::*;
    use crate::scene::Layer;

    fn slot(rgb: [u8; 3]) -> Arc<FrameSlot> {
        let slot = FrameSlot::new();
        let (w, h) = (64usize, 36usize);
        let mut rgba = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        slot.put(Frame { width: w, height: h, rgba });
        slot
    }

    #[test]
    fn draws_scenes_and_sources_with_tally() {
        let src = KeySources {
            canvas: (1920, 1080),
            background: [10, 10, 10],
            scenes: vec![Scene {
                name: "Side by side".into(),
                layers: vec![Layer::at(0, 0.0, 0.25, 0.5, 0.5), Layer::at(1, 0.5, 0.25, 0.5, 0.5)],
                ..Default::default()
            }],
            previews: vec![slot([0, 0, 255]), slot([255, 255, 0])],
        };
        let state = ShowState {
            scenes: vec![SceneState { name: "Side by side".into(), program: true, preview: false }],
            sources: vec![SourceState { name: "Wide".into(), tally: "preview".into(), offline: false }, SourceState { name: "Host".into(), tally: "off".into(), offline: true }],
            ..Default::default()
        };
        let img = render(&src, &state, KeyKind::Scene(0), 144, true).unwrap();
        assert_eq!(img.dimensions(), (144, 144));
        // Red tally border, blue on the left half of the picture, yellow on the right.
        assert_eq!(img.get_pixel(1, 1).0, RED);
        let (pic_y, left_x, right_x) = (52, 40, 104);
        assert!(img.get_pixel(left_x, pic_y)[2] > 200 && img.get_pixel(left_x, pic_y)[0] < 50, "{:?}", img.get_pixel(left_x, pic_y));
        assert!(img.get_pixel(right_x, pic_y)[0] > 200 && img.get_pixel(right_x, pic_y)[1] > 200);
        // The name is drawn in white on the red label.
        assert!(img.pixels().filter(|p| p[0] > 240 && p[1] > 240 && p[2] > 240).count() > 40);
        let wide = render(&src, &state, KeyKind::Source(0), 72, false).unwrap();
        assert_eq!(wide.get_pixel(0, 0).0, GREEN);
        assert!(wide.get_pixel(36, 36)[2] > 200);
        let host = render(&src, &state, KeyKind::Source(1), 144, true).unwrap();
        assert!(host.pixels().any(|p| p[0] > 200 && p[1] > 120 && p[2] < 80), "NO SIGNAL in amber");
        assert!(png(&host).unwrap().starts_with(b"\x89PNG"));
        assert!(render(&src, &state, KeyKind::Scene(5), 144, true).is_none());
    }
}
