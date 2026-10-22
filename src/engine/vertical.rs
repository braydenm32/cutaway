//! The vertical (9:16) version: each scene's tall layout, made from the
//! same sources as the program. Pure logic; the compositor draws it.
//!
//! A source fills its tall frame (cropped, not letterboxed): its framing
//! says which part of the picture shows, and how close.

use crate::config::{VerticalConfig, VerticalMode};
use crate::scene::Scene;

/// One source in the tall picture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TallFrame {
    pub source: usize,
    /// Where: x, y, w, h as fractions of the tall canvas.
    pub rect: [f32; 4],
    /// Which part of the picture: the window's place across and down (0..1).
    pub focus: [f32; 2],
    /// 1 = as much as fits.
    pub zoom: f32,
    pub alpha: f32,
    /// Drawing order (higher on top).
    pub z: u32,
}

/// Framing per source index: (focus, zoom).
pub type Framing = Vec<([f32; 2], f32)>;

/// Each source's framing from the settings (by source name).
pub fn framing(cfg: &VerticalConfig, source_names: &[String]) -> Framing {
    source_names
        .iter()
        .map(|n| {
            let f = cfg.framing_of(n);
            ([f.x.clamp(0.0, 1.0), f.y.clamp(0.0, 1.0)], f.zoom.clamp(1.0, 4.0))
        })
        .collect()
}

const FULL: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

fn covers(l: &crate::scene::Layer) -> bool {
    l.x <= 0.01 && l.y <= 0.01 && l.x + l.w >= 0.99 && l.y + l.h >= 0.99
}

/// Rows (or, past three, a two-column grid) filling the tall canvas.
fn stacked(sources: &[usize]) -> Vec<(usize, [f32; 4])> {
    let n = sources.len();
    if n == 0 {
        return Vec::new();
    }
    let cols = if n > 3 { 2 } else { 1 };
    let rows = n.div_ceil(cols);
    sources
        .iter()
        .enumerate()
        .map(|(k, &s)| {
            let (r, c) = (k / cols, k % cols);
            // A short last row is one wide frame.
            let wide = cols == 2 && r == rows - 1 && n % 2 == 1;
            let w = if wide { 1.0 } else { 1.0 / cols as f32 };
            (s, [if wide { 0.0 } else { c as f32 * w }, r as f32 / rows as f32, w, 1.0 / rows as f32])
        })
        .collect()
}

/// Where each source goes in the tall picture for `scene`, bottom first.
/// `tall_aspect`: width / height of the tall canvas.
pub fn arrange(scene: &Scene, mode: VerticalMode, one: Option<usize>, tall_aspect: f32) -> Vec<(usize, [f32; 4])> {
    // The scene's sources, top to bottom then left to right.
    let mut order: Vec<&crate::scene::Layer> = scene.layers.iter().collect();
    order.sort_by(|a, b| (a.y + a.h / 2.0).total_cmp(&(b.y + b.h / 2.0)).then((a.x + a.w / 2.0).total_cmp(&(b.x + b.w / 2.0))));
    let mut by_place: Vec<usize> = Vec::new();
    for l in order {
        if !by_place.contains(&l.source) {
            by_place.push(l.source);
        }
    }
    match mode {
        VerticalMode::One => match one.or_else(|| scene.full_frame_source()).or_else(|| by_place.first().copied()) {
            Some(s) => vec![(s, FULL)],
            None => Vec::new(),
        },
        VerticalMode::Stack => stacked(&by_place),
        VerticalMode::Auto => {
            if let Some(s) = scene.full_frame_source() {
                return vec![(s, FULL)];
            }
            // Picture in picture: the full-screen source, with the others as
            // insets down the top right (clear of the captions and app
            // buttons at the bottom).
            if let Some(main) = scene.layers.first().filter(|l| covers(l)) {
                let mut out = vec![(main.source, FULL)];
                let w = 0.46;
                let h = w * 9.0 / 16.0 * tall_aspect;
                for (k, l) in scene.layers.iter().skip(1).filter(|l| l.source != main.source).enumerate() {
                    out.push((l.source, [1.0 - w - 0.04, 0.06 + k as f32 * (h + 0.02), w, h]));
                }
                return out;
            }
            stacked(&by_place)
        }
    }
}

/// The tall frames for one scene, with framing.
pub fn frames(scene: &Scene, mode: VerticalMode, one: Option<usize>, tall_aspect: f32, framing: &Framing) -> Vec<TallFrame> {
    arrange(scene, mode, one, tall_aspect)
        .into_iter()
        .enumerate()
        .map(|(k, (source, rect))| {
            let (focus, zoom) = framing.get(source).copied().unwrap_or(([0.5, 0.5], 1.0));
            TallFrame { source, rect, focus, zoom, alpha: 1.0, z: 100 + k as u32 }
        })
        .collect()
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Going from one tall layout to another, `t` in 0..=1: shared sources
/// glide, others fade (like the program's mix).
pub fn compose(from: Option<&[TallFrame]>, to: &[TallFrame], t: f32) -> Vec<TallFrame> {
    let Some(from) = from.filter(|_| t < 1.0) else { return to.to_vec() };
    let e = ease(t);
    let mut out = Vec::new();
    for (k, a) in from.iter().enumerate() {
        if !to.iter().any(|b| b.source == a.source) {
            out.push(TallFrame { alpha: a.alpha * (1.0 - e), z: 10 + k as u32, ..*a });
        }
    }
    for (k, b) in to.iter().enumerate() {
        match from.iter().find(|a| a.source == b.source) {
            Some(a) => {
                let mut rect = [0.0; 4];
                for i in 0..4 {
                    rect[i] = a.rect[i] + (b.rect[i] - a.rect[i]) * e;
                }
                out.push(TallFrame { rect, z: 100 + k as u32, ..*b });
            }
            None => out.push(TallFrame { alpha: b.alpha * e, z: 200 + k as u32, ..*b }),
        }
    }
    out.sort_by_key(|f| f.z);
    out
}

/// The part of a picture (`src_aspect` = width / height) shown in a frame
/// of aspect `frame_aspect`: x0, y0, x1, y1 in 0..1 of the picture.
pub fn crop(src_aspect: f32, frame_aspect: f32, focus: [f32; 2], zoom: f32) -> [f32; 4] {
    let (mut w, mut h) = if src_aspect > frame_aspect { (frame_aspect / src_aspect, 1.0) } else { (1.0, src_aspect / frame_aspect) };
    let z = zoom.max(1.0);
    w /= z;
    h /= z;
    let x0 = (1.0 - w) * focus[0].clamp(0.0, 1.0);
    let y0 = (1.0 - h) * focus[1].clamp(0.0, 1.0);
    [x0, y0, x0 + w, y0 + h]
}

/// Where a graphic made for the program (`rect` in its fractions) goes in
/// the tall picture: the program's width across, in a band of the
/// program's shape. Graphics in the program's top half (logos, clocks) go
/// near the top; the rest (titles, tickers) low down, clear of the
/// captions and buttons apps put at the very bottom.
pub fn graphic_rect(rect: [f32; 4], program_aspect: f32, tall_aspect: f32) -> [f32; 4] {
    // The band the program's picture maps onto, as a fraction of the tall height.
    let band_h = tall_aspect / program_aspect;
    let band_top = if rect[1] + rect[3] / 2.0 < 0.5 { 0.05 } else { 0.86 - band_h };
    [rect[0], band_top + rect[1] * band_h, rect[2], rect[3] * band_h]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{presets, Layer};

    const TALL: f32 = 9.0 / 16.0;

    fn names(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("Cam {i}")).collect()
    }

    #[test]
    fn scenes_become_tall_layouts() {
        let p = presets(&names(4));
        // A full-screen camera fills it.
        assert_eq!(arrange(&p[1], VerticalMode::Auto, None, TALL), vec![(1, FULL)]);
        // Side by side: one above the other, left one on top.
        let sbs = p.iter().find(|s| s.name == "Side by side").unwrap();
        assert_eq!(arrange(sbs, VerticalMode::Auto, None, TALL), vec![(0, [0.0, 0.0, 1.0, 0.5]), (1, [0.0, 0.5, 1.0, 0.5])]);
        // Picture in picture keeps its inset, at the top right.
        let pip = p.iter().find(|s| s.name == "Picture in picture").unwrap();
        let a = arrange(pip, VerticalMode::Auto, None, TALL);
        assert_eq!(a[0], (0, FULL));
        assert_eq!(a[1].0, 1);
        assert!(a[1].1[0] > 0.4 && a[1].1[1] < 0.2, "{:?}", a[1]);
        // An inset keeps a 16:9 frame in the tall picture.
        let px = (a[1].1[2] * 1080.0, a[1].1[3] * 1920.0);
        assert!((px.0 / px.1 - 16.0 / 9.0).abs() < 0.01);
        // Four sources: a two-by-two grid; five: the last row is one wide frame.
        let grid = p.iter().find(|s| s.name == "Grid").unwrap();
        let g = arrange(grid, VerticalMode::Auto, None, TALL);
        assert_eq!(g.len(), 4);
        assert!(g.iter().all(|(_, r)| r[2] == 0.5 && r[3] == 0.5));
        let g5 = stacked(&[0, 1, 2, 3, 4]);
        assert_eq!(g5[4].1, [0.0, 2.0 / 3.0, 1.0, 1.0 / 3.0]);
        // Chosen: stacked even for one camera; one source even for a split.
        assert_eq!(arrange(&p[0], VerticalMode::Stack, None, TALL), vec![(0, FULL)]);
        assert_eq!(arrange(sbs, VerticalMode::One, Some(1), TALL), vec![(1, FULL)]);
        assert_eq!(arrange(sbs, VerticalMode::One, None, TALL), vec![(0, FULL)], "no choice: the first");
        assert!(arrange(&Scene::default(), VerticalMode::Auto, None, TALL).is_empty());
        // Every frame stays on the canvas.
        for s in &p {
            for (_, r) in arrange(s, VerticalMode::Auto, None, TALL) {
                assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[0] + r[2] <= 1.0001 && r[1] + r[3] <= 1.0001, "{} {r:?}", s.name);
            }
        }
        let _ = Layer::at(0, 0.0, 0.0, 1.0, 1.0);
    }

    #[test]
    fn crops_fill_the_frame_where_framed() {
        // 16:9 into 9:16: a narrow slice, in the middle by default.
        let c = crop(16.0 / 9.0, 9.0 / 16.0, [0.5, 0.5], 1.0);
        let w = c[2] - c[0];
        assert!((w - (9.0 / 16.0) / (16.0 / 9.0)).abs() < 1e-5);
        assert!((c[0] - (1.0 - w) / 2.0).abs() < 1e-5 && c[1] == 0.0 && c[3] == 1.0);
        // Framed to the left edge, and zoomed in two times.
        let l = crop(16.0 / 9.0, 9.0 / 16.0, [0.0, 0.5], 2.0);
        assert_eq!(l[0], 0.0);
        assert!(((l[2] - l[0]) - w / 2.0).abs() < 1e-5 && ((l[3] - l[1]) - 0.5).abs() < 1e-5);
        // A wide frame from a 16:9 picture: the full width, a band of its height.
        let b = crop(16.0 / 9.0, 16.0 / 4.5, [0.5, 0.0], 1.0);
        assert_eq!((b[0], b[2], b[1]), (0.0, 1.0, 0.0));
        assert!((b[3] - 0.5).abs() < 1e-5);
    }

    #[test]
    fn layouts_glide_and_fade() {
        let p = presets(&names(2));
        let f = vec![([0.5, 0.5], 1.0); 2];
        let full = frames(&p[0], VerticalMode::Auto, None, TALL, &f);
        let sbs = frames(p.iter().find(|s| s.name == "Side by side").unwrap(), VerticalMode::Auto, None, TALL, &f);
        let mid = compose(Some(&full), &sbs, 0.5);
        let s0 = mid.iter().find(|x| x.source == 0).unwrap();
        let s1 = mid.iter().find(|x| x.source == 1).unwrap();
        assert_eq!(s0.alpha, 1.0);
        assert!(s0.rect[3] < 1.0 && s0.rect[3] > 0.5, "gliding: {:?}", s0.rect);
        assert!((s1.alpha - 0.5).abs() < 1e-6 && s1.z > s0.z, "fading in on top");
        assert_eq!(compose(Some(&full), &sbs, 1.0), sbs);
        assert_eq!(compose(None, &sbs, 0.2), sbs);
        // Going back, the one that leaves fades out underneath.
        let back = compose(Some(&sbs), &full, 0.25);
        let gone = back.iter().find(|x| x.source == 1).unwrap();
        assert!(gone.alpha < 1.0 && gone.alpha > 0.0 && gone.z < back.iter().find(|x| x.source == 0).unwrap().z);
    }

    #[test]
    fn graphics_sit_low_across_the_width() {
        let lower_third = [0.05, 0.75, 0.5, 0.15];
        let r = graphic_rect(lower_third, 16.0 / 9.0, TALL);
        assert_eq!((r[0], r[2]), (0.05, 0.5));
        let bottom = r[1] + r[3];
        assert!(bottom < 0.86 && bottom > 0.7, "{r:?}");
        // The program's full picture would end 86% of the way down.
        let full = graphic_rect([0.0, 0.0, 1.0, 1.0], 16.0 / 9.0, TALL);
        assert!((full[1] + full[3] - 0.86).abs() < 1e-5);
        // A logo in the top corner stays near the top.
        let logo = graphic_rect([0.85, 0.04, 0.1, 0.1], 16.0 / 9.0, TALL);
        assert!(logo[1] > 0.05 && logo[1] < 0.07, "{logo:?}");
    }
}
