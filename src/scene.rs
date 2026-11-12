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
    /// Move the frame by `dx`, `dy` (parts of the canvas), as precisely as
    /// asked (no snapping), kept where part of it still shows.
    pub fn nudge(&mut self, dx: f32, dy: f32) {
        self.x = (self.x + dx).clamp(-self.w + 0.02, 0.98);
        self.y = (self.y + dy).clamp(-self.h + 0.02, 0.98);
    }

    /// Move the picture inside its frame by `dx`, `dy` canvas pixels: the
    /// picture follows, the view goes the other way, by the part of the
    /// picture that shows (`src_aspect` is the source's shape). Kept where
    /// the view can actually be, so moving back responds at once.
    pub fn pan(&mut self, dx: f32, dy: f32, canvas: (u32, u32), src_aspect: f32) {
        let (frame, part) = place(self, canvas, src_aspect);
        let (pw, ph) = (part[2] - part[0], part[3] - part[1]);
        let fx = (self.focus[0] - dx / frame.2.max(1.0) * pw).clamp(pw / 2.0, 1.0 - pw / 2.0);
        let fy = (self.focus[1] - dy / frame.3.max(1.0) * ph).clamp(ph / 2.0, 1.0 - ph / 2.0);
        self.focus = [fx, fy];
    }

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
    /// The default layout this scene was made from, if any (Reset to
    /// default puts its frames back as that layout has them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<Preset>,
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

    /// The sources that fill most of the picture, left to right: the
    /// full-frame one, or else the biggest frame, or every frame that's as
    /// big as the biggest (two people side by side from one split camera).
    /// Name titles that follow the program name them all.
    pub fn main_sources(&self) -> Vec<usize> {
        if let Some(s) = self.full_frame_source() {
            return vec![s];
        }
        let area = |l: &Layer| l.w.clamp(0.0, 1.0) * l.h.clamp(0.0, 1.0);
        let biggest = self.layers.iter().map(area).fold(f32::MIN, f32::max);
        let mut tied: Vec<&Layer> = self.layers.iter().filter(|l| area(l) >= biggest - 1e-4).collect();
        tied.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
        let mut out: Vec<usize> = Vec::new();
        for l in tied {
            if !out.contains(&l.source) {
                out.push(l.source);
            }
        }
        out
    }

    /// The source this scene shows on its own, full frame (the top layer
    /// covers the whole canvas), if any.
    pub fn full_frame_source(&self) -> Option<usize> {
        let top = self.layers.last()?;
        let covers = top.x <= 0.01 && top.y <= 0.01 && top.x + top.w >= 0.99 && top.y + top.h >= 0.99;
        covers.then_some(top.source)
    }

    /// Show `source` in frame `layer` instead of what's there. If `source`
    /// is already in another frame of this scene (each source shows once),
    /// the two frames trade sources. Each frame keeps its place, size and
    /// Fit/Fill; how far a picture is zoomed and where it's centred go with
    /// the picture when two trade, and a source new to the scene starts
    /// whole and centred. Returns whether anything changed.
    pub fn set_source(&mut self, layer: usize, source: usize) -> bool {
        let Some(old) = self.layers.get(layer).map(|l| l.source) else { return false };
        if old == source {
            return false;
        }
        let (zoom, focus) = (self.layers[layer].zoom, self.layers[layer].focus);
        match self.layers.iter().position(|l| l.source == source) {
            Some(other) => {
                let o = &mut self.layers[other];
                let (oz, of) = (o.zoom, o.focus);
                (o.source, o.zoom, o.focus) = (old, zoom, focus);
                let l = &mut self.layers[layer];
                (l.source, l.zoom, l.focus) = (source, oz, of);
            }
            None => {
                let l = &mut self.layers[layer];
                (l.source, l.zoom, l.focus) = (source, 1.0, [0.5, 0.5]);
            }
        }
        true
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

/// The built-in layouts a scene can come from ("defaults"). A scene made
/// from one remembers which, so Reset to default can put its frames back.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// One source full screen (the 1-camera layout, one for each source).
    Full,
    /// Two cameras side by side, each half the picture.
    Two,
    /// Three cameras across.
    Three,
    /// Four cameras in a 2 × 2 grid.
    Four,
    /// Five cameras, all the same size: two stacked on the left, two on the
    /// right, one in the centre midway between them.
    Five,
    /// The screen capture large at the top, the cameras in a row below.
    Screen,
    /// One camera full screen with another small in the corner.
    PictureInPicture,
}

impl Preset {
    /// In the order the defaults go in the bank.
    pub const LAYOUTS: [Preset; 6] = [Preset::Two, Preset::Three, Preset::Four, Preset::Five, Preset::Screen, Preset::PictureInPicture];

    /// Its scene's name (Full scenes are named after their source).
    pub fn name(self) -> &'static str {
        match self {
            Preset::Full => "Full screen",
            Preset::Two => "2 cameras",
            Preset::Three => "3 cameras",
            Preset::Four => "4 cameras",
            Preset::Five => "5 cameras",
            Preset::Screen => "Screen and cameras",
            Preset::PictureInPicture => "Picture in picture",
        }
    }

    /// What it needs, for a layout that can't be made yet.
    pub fn needs(self) -> &'static str {
        match self {
            Preset::Full => "a source",
            Preset::Two | Preset::PictureInPicture => "2 cameras",
            Preset::Three => "3 cameras",
            Preset::Four => "4 cameras",
            Preset::Five => "5 cameras",
            Preset::Screen => "a screen capture and a camera",
        }
    }
}

/// Which sources go where in the default layouts: the cameras (and
/// close-ups) in source order, and the screen capture.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Slots {
    pub cameras: Vec<usize>,
    pub screen: Option<usize>,
}

/// Sort the sources into cameras and a screen. A screen capture (or a
/// guest's shared screen) goes in the screen slot; a media player isn't a
/// camera; a wide camera that close-ups are cut from is left out, as its
/// close-ups take its place. With nothing else left, every source that
/// isn't a screen counts as a camera.
pub fn slots(sources: &[crate::config::SourceConfig]) -> Slots {
    let dev = |s: &crate::config::SourceConfig| s.device.trim().to_lowercase();
    let is_screen = |s: &crate::config::SourceConfig| {
        let d = dev(s);
        crate::platform::capture::is_screen(&d) || d.starts_with("guest-screen:")
    };
    let has_close_ups = |s: &crate::config::SourceConfig| {
        let name = s.name.trim();
        sources.iter().any(|o| o.cut_from().is_some_and(|f| f.eq_ignore_ascii_case(name)))
    };
    let screen = sources.iter().position(is_screen);
    let mut cameras: Vec<usize> = sources.iter().enumerate().filter(|(_, s)| !is_screen(s) && dev(s) != "media" && !has_close_ups(s)).map(|(i, _)| i).collect();
    if cameras.is_empty() {
        cameras = sources.iter().enumerate().filter(|(_, s)| !is_screen(s)).map(|(i, _)| i).collect();
    }
    Slots { cameras, screen }
}

/// The default layouts are drawn on a 1920 × 1080 picture, with a gap of
/// 20 pixels between frames.
const PW: f32 = 1920.0;
const PH: f32 = 1080.0;
const GAP: f32 = 20.0;

/// A frame given in pixels of the 1920 × 1080 picture, filled (cropped to
/// its shape, centred).
fn tile(source: usize, x: f32, y: f32, w: f32, h: f32) -> Layer {
    Layer { fill: true, ..Layer::at(source, x / PW, y / PH, w / PW, h / PH) }
}

/// The most cameras the screen layout puts in its row.
pub const MAX_SCREEN_CAMERAS: usize = 8;

/// The frames of a default layout for these slots. `full` is the source a
/// Full scene shows. None if there aren't enough cameras (or no screen).
pub fn preset_layers(preset: Preset, slots: &Slots, full: Option<usize>) -> Option<Vec<Layer>> {
    let c = &slots.cameras;
    let need = |n: usize| (c.len() >= n).then_some(());
    let g = GAP;
    Some(match preset {
        Preset::Full => vec![Layer::at(full?, 0.0, 0.0, 1.0, 1.0)],
        Preset::Two => {
            need(2)?;
            let w = (PW - 3.0 * g) / 2.0;
            (0..2).map(|k| tile(c[k], g + k as f32 * (w + g), g, w, PH - 2.0 * g)).collect()
        }
        Preset::Three => {
            need(3)?;
            let w = (PW - 4.0 * g) / 3.0;
            (0..3).map(|k| tile(c[k], g + k as f32 * (w + g), g, w, PH - 2.0 * g)).collect()
        }
        Preset::Four => {
            need(4)?;
            let (w, h) = ((PW - 3.0 * g) / 2.0, (PH - 3.0 * g) / 2.0);
            (0..4).map(|k| tile(c[k], g + (k % 2) as f32 * (w + g), g + (k / 2) as f32 * (h + g), w, h)).collect()
        }
        Preset::Five => {
            need(5)?;
            let (w, h) = ((PW - 4.0 * g) / 3.0, (PH - 3.0 * g) / 2.0);
            let (left, mid, right) = (g, 2.0 * g + w, 3.0 * g + 2.0 * w);
            vec![
                tile(c[0], mid, (PH - h) / 2.0, w, h),
                tile(c[1], left, g, w, h),
                tile(c[2], left, 2.0 * g + h, w, h),
                tile(c[3], right, g, w, h),
                tile(c[4], right, 2.0 * g + h, w, h),
            ]
        }
        Preset::Screen => {
            let screen = slots.screen?;
            need(1)?;
            let n = c.len().min(MAX_SCREEN_CAMERAS);
            let sh = (PH - 3.0 * g) * 0.74;
            let sw = sh * 16.0 / 9.0;
            // The whole screen shows (fitted), large and centred at the top.
            let mut out = vec![Layer::at(screen, (PW - sw) / 2.0 / PW, g / PH, sw / PW, sh / PH)];
            let th = PH - 3.0 * g - sh;
            let tw = ((PW - (n as f32 + 1.0) * g) / n as f32).min(th * 16.0 / 9.0);
            let x0 = (PW - (n as f32 * tw + (n as f32 - 1.0) * g)) / 2.0;
            out.extend((0..n).map(|k| tile(c[k], x0 + k as f32 * (tw + g), 2.0 * g + sh, tw, th)));
            out
        }
        Preset::PictureInPicture => {
            need(2)?;
            vec![Layer::at(c[0], 0.0, 0.0, 1.0, 1.0), Layer::at(c[1], 0.70, 0.68, 0.27, 0.27)]
        }
    })
}

/// The source a Full scene is about: the one it's named after, or else the
/// one it shows.
fn full_source(scene: &Scene, sources: &[crate::config::SourceConfig]) -> Option<usize> {
    sources.iter().position(|s| s.name.trim().eq_ignore_ascii_case(scene.name.trim())).or_else(|| scene.layers.first().map(|l| l.source).filter(|&i| i < sources.len()))
}

/// A scene's frames as its default layout has them for these sources (for
/// Reset to default). None if it isn't from a default layout, or that
/// layout can't be made with these sources.
pub fn reset_layers(scene: &Scene, sources: &[crate::config::SourceConfig]) -> Option<Vec<Layer>> {
    let preset = scene.preset?;
    preset_layers(preset, &slots(sources), full_source(scene, sources))
}

/// The default scenes for these sources: each source full screen, then
/// each layout there are enough cameras for.
pub fn defaults(sources: &[crate::config::SourceConfig]) -> Vec<Scene> {
    let sl = slots(sources);
    let mut out: Vec<Scene> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| Scene { name: s.name.clone(), layers: vec![Layer::at(i, 0.0, 0.0, 1.0, 1.0)], preset: Some(Preset::Full), ..Default::default() })
        .collect();
    for p in Preset::LAYOUTS {
        if let Some(layers) = preset_layers(p, &sl, None) {
            out.push(Scene { name: p.name().into(), layers, preset: Some(p), ..Default::default() });
        }
    }
    out
}

/// The default scenes the bank doesn't have (deleted, or from before there
/// were defaults), named so they clash with none: a source with no scene
/// of its own full screen, and each layout no scene was made from.
pub fn missing_defaults(sources: &[crate::config::SourceConfig], existing: &[Scene]) -> Vec<Scene> {
    let mut have: Vec<Scene> = existing.to_vec();
    let mut out = Vec::new();
    for d in defaults(sources) {
        let there = match d.preset {
            Some(Preset::Full) => {
                let src = d.layers[0].source;
                have.iter().any(|s| s.layers.len() == 1 && s.layers[0].source == src && s.full_frame_source() == Some(src))
            }
            p => have.iter().any(|s| s.preset == p),
        };
        if !there {
            let s = new_scene(Some(&d), &have);
            have.push(s.clone());
            out.push(s);
        }
    }
    out
}

/// The defaults for sources with these names, all cameras (for tests).
#[cfg(test)]
pub fn presets(names: &[String]) -> Vec<Scene> {
    let sources: Vec<crate::config::SourceConfig> = names.iter().map(|n| crate::config::SourceConfig { name: n.clone(), device: "test".into(), ..Default::default() }).collect();
    defaults(&sources)
}

/// A new scene for the bank: blank, or a copy of a layout (one of
/// `defaults`), named so no other scene has its name.
pub fn new_scene(layout: Option<&Scene>, existing: &[Scene]) -> Scene {
    let base = layout.map_or("New scene", |l| l.name.as_str());
    let taken = |n: &str| existing.iter().any(|s| s.name.eq_ignore_ascii_case(n));
    let name = if taken(base) { (2..).map(|i| format!("{base} {i}")).find(|n| !taken(n)).unwrap() } else { base.to_string() };
    Scene { name, layers: layout.map(|l| l.layers.clone()).unwrap_or_default(), preset: layout.and_then(|l| l.preset), ..Default::default() }
}

/// A scene showing close-ups (sources `close`, in order) side by side, for
/// "<camera>: both" / "all three". Sized like "Side by side", centred.
pub fn side_by_side(name: &str, close: &[usize]) -> Scene {
    let n = close.len().max(1) as f32;
    let gap = 0.01;
    let w = (0.98 - gap * (n - 1.0)) / n;
    let layers = close.iter().enumerate().map(|(k, &i)| Layer::at(i, gap + k as f32 * (w + gap), (1.0 - w) / 2.0, w, w)).collect();
    Scene { name: name.into(), layers, ..Default::default() }
}

/// The scenes for close-ups just cut from a camera (a split): for each
/// camera that has two or more new virtual-PTZ close-ups (`is_new(i)`),
/// "<camera>: both" (or "all three"), unless a scene by that name exists.
pub fn split_scenes(sources: &[crate::config::SourceConfig], is_new: &dyn Fn(usize) -> bool, existing: &[Scene]) -> Vec<Scene> {
    let mut out: Vec<Scene> = Vec::new();
    for (i, s) in sources.iter().enumerate() {
        let Some(wide) = s.virtual_ptz_of().filter(|_| is_new(i)) else { continue };
        let key = wide.to_lowercase();
        if sources.iter().take(i).enumerate().any(|(j, o)| is_new(j) && o.virtual_ptz_of().is_some_and(|w| w.to_lowercase() == key)) {
            continue; // handled with the first of its group
        }
        let close: Vec<usize> = sources.iter().enumerate().filter(|(j, o)| is_new(*j) && o.virtual_ptz_of().is_some_and(|w| w.to_lowercase() == key)).map(|(j, _)| j).collect();
        if close.len() < 2 || close.len() > 4 {
            continue;
        }
        let shown = sources.iter().find(|o| o.name.eq_ignore_ascii_case(wide)).map_or(wide, |o| o.name.as_str());
        let tail = match close.len() {
            2 => "both",
            3 => "all three",
            _ => "all four",
        };
        let name = format!("{shown}: {tail}");
        if existing.iter().chain(&out).any(|e| e.name.eq_ignore_ascii_case(&name)) {
            continue;
        }
        out.push(side_by_side(&name, &close));
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

    #[test]
    fn swapping_a_source_keeps_the_frame_and_trades_with_one_already_there() {
        let mut s = Scene { name: "Three".into(), layers: vec![Layer::at(0, 0.0, 0.0, 0.33, 1.0), Layer::at(1, 0.33, 0.0, 0.33, 1.0)], ..Default::default() };
        s.layers[1].fill = true;
        s.layers[1].zoom = 2.0;
        // A source new to the scene: same place, size and Fill, whole picture.
        assert!(s.set_source(1, 3));
        assert_eq!((s.layers[1].source, s.layers[1].x, s.layers[1].fill, s.layers[1].zoom), (3, 0.33, true, 1.0));
        // One already in the scene: the two trade places, zoom going with the picture.
        s.layers[1].zoom = 1.5;
        assert!(s.set_source(0, 3));
        assert_eq!((s.layers[0].source, s.layers[0].zoom, s.layers[0].x), (3, 1.5, 0.0));
        assert_eq!((s.layers[1].source, s.layers[1].zoom, s.layers[1].x), (0, 1.0, 0.33));
        assert!(!s.set_source(0, 3));
        assert!(!s.set_source(5, 0));
    }

    #[test]
    fn nudging_moves_by_exactly_what_was_asked_and_keeps_the_frame_in_view() {
        let mut l = Layer::at(0, 0.5, 0.5, 0.25, 0.25);
        l.nudge(1.0 / 1920.0, -10.0 / 1080.0);
        assert!((l.x * 1920.0 - 961.0).abs() < 1e-3 && (l.y * 1080.0 - 530.0).abs() < 1e-3);
        l.nudge(5.0, 5.0);
        assert_eq!((l.x, l.y), (0.98, 0.98));
        l.nudge(-5.0, -5.0);
        assert!((l.x + 0.23).abs() < 1e-6);
    }

    #[test]
    fn panning_a_filled_picture_follows_the_keys_and_stops_at_its_edge() {
        // A 16:9 picture filling a tall frame: only part of it across shows.
        let mut l = Layer::at(0, 0.0, 0.0, 0.25, 1.0);
        l.fill = true;
        l.pan(10.0, 0.0, CANVAS, 16.0 / 9.0);
        assert!(l.focus[0] < 0.5, "the picture moved right, so the view moved left");
        assert_eq!(l.focus[1], 0.5, "nothing to move up and down");
        for _ in 0..1000 {
            l.pan(10.0, 0.0, CANVAS, 16.0 / 9.0);
        }
        let (_, part) = place(&l, CANVAS, 16.0 / 9.0);
        assert!(part[0].abs() < 1e-4, "stops at the picture's edge");
        // Fitted and not zoomed: nothing to move.
        let mut f = Layer::at(0, 0.0, 0.0, 0.5, 0.5);
        f.pan(50.0, 50.0, CANVAS, 16.0 / 9.0);
        assert_eq!(f.focus, [0.5, 0.5]);
    }

    #[test]
    fn a_split_gets_a_scene_with_its_close_ups_side_by_side() {
        use crate::config::SourceConfig;
        let cam = |n: &str, d: &str| SourceConfig { name: n.into(), device: d.into(), ..Default::default() };
        let src = vec![cam("Wide", "test:bars"), cam("Wide left", "vptz:Wide"), cam("Wide right", "vptz:Wide"), cam("Screen", "test:gradient")];
        let found = split_scenes(&src, &|i| i == 1 || i == 2, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "Wide: both");
        let (a, b) = (found[0].layers[0], found[0].layers[1]);
        assert_eq!((a.source, b.source), (1, 2));
        // Two square-ish frames, centred, side by side.
        assert!((a.x - 0.01).abs() < 1e-4 && (a.w - 0.485).abs() < 1e-4 && (a.h - 0.485).abs() < 1e-4);
        assert!((a.y - 0.2575).abs() < 0.01);
        assert!((b.x - 0.505).abs() < 1e-4);
        // Not again if the scene is there, or if the close-ups aren't new.
        assert!(split_scenes(&src, &|i| i == 1 || i == 2, &found).is_empty());
        assert!(split_scenes(&src, &|_| false, &[]).is_empty());
        // Three close-ups: "all three", inside the canvas.
        let src3 = vec![cam("Wide", "test:bars"), cam("a", "vptz:wide"), cam("b", "vptz:Wide"), cam("c", "vptz:Wide")];
        let s3 = &split_scenes(&src3, &|i| i > 0, &[])[0];
        assert_eq!(s3.name, "Wide: all three");
        assert!(s3.layers.iter().all(|l| l.x >= 0.0 && l.x + l.w <= 1.0));
    }

    fn names(n: usize) -> Vec<String> {
        (1..=n).map(|i| format!("Cam {i}")).collect()
    }

    #[test]
    fn as_big_frames_are_all_main_left_to_right() {
        let p = presets(&names(3));
        assert_eq!(p[1].main_sources(), vec![1], "full screen");
        let pip = p.iter().find(|s| s.name == "Picture in picture").unwrap();
        assert_eq!(pip.main_sources(), vec![pip.layers[0].source], "the big one, not the inset");
        // One camera split in two (sources 3 and 4), side by side, drawn right first.
        let mut both = Scene { name: "Cam 1: both".into(), ..Default::default() };
        both.layers = vec![Layer::at(4, 0.5, 0.0, 0.5, 1.0), Layer::at(3, 0.0, 0.0, 0.5, 1.0)];
        assert_eq!(both.main_sources(), vec![3, 4]);
        let mut two = Scene { name: "Two".into(), ..Default::default() };
        two.layers = vec![Layer::at(0, 0.0, 0.0, 0.3, 0.3), Layer::at(2, 0.3, 0.0, 0.7, 1.0)];
        assert_eq!(two.main_sources(), vec![2]);
        assert!(Scene::default().main_sources().is_empty());
    }

    #[test]
    fn new_scenes_get_a_name_of_their_own() {
        let p = presets(&names(2));
        let blank = new_scene(None, &p);
        assert_eq!((blank.name.as_str(), blank.layers.len()), ("New scene", 0));
        let pip = p.iter().find(|s| s.name == "Picture in picture").unwrap();
        let again = new_scene(Some(pip), &p);
        assert_eq!(again.name, "Picture in picture 2");
        assert_eq!(again.layers, pip.layers);
        let mut more = p.clone();
        more.push(again);
        more.push(Scene { name: "new SCENE".into(), ..Default::default() });
        assert_eq!(new_scene(Some(pip), &more).name, "Picture in picture 3");
        assert_eq!(new_scene(None, &more).name, "New scene 2");
    }

    #[test]
    fn presets_cover_sources() {
        let p = presets(&names(4));
        assert_eq!(p[0].name, "Cam 1");
        let layouts: Vec<&str> = p[4..].iter().map(|s| s.name.as_str()).collect();
        assert_eq!(layouts, ["2 cameras", "3 cameras", "4 cameras", "Picture in picture"], "5 cameras needs five; no screen");
        let four = p.iter().find(|s| s.name == "4 cameras").unwrap();
        assert_eq!(four.sources(), vec![0, 1, 2, 3]);
        assert!(p.iter().all(|s| s.preset.is_some()));
        assert_eq!(presets(&names(1)).len(), 1);
        assert_eq!(presets(&names(5)).iter().filter(|s| s.preset == Some(Preset::Five)).count(), 1);
    }

    fn src(name: &str, device: &str) -> crate::config::SourceConfig {
        crate::config::SourceConfig { name: name.into(), device: device.into(), ..Default::default() }
    }

    fn on_canvas_apart(layers: &[Layer], what: &str) {
        for l in layers {
            assert!(l.x >= 0.0 && l.y >= 0.0 && l.x + l.w <= 1.0001 && l.y + l.h <= 1.0001, "{what}: {l:?}");
        }
        for (i, a) in layers.iter().enumerate() {
            for b in &layers[i + 1..] {
                let overlap = a.x < b.x + b.w - 1e-4 && b.x < a.x + a.w - 1e-4 && a.y < b.y + b.h - 1e-4 && b.y < a.y + a.h - 1e-4;
                assert!(!overlap, "{what}: {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn default_layouts_stay_on_the_picture_and_apart() {
        let mut sources: Vec<_> = (1..=MAX_SCREEN_CAMERAS + 2).map(|i| src(&format!("Cam {i}"), "test")).collect();
        sources.push(src("Slides", "screen"));
        for n in 1..=MAX_SCREEN_CAMERAS + 2 {
            let sl = Slots { cameras: (0..n).collect(), screen: Some(sources.len() - 1) };
            for p in Preset::LAYOUTS {
                if let Some(layers) = preset_layers(p, &sl, None) {
                    on_canvas_apart(if p == Preset::PictureInPicture { &layers[1..] } else { &layers }, &format!("{p:?} with {n}"));
                }
            }
        }
    }

    #[test]
    fn five_cameras_are_all_the_same_size() {
        let l = preset_layers(Preset::Five, &Slots { cameras: vec![0, 1, 2, 3, 4], screen: None }, None).unwrap();
        assert!(l.iter().all(|f| (f.w - l[0].w).abs() < 1e-6 && (f.h - l[0].h).abs() < 1e-6 && f.fill));
        // Cam 1 in the centre, midway down; two each side, one above the other.
        assert!((l[0].x + l[0].w / 2.0 - 0.5).abs() < 1e-4 && (l[0].y + l[0].h / 2.0 - 0.5).abs() < 1e-4);
        assert!(l[1].x < l[0].x && l[2].x == l[1].x && l[2].y > l[1].y);
        assert!(l[3].x > l[0].x && l[4].x == l[3].x && l[4].y > l[3].y);
        // Three across: as tall as the picture, the same width.
        let t = preset_layers(Preset::Three, &Slots { cameras: vec![0, 1, 2], screen: None }, None).unwrap();
        assert!(t.iter().all(|f| f.h > 0.95 && (f.w - t[0].w).abs() < 1e-6));
        assert!(t[0].x < t[1].x && t[1].x < t[2].x);
    }

    #[test]
    fn the_screen_layout_puts_the_cameras_in_a_row_below() {
        let sl = Slots { cameras: vec![0, 1, 2, 3, 4], screen: Some(5) };
        let l = preset_layers(Preset::Screen, &sl, None).unwrap();
        assert_eq!(l[0].source, 5);
        assert!(!l[0].fill, "the whole screen shows");
        assert!(l[0].w > 0.65 && (l[0].x + l[0].w / 2.0 - 0.5).abs() < 1e-4);
        let row = &l[1..];
        assert_eq!(row.iter().map(|f| f.source).collect::<Vec<_>>(), vec![0, 1, 2, 3, 4]);
        assert!(row.iter().all(|f| f.y > l[0].y + l[0].h && (f.y - row[0].y).abs() < 1e-6));
        // Up to four keep a 16:9 shape, centred.
        let one = preset_layers(Preset::Screen, &Slots { cameras: vec![0], screen: Some(1) }, None).unwrap();
        assert!(((one[1].w * 16.0) / (one[1].h * 9.0) - 16.0 / 9.0).abs() < 0.01);
        assert!((one[1].x + one[1].w / 2.0 - 0.5).abs() < 1e-4);
        // No screen, or no camera: no layout.
        assert!(preset_layers(Preset::Screen, &Slots { cameras: vec![0], screen: None }, None).is_none());
        assert!(preset_layers(Preset::Screen, &Slots { cameras: vec![], screen: Some(0) }, None).is_none());
        // Many cameras: only the first eight.
        let many = preset_layers(Preset::Screen, &Slots { cameras: (0..12).collect(), screen: Some(12) }, None).unwrap();
        assert_eq!(many.len(), 1 + MAX_SCREEN_CAMERAS);
    }

    #[test]
    fn close_ups_take_the_wide_cameras_place_and_screens_go_in_the_screen_slot() {
        // Like Brayden's show: Cam 3 is cut into two close-ups.
        let sources = vec![
            src("Cam 1", "FaceTime"),
            src("Cam 2", "#2"),
            src("Cam 3", "#3"),
            src("Screen", "screen:display:Main"),
            src("Cam 3 left", "vptz:Cam 3"),
            src("Cam 3 right", "vptz: cam 3"),
            src("Clips", "media"),
        ];
        let sl = slots(&sources);
        assert_eq!(sl.cameras, vec![0, 1, 4, 5]);
        assert_eq!(sl.screen, Some(3));
        let d = defaults(&sources);
        assert_eq!(d.iter().filter(|s| s.preset == Some(Preset::Full)).count(), sources.len(), "every source full screen");
        let four = d.iter().find(|s| s.preset == Some(Preset::Four)).unwrap();
        assert_eq!(four.sources(), vec![0, 1, 4, 5]);
        assert!(d.iter().any(|s| s.preset == Some(Preset::Screen)));
        assert!(!d.iter().any(|s| s.preset == Some(Preset::Five)));
        // Only screens: nothing to put beside them.
        assert!(slots(&[src("Screen", "screen")]).cameras.is_empty());
        // Only a wide camera and its close-ups... its close-ups are the cameras.
        assert_eq!(slots(&[src("Wide", "test"), src("a", "frame:Wide")]).cameras, vec![1]);
    }

    #[test]
    fn reset_puts_the_default_frames_back() {
        let sources: Vec<_> = (1..=3).map(|i| src(&format!("Cam {i}"), "test")).collect();
        let d = defaults(&sources);
        let mut three = d.iter().find(|s| s.preset == Some(Preset::Three)).unwrap().clone();
        let want = three.layers.clone();
        three.layers[1].x = 0.3;
        three.layers.pop();
        three.name = "Panel".into();
        assert_eq!(reset_layers(&three, &sources), Some(want));
        // A full-screen scene: the source it's named after.
        let mut cam2 = d[1].clone();
        cam2.layers.clear();
        assert_eq!(reset_layers(&cam2, &sources), Some(vec![Layer::at(1, 0.0, 0.0, 1.0, 1.0)]));
        // Too few cameras now, or not from a default: nothing.
        assert!(reset_layers(&three, &sources[..2]).is_none());
        assert!(reset_layers(&Scene { name: "Mine".into(), ..Default::default() }, &sources).is_none());
        // A copy of a default remembers it.
        assert_eq!(new_scene(Some(&three), &d).preset, Some(Preset::Three));
    }

    #[test]
    fn missing_defaults_come_back_and_others_are_left_alone() {
        let sources: Vec<_> = (1..=2).map(|i| src(&format!("Cam {i}"), "test")).collect();
        let d = defaults(&sources);
        assert!(missing_defaults(&sources, &d).is_empty());
        // An older show: its own scenes, one with a default's name.
        let mine = vec![d[0].clone(), Scene { name: "2 cameras".into(), layers: vec![Layer::at(1, 0.0, 0.0, 0.5, 0.5)], ..Default::default() }];
        let back = missing_defaults(&sources, &mine);
        assert_eq!(back.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Cam 2", "2 cameras 2", "Picture in picture"]);
        assert_eq!(back[1].preset, Some(Preset::Two));
        // The old scene is a scene of yours, not the default.
        assert_eq!(mine[1].preset, None);
    }

    #[test]
    fn a_scenes_default_is_saved_and_old_scenes_load() {
        let s = Scene { name: "Panel".into(), layers: vec![Layer::at(0, 0.0, 0.0, 1.0, 1.0)], preset: Some(Preset::PictureInPicture), ..Default::default() };
        let t = toml::to_string(&s).unwrap();
        assert!(t.contains("preset = \"picture_in_picture\""), "{t}");
        assert_eq!(toml::from_str::<Scene>(&t).unwrap(), s);
        let old: Scene = toml::from_str("name = \"Wide\"").unwrap();
        assert_eq!(old.preset, None);
        assert!(!toml::to_string(&old).unwrap().contains("preset"));
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
        let sbs = p.iter().find(|s| s.name == "2 cameras").unwrap();
        let aspects = [16.0 / 9.0; 2];
        let start = compose(None, &p[0], 0.0, CANVAS, &aspects)[0];
        let end = compose(None, sbs, 0.0, CANVAS, &aspects)[0];
        let mid = compose(Some(&p[0]), sbs, 0.5, CANVAS, &aspects)[0];
        assert!(mid.w < start.w && mid.w > end.w);
        assert_eq!(mid.alpha, 1.0);
    }
}
