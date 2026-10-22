//! Scenes (layouts of frames on the program canvas) and transition maths.
//! Pure logic, fully unit-tested.

use serde::{Deserialize, Serialize};

/// One frame on the canvas. Coordinates are fractions of the canvas
/// (0.0..1.0). The source is fitted inside the frame keeping its aspect
/// ratio (any leftover area shows the background), or fills it, cropped;
/// either way it can be zoomed into, showing the part around `focus`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Layer {
    /// Index into the source list (0-based).
    pub source: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Fill the frame, cropping the picture to its shape, instead of
    /// fitting the whole picture inside it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub fill: bool,
    /// How far zoomed in (1: the whole picture).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub zoom: f32,
    /// The point of the picture (0..1 across and down) the view centres
    /// on when it's cropped or zoomed.
    #[serde(default = "centre", skip_serializing_if = "is_centre")]
    pub focus: [f32; 2],
}

fn is_false(b: &bool) -> bool {
    !*b
}
fn one() -> f32 {
    1.0
}
fn is_one(z: &f32) -> bool {
    (*z - 1.0).abs() < 1e-4
}
fn centre() -> [f32; 2] {
    [0.5, 0.5]
}
fn is_centre(f: &[f32; 2]) -> bool {
    (f[0] - 0.5).abs() < 1e-4 && (f[1] - 0.5).abs() < 1e-4
}

/// The most a frame can be zoomed into.
pub const MAX_ZOOM: f32 = 4.0;

impl Layer {
    /// A frame showing all of `source`'s picture, fitted.
    pub const fn at(source: usize, x: f32, y: f32, w: f32, h: f32) -> Layer {
        Layer { source, x, y, w, h, fill: false, zoom: 1.0, focus: [0.5, 0.5] }
    }

    /// Whether only part of the picture shows (filled or zoomed).
    pub fn crops(&self) -> bool {
        self.fill || !is_one(&self.zoom)
    }
}

/// Layers are drawn in order: later layers sit on top.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Scene {
    pub name: String,
    #[serde(default)]
    pub layers: Vec<Layer>,
    /// Audio inputs (by name) this scene turns on when it's taken.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_on: Vec<String>,
    /// Audio inputs this scene turns off.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audio_off: Vec<String>,
    /// Commands run when you take this scene: a Syrinscape mood, a sound
    /// pad, a graphic (e.g. "syrinscape:Tavern", "pad:Whoosh").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<String>,
}

impl Scene {
    pub fn layer_for(&self, source: usize) -> Option<(usize, &Layer)> {
        self.layers.iter().enumerate().find(|(_, l)| l.source == source)
    }

    /// Sources visible in this scene (for tally lights).
    pub fn sources(&self) -> Vec<usize> {
        let mut v: Vec<usize> = self.layers.iter().map(|l| l.source).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// The source this scene shows on its own, full frame (the top layer
    /// covers the whole canvas), if any.
    pub fn full_frame_source(&self) -> Option<usize> {
        let top = self.layers.last()?;
        let covers = top.x <= 0.01 && top.y <= 0.01 && top.x + top.w >= 0.99 && top.y + top.h >= 0.99;
        covers.then_some(top.source)
    }

    /// Drop layers that point at sources that no longer exist.
    pub fn retain_sources(&mut self, n_sources: usize) {
        self.layers.retain(|l| l.source < n_sources);
    }

    /// Sources were added, removed or moved: `new_of(old)` is a source's new
    /// number (None: it's gone, and so are its frames in this scene).
    pub fn remap_sources(&mut self, new_of: &dyn Fn(usize) -> Option<usize>) {
        self.layers.retain_mut(|l| match new_of(l.source) {
            Some(n) => {
                l.source = n;
                true
            }
            None => false,
        });
    }
}

/// Built-in layouts for `names.len()` sources.
pub fn presets(names: &[String]) -> Vec<Scene> {
    let n = names.len();
    let mut out: Vec<Scene> = names
        .iter()
        .enumerate()
        .map(|(i, name)| Scene { name: name.clone(), layers: vec![Layer::at(i, 0.0, 0.0, 1.0, 1.0)], ..Default::default() })
        .collect();
    if n >= 2 {
        out.push(Scene {
            name: "Side by side".into(),
            layers: vec![
                Layer::at(0, 0.01, 0.2525, 0.485, 0.485),
                Layer::at(1, 0.505, 0.2525, 0.485, 0.485),
            ],
            ..Default::default()
        });
        out.push(Scene {
            name: "Picture in picture".into(),
            layers: vec![
                Layer::at(0, 0.0, 0.0, 1.0, 1.0),
                Layer::at(1, 0.70, 0.68, 0.27, 0.27),
            ],
            ..Default::default()
        });
    }
    if n >= 3 {
        out.push(grid(n));
    }
    out
}

/// Evenly spaced grid holding all `n` sources.
pub fn grid(n: usize) -> Scene {
    let cols = (n as f32).sqrt().ceil().max(1.0) as usize;
    let rows = n.div_ceil(cols);
    let gap = 0.008;
    let cw = (1.0 - gap * (cols as f32 + 1.0)) / cols as f32;
    let ch = (1.0 - gap * (rows as f32 + 1.0)) / rows as f32;
    let layers = (0..n)
        .map(|i| {
            let (r, c) = (i / cols, i % cols);
            // Centre a short last row.
            let in_row = if r == rows - 1 { n - r * cols } else { cols };
            let row_offset = (cols - in_row) as f32 * (cw + gap) / 2.0;
            Layer::at(i, gap + c as f32 * (cw + gap) + row_offset, gap + r as f32 * (ch + gap), cw, ch)
        })
        .collect();
    Scene { name: "Grid".into(), layers, ..Default::default() }
}

/// Position of one compositor input, in canvas pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PadState {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub alpha: f64,
    pub zorder: u32,
    /// The part of the source's picture shown, [x0, y0, x1, y1] as
    /// fractions of it (after its own crop in Picture adjustments).
    pub crop: [f32; 4],
}

impl PadState {
    pub const WHOLE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];
    pub const HIDDEN: PadState = PadState { x: 0, y: 0, w: 16, h: 16, alpha: 0.0, zorder: 1, crop: PadState::WHOLE };
}

/// Where a source of aspect `src_aspect` goes in its frame, (x, y, w, h)
/// in pixels, and which part of its picture shows there, [x0, y0, x1, y1]
/// as fractions of the picture: fitted or filling, zoomed around its focus.
pub fn place(layer: &Layer, canvas: (u32, u32), src_aspect: f32) -> ((f32, f32, f32, f32), [f32; 4]) {
    let z = layer.zoom.clamp(1.0, MAX_ZOOM);
    let (rect, part) = if layer.fill && src_aspect > 0.0 {
        let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
        let rect = (layer.x * cw, layer.y * ch, (layer.w * cw).max(0.0), (layer.h * ch).max(0.0));
        let frame_aspect = rect.2 / rect.3.max(1e-3);
        // The part of the picture that has the frame's shape.
        let part = if src_aspect > frame_aspect { (frame_aspect / src_aspect, 1.0) } else { (1.0, src_aspect / frame_aspect) };
        (rect, part)
    } else {
        (fit(layer, canvas, src_aspect), (1.0, 1.0))
    };
    (rect, view(part.0 / z, part.1 / z, layer.focus))
}

/// A `w`×`h` window on the picture centred on `focus`, kept inside it.
fn view(w: f32, h: f32, focus: [f32; 2]) -> [f32; 4] {
    let (w, h) = (w.clamp(0.0, 1.0), h.clamp(0.0, 1.0));
    let x0 = (focus[0] - w / 2.0).clamp(0.0, 1.0 - w);
    let y0 = (focus[1] - h / 2.0).clamp(0.0, 1.0 - h);
    [x0, y0, x0 + w, y0 + h]
}

/// Fit a source of aspect `src_aspect` (width/height) inside a frame,
/// centred, in pixels. Returns (x, y, w, h) as floats.
pub fn fit(layer: &Layer, canvas: (u32, u32), src_aspect: f32) -> (f32, f32, f32, f32) {
    let (cw, ch) = (canvas.0 as f32, canvas.1 as f32);
    let (fx, fy, fw, fh) = (layer.x * cw, layer.y * ch, layer.w * cw, layer.h * ch);
    if fw <= 0.0 || fh <= 0.0 || src_aspect <= 0.0 {
        return (fx, fy, fw.max(0.0), fh.max(0.0));
    }
    let frame_aspect = fw / fh;
    let (w, h) = if src_aspect > frame_aspect { (fw, fw / src_aspect) } else { (fh * src_aspect, fh) };
    (fx + (fw - w) / 2.0, fy + (fh - h) / 2.0, w, h)
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Compositor state for every source while going from `from` to `to`,
/// `t` in 0..=1. Shared sources glide between positions; sources only in
/// `from` fade out; sources only in `to` fade in on top.
/// With `from = None` (or t >= 1) this is simply the `to` scene.
pub fn compose(
    from: Option<&Scene>,
    to: &Scene,
    t: f32,
    canvas: (u32, u32),
    aspects: &[f32],
) -> Vec<PadState> {
    let e = ease(t);
    let done = from.is_none() || t >= 1.0;
    aspects
        .iter()
        .enumerate()
        .map(|(s, &aspect)| {
            let a = if done { None } else { from.and_then(|f| f.layer_for(s)) };
            let b = to.layer_for(s);
            let px = |l: &Layer| place(l, canvas, aspect);
            let ((rect, crop), alpha, z) = match (a, b) {
                (None, None) => return PadState::HIDDEN,
                (None, Some((ib, lb))) => {
                    let alpha = if done { 1.0 } else { e };
                    let z = if done { 100 + ib } else { 200 + ib };
                    (px(lb), alpha, z)
                }
                (Some((ia, la)), None) => (px(la), 1.0 - e, 10 + ia),
                (Some((_, la)), Some((ib, lb))) => {
                    // The frame glides, and the picture's crop with it.
                    let ((ra, ca), (rb, cb)) = (px(la), px(lb));
                    let r = (lerp(ra.0, rb.0, e), lerp(ra.1, rb.1, e), lerp(ra.2, rb.2, e), lerp(ra.3, rb.3, e));
                    let c = [lerp(ca[0], cb[0], e), lerp(ca[1], cb[1], e), lerp(ca[2], cb[2], e), lerp(ca[3], cb[3], e)];
                    ((r, c), 1.0, 100 + ib)
                }
            };
            let (x, y, w, h) = rect;
            let (x0, y0) = (x.round() as i32, y.round() as i32);
            let (x1, y1) = ((x + w).round() as i32, (y + h).round() as i32);
            PadState {
                x: x0,
                y: y0,
                w: (x1 - x0).max(2),
                h: (y1 - y0).max(2),
                alpha: alpha as f64,
                zorder: z as u32 + 1,
                crop,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CANVAS: (u32, u32) = (1920, 1080);

    fn names(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("Cam {i}")).collect()
    }

    #[test]
    fn presets_cover_sources() {
        let p = presets(&names(4));
        assert_eq!(p[0].name, "Cam 1");
        assert_eq!(p.len(), 4 + 3);
        let grid = p.iter().find(|s| s.name == "Grid").unwrap();
        assert_eq!(grid.sources(), vec![0, 1, 2, 3]);
        assert_eq!(presets(&names(1)).len(), 1);
    }

    #[test]
    fn grid_cells_stay_on_canvas_and_do_not_overlap() {
        for n in 1..=12 {
            let g = grid(n);
            assert_eq!(g.layers.len(), n);
            for l in &g.layers {
                assert!(l.x >= 0.0 && l.y >= 0.0 && l.x + l.w <= 1.0001 && l.y + l.h <= 1.0001, "n={n} {l:?}");
            }
            for (i, a) in g.layers.iter().enumerate() {
                for b in &g.layers[i + 1..] {
                    let overlap = a.x < b.x + b.w - 1e-4 && b.x < a.x + a.w - 1e-4 && a.y < b.y + b.h - 1e-4 && b.y < a.y + a.h - 1e-4;
                    assert!(!overlap, "n={n}");
                }
            }
        }
    }

    #[test]
    fn fit_letterboxes() {
        let full = Layer::at(0, 0.0, 0.0, 1.0, 1.0);
        // 16:9 on 16:9 fills exactly.
        assert_eq!(fit(&full, CANVAS, 16.0 / 9.0), (0.0, 0.0, 1920.0, 1080.0));
        // 4:3 gets pillarboxed and centred.
        let (x, y, w, h) = fit(&full, CANVAS, 4.0 / 3.0);
        assert_eq!((x, y, w, h), (240.0, 0.0, 1440.0, 1080.0));
    }

    #[test]
    fn fill_crops_to_the_frame() {
        // A 16:9 picture in a square frame, filled: the middle 9/16 shows.
        let mut l = Layer::at(0, 0.0, 0.0, 0.5, 960.0 / 1080.0);
        l.fill = true;
        let (r, c) = place(&l, CANVAS, 16.0 / 9.0);
        assert_eq!(r, (0.0, 0.0, 960.0, 960.0));
        assert!((c[2] - c[0] - 0.5625).abs() < 1e-4 && (c[3] - c[1] - 1.0).abs() < 1e-4, "{c:?}");
        assert!((c[0] - (1.0 - 0.5625) / 2.0).abs() < 1e-4, "centred {c:?}");
        // Focused on the left edge: the view stops at the picture's edge.
        l.focus = [0.0, 0.5];
        assert!(place(&l, CANVAS, 16.0 / 9.0).1[0].abs() < 1e-6);
        // Fitted (the default), the whole picture shows, letterboxed.
        let fitted = Layer::at(0, 0.0, 0.0, 0.5, 960.0 / 1080.0);
        let (r, c) = place(&fitted, CANVAS, 16.0 / 9.0);
        assert_eq!(c, PadState::WHOLE);
        assert_eq!(r, (0.0, 210.0, 960.0, 540.0));
    }

    #[test]
    fn zoom_shows_less_around_the_focus() {
        let mut l = Layer::at(0, 0.0, 0.0, 1.0, 1.0);
        l.zoom = 2.0;
        l.focus = [0.75, 0.25];
        let (r, c) = place(&l, CANVAS, 16.0 / 9.0);
        assert_eq!(r, (0.0, 0.0, 1920.0, 1080.0));
        assert_eq!(c, [0.5, 0.0, 1.0, 0.5]);
        // Never more than the most zoom.
        l.zoom = 50.0;
        let c = place(&l, CANVAS, 16.0 / 9.0).1;
        assert!((c[2] - c[0] - 1.0 / MAX_ZOOM).abs() < 1e-5);
    }

    #[test]
    fn old_scenes_load_and_new_fields_stay_out_when_unused() {
        let l: Layer = toml::from_str("source = 1\nx = 0.0\ny = 0.0\nw = 1.0\nh = 1.0").unwrap();
        assert_eq!(l, Layer::at(1, 0.0, 0.0, 1.0, 1.0));
        let text = toml::to_string(&l).unwrap();
        assert!(!text.contains("fill") && !text.contains("zoom") && !text.contains("focus"), "{text}");
        let mut z = l;
        z.fill = true;
        z.zoom = 1.5;
        let back: Layer = toml::from_str(&toml::to_string(&z).unwrap()).unwrap();
        assert_eq!(back, z);
    }

    #[test]
    fn a_mix_glides_the_crop_too() {
        let a = Scene { name: "a".into(), layers: vec![Layer::at(0, 0.0, 0.0, 1.0, 1.0)], ..Default::default() };
        let mut b = a.clone();
        b.layers[0].zoom = 2.0;
        let mid = compose(Some(&a), &b, 0.5, CANVAS, &[16.0 / 9.0]);
        let w = mid[0].crop[2] - mid[0].crop[0];
        assert!(w < 1.0 && w > 0.5, "{w}");
    }

    #[test]
    fn cut_shows_only_target() {
        let p = presets(&names(3));
        let st = compose(None, &p[1], 0.0, CANVAS, &[16.0 / 9.0; 3]);
        assert_eq!(st[0].alpha, 0.0);
        assert_eq!(st[1].alpha, 1.0);
        assert_eq!((st[1].x, st[1].y, st[1].w, st[1].h), (0, 0, 1920, 1080));
        assert_eq!(st[2].alpha, 0.0);
    }

    #[test]
    fn mix_glides_and_fades() {
        let p = presets(&names(2));
        let full1 = &p[0];
        let pip = p.iter().find(|s| s.name == "Picture in picture").unwrap();
        let aspects = [16.0 / 9.0; 2];
        // Halfway: source 0 is in both (stays full, opaque); source 1 fades in on top.
        let mid = compose(Some(full1), pip, 0.5, CANVAS, &aspects);
        assert_eq!(mid[0].alpha, 1.0);
        assert!((mid[1].alpha - 0.5).abs() < 1e-6);
        assert!(mid[1].zorder > mid[0].zorder);
        // Going back: source 1 fades out.
        let back = compose(Some(pip), full1, 0.25, CANVAS, &aspects);
        assert!(back[1].alpha < 1.0 && back[1].alpha > 0.0);
        // Ends exactly on the target.
        let end = compose(Some(full1), pip, 1.0, CANVAS, &aspects);
        assert_eq!(end, compose(None, pip, 0.0, CANVAS, &aspects));
    }

    #[test]
    fn shared_source_moves_between_rects() {
        let names = names(2);
        let p = presets(&names);
        let sbs = p.iter().find(|s| s.name == "Side by side").unwrap();
        let aspects = [16.0 / 9.0; 2];
        let start = compose(None, &p[0], 0.0, CANVAS, &aspects)[0];
        let end = compose(None, sbs, 0.0, CANVAS, &aspects)[0];
        let mid = compose(Some(&p[0]), sbs, 0.5, CANVAS, &aspects)[0];
        assert!(mid.w < start.w && mid.w > end.w);
        assert_eq!(mid.alpha, 1.0);
    }
}
