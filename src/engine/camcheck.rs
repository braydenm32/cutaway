//! Check cameras: measures each camera's picture before the show and says,
//! in plain words, what to change on the camera so they all match and look
//! their best. Advice only: nothing here changes a camera or the picture.
//!
//! Everything works on the multiview's own pictures of each source (about
//! 480 px wide, before Picture adjustments), on this computer.
//!
//! Two kinds of reading:
//! - [`Scene`]: about two seconds of the normal picture with nobody moving,
//!   for exposure, flicker, black level, sharpness and grain.
//! - [`Page`]: a sheet of plain white paper held where a guest sits. The
//!   page and the light are the same for every camera, so any difference
//!   in its colour or brightness is the camera's.
//!
//! [`advise`] turns the readings into a mark and a fix per check.

use super::Frame;
use crate::config::{CameraModel, CameraNotes, SourceLook};

/// How far Look's warmth and tint go at their ends (the compositor's
/// shader uses the same numbers).
pub const WARMTH_GAIN: f32 = 0.25;
pub const TINT_GAIN: f32 = 0.2;

/// Mired (micro reciprocal degrees) per unit of ln(red / blue): how far
/// a camera's white balance moves the page's colour. From the colours of
/// light at 3200 K and 5600 K in sRGB.
const MIRED_PER_WARMTH: f32 = 100.0;

pub fn to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

pub fn to_gamma(v: f32) -> f32 {
    let v = v.max(0.0);
    if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn luma(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// A white page as one camera saw it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Page {
    /// Where it was, as fractions of the picture: left, top, right, bottom.
    pub rect: [f32; 4],
    /// Its colour as the camera sends it (0..1, not linear).
    pub rgb: [f32; 3],
    /// Parts of it are pure white: too bright to measure properly.
    pub clipped: bool,
}

impl Page {
    pub fn with_rgb(&self, rgb: [f32; 3]) -> Page {
        Page { rgb, ..*self }
    }
    fn linear(&self) -> [f32; 3] {
        self.rgb.map(|v| to_linear(v).max(1e-4))
    }
    /// ln(red / blue): above 0 is warmer (orange) than white.
    pub fn warmth(&self) -> f32 {
        let l = self.linear();
        (l[0] / l[2]).ln()
    }
    /// ln(green / the middle of red and blue): above 0 is greener.
    pub fn tint(&self) -> f32 {
        let l = self.linear();
        (l[1] / (l[0] * l[2]).sqrt()).ln()
    }
    /// Linear brightness.
    pub fn level(&self) -> f32 {
        luma(self.linear())
    }
}

/// A source's picture split into cells: each one's average colour and how
/// even it is.
struct Grid {
    w: usize,
    h: usize,
    rgb: Vec<[f32; 3]>,
    /// Spread of brightness inside the cell.
    spread: Vec<f32>,
    /// Share of the cell's pixels that are pure white in some channel.
    clip: Vec<f32>,
}

fn grid(f: &Frame, cols: usize) -> Option<Grid> {
    if f.width < cols || f.height < 8 || f.rgba.len() < f.width * f.height * 4 {
        return None;
    }
    let cell = (f.width / cols).max(1);
    let rows = (f.height / cell).max(1);
    let mut sum = vec![[0.0f64; 5]; cols * rows];
    for y in 0..rows * cell {
        let row = &f.rgba[y * f.width * 4..(y + 1) * f.width * 4];
        for x in 0..cols * cell {
            let p = &row[x * 4..x * 4 + 3];
            let (r, g, b) = (p[0] as f64 / 255.0, p[1] as f64 / 255.0, p[2] as f64 / 255.0);
            let l = 0.2126 * r + 0.7152 * g + 0.0722 * b;
            let s = &mut sum[(y / cell) * cols + x / cell];
            s[0] += r;
            s[1] += g;
            s[2] += b;
            s[3] += l * l;
            if p[0] >= 250 || p[1] >= 250 || p[2] >= 250 {
                s[4] += 1.0;
            }
        }
    }
    let n = (cell * cell) as f64;
    let mut out = Grid { w: cols, h: rows, rgb: Vec::with_capacity(sum.len()), spread: Vec::with_capacity(sum.len()), clip: Vec::with_capacity(sum.len()) };
    for s in sum {
        let rgb = [(s[0] / n) as f32, (s[1] / n) as f32, (s[2] / n) as f32];
        let mean = luma(rgb) as f64;
        out.rgb.push(rgb);
        out.spread.push((s[3] / n - mean * mean).max(0.0).sqrt() as f32);
        out.clip.push((s[4] / n) as f32);
    }
    Some(out)
}

fn colour_gap(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).abs().max((a[1] - b[1]).abs()).max((a[2] - b[2]).abs())
}

/// Finds a white page held up in front of the camera: the biggest even,
/// bright, nearly colourless patch that wasn't there in `before` (the same
/// camera's picture without it, when there is one).
pub fn find_page(f: &Frame, before: Option<&Frame>) -> Option<Page> {
    const COLS: usize = 32;
    let g = grid(f, COLS)?;
    let b = before.filter(|b| b.width == f.width && b.height == f.height).and_then(|b| grid(b, COLS));
    let n = g.rgb.len();
    let lumas: Vec<f32> = g.rgb.iter().map(|c| luma(*c)).collect();
    let median = {
        let mut s = lumas.clone();
        s.sort_by(|a, b| a.total_cmp(b));
        s[s.len() / 2]
    };
    let candidate: Vec<bool> = (0..n)
        .map(|i| {
            let c = g.rgb[i];
            let (hi, lo) = (c[0].max(c[1]).max(c[2]), c[0].min(c[1]).min(c[2]));
            let pale = hi > 0.0 && (hi - lo) / hi <= 0.42;
            let bright = lumas[i] >= 0.3 && lumas[i] >= median;
            let even = g.spread[i] <= 0.06;
            let new = b.as_ref().is_none_or(|b| colour_gap(c, b.rgb[i]) > 0.07);
            pale && bright && even && new
        })
        .collect();
    // Joined patches of a similar colour.
    let mut seen = vec![false; n];
    let mut best: Option<(f32, Vec<usize>)> = None;
    for start in 0..n {
        if !candidate[start] || seen[start] {
            continue;
        }
        let mut part = vec![start];
        seen[start] = true;
        let mut k = 0;
        while k < part.len() {
            let i = part[k];
            k += 1;
            let (x, y) = (i % g.w, i / g.w);
            let mut near = Vec::with_capacity(4);
            if x > 0 {
                near.push(i - 1);
            }
            if x + 1 < g.w {
                near.push(i + 1);
            }
            if y > 0 {
                near.push(i - g.w);
            }
            if y + 1 < g.h {
                near.push(i + g.w);
            }
            for j in near {
                if candidate[j] && !seen[j] && colour_gap(g.rgb[i], g.rgb[j]) <= 0.08 {
                    seen[j] = true;
                    part.push(j);
                }
            }
        }
        // A page is at least a few cells, and not most of the picture
        // (that's a wall).
        if part.len() < 6 || part.len() * 10 > n * 4 {
            continue;
        }
        let bright = part.iter().map(|&i| lumas[i]).sum::<f32>() / part.len() as f32;
        let score = part.len() as f32 * bright;
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, part));
        }
    }
    let (_, part) = best?;
    // Its middle: away from the edges and the fingers holding it.
    let inside: Vec<usize> = part
        .iter()
        .copied()
        .filter(|&i| {
            let (x, y) = (i % g.w, i / g.w);
            x > 0 && y > 0 && x + 1 < g.w && y + 1 < g.h && [i - 1, i + 1, i - g.w, i + g.w].iter().all(|j| part.contains(j))
        })
        .collect();
    let use_cells = if inside.len() >= 3 { &inside } else { &part };
    let mut rgb = [0.0f32; 3];
    let mut clip = 0.0;
    for &i in use_cells {
        for (a, v) in rgb.iter_mut().zip(g.rgb[i]) {
            *a += v;
        }
        clip += g.clip[i];
    }
    let k = use_cells.len() as f32;
    rgb = rgb.map(|v| v / k);
    clip /= k;
    let (mut x0, mut y0, mut x1, mut y1) = (g.w, g.h, 0, 0);
    for &i in &part {
        let (x, y) = (i % g.w, i / g.w);
        (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x + 1), y1.max(y + 1));
    }
    Some(Page {
        rect: [x0 as f32 / g.w as f32, y0 as f32 / g.h as f32, x1 as f32 / g.w as f32, y1 as f32 / g.h as f32],
        rgb,
        clipped: clip > 0.25 || rgb.iter().any(|&v| v > 0.975),
    })
}

/// Several readings of the page agree: the one to keep (their average).
pub fn settled(pages: &[Page]) -> Option<Page> {
    const NEED: usize = 4;
    if pages.len() < NEED {
        return None;
    }
    let last = &pages[pages.len() - NEED..];
    let first = last[0];
    let agree = last.iter().all(|p| colour_gap(p.rgb, first.rgb) <= 0.025 && (p.rect[0] - first.rect[0]).abs() < 0.15 && (p.rect[1] - first.rect[1]).abs() < 0.15);
    if !agree {
        return None;
    }
    let mut rgb = [0.0f32; 3];
    for p in last {
        for (a, v) in rgb.iter_mut().zip(p.rgb) {
            *a += v / NEED as f32;
        }
    }
    Some(Page { rgb, clipped: last.iter().any(|p| p.clipped), ..last[NEED - 1] })
}

/// The normal picture, measured over a couple of seconds.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Scene {
    /// Brightness (0..1, as sent): the middle, the darkest and brightest
    /// half percent.
    pub median: f32,
    pub darkest: f32,
    pub brightest: f32,
    /// Share of the picture that is pure white in some channel.
    pub clipped: f32,
    /// Share that is pure black.
    pub crushed: f32,
    /// Faces' brightness, when there are faces.
    pub face: Option<f32>,
    /// How crisp the strongest edges are (relative: compare cameras).
    pub sharpness: f32,
    /// Grain: how much a still picture changes from frame to frame.
    pub noise: f32,
    /// Bands rolling through the picture, and the whole picture pulsing
    /// (share of its brightness).
    pub bands: f32,
    pub pulse: f32,
    /// Different pictures it was measured on.
    pub frames: usize,
}

fn gray(f: &Frame) -> Vec<f32> {
    f.rgba.chunks_exact(4).take(f.width * f.height).map(|p| (0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32) / 255.0).collect()
}

/// The value `share` of the way up a list (sorted in place).
fn quantile(v: &mut [f32], share: f32) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let k = ((v.len() - 1) as f32 * share.clamp(0.0, 1.0)) as usize;
    *v.select_nth_unstable_by(k, |a, b| a.total_cmp(b)).1
}

/// Measures a camera's normal picture: `frames` in the order they came
/// (repeats of the same picture are fine), `faces` as fractions of the
/// picture (left, top, width, height).
pub fn measure_scene(frames: &[&Frame], faces: &[[f32; 4]]) -> Option<Scene> {
    let first = frames.first()?;
    let (w, h) = (first.width, first.height);
    let frames: Vec<&Frame> = frames.iter().copied().filter(|f| f.width == w && f.height == h && f.rgba.len() >= w * h * 4 && w >= 16 && h >= 16).collect();
    let mid = *frames.get(frames.len() / 2)?;
    let g = gray(mid);
    let mut sorted = g.clone();
    let mut s = Scene {
        median: quantile(&mut sorted, 0.5),
        darkest: quantile(&mut sorted, 0.005),
        brightest: quantile(&mut sorted, 0.995),
        ..Default::default()
    };
    let px = (w * h) as f32;
    s.clipped = mid.rgba.chunks_exact(4).filter(|p| p[0] >= 250 || p[1] >= 250 || p[2] >= 250).count() as f32 / px;
    s.crushed = g.iter().filter(|&&v| v <= 0.02).count() as f32 / px;
    // Faces: their middles.
    let mut face_sum = (0.0f32, 0usize);
    for r in faces {
        let (x0, y0) = (((r[0] + r[2] * 0.2) * w as f32) as usize, ((r[1] + r[3] * 0.2) * h as f32) as usize);
        let (x1, y1) = ((((r[0] + r[2] * 0.8) * w as f32) as usize).min(w), (((r[1] + r[3] * 0.8) * h as f32) as usize).min(h));
        for y in y0..y1 {
            for x in x0..x1 {
                face_sum.0 += g[y * w + x];
                face_sum.1 += 1;
            }
        }
    }
    if face_sum.1 > 0 {
        s.face = Some(face_sum.0 / face_sum.1 as f32);
    }
    // Sharpness: the strongest edges in the middle of the picture (where
    // the people are), or around the faces.
    let area = if faces.is_empty() {
        [w / 10, h / 10, w - w / 10, h - h / 10]
    } else {
        let (mut a, mut b, mut c, mut d) = (1.0f32, 1.0f32, 0.0f32, 0.0f32);
        for r in faces {
            (a, b, c, d) = (a.min(r[0] - r[2] * 0.5), b.min(r[1] - r[3] * 0.3), c.max(r[0] + r[2] * 1.5), d.max(r[1] + r[3] * 2.0));
        }
        let px = |v: f32, n: usize| ((v.clamp(0.0, 1.0) * n as f32) as usize).clamp(1, n - 1);
        [px(a, w), px(b, h), px(c, w), px(d, h)]
    };
    let mut lap = Vec::with_capacity(((area[2] - area[0]) * (area[3] - area[1])).max(1));
    for y in area[1].max(1)..area[3].min(h - 1) {
        for x in area[0].max(1)..area[2].min(w - 1) {
            let i = y * w + x;
            lap.push((4.0 * g[i] - g[i - 1] - g[i + 1] - g[i - w] - g[i + w]).abs());
        }
    }
    s.sharpness = quantile(&mut lap, 0.98);
    // Grain and flicker over time.
    let grays: Vec<Vec<f32>> = {
        let mut out: Vec<Vec<f32>> = Vec::new();
        let mut last: Option<&[u8]> = None;
        for f in &frames {
            // Each different picture once.
            if last.is_some_and(|l| l == &f.rgba[..]) {
                continue;
            }
            last = Some(&f.rgba[..]);
            out.push(if std::ptr::eq(*f, mid) { g.clone() } else { gray(f) });
        }
        out
    };
    s.frames = grays.len();
    let mut pair_noise = Vec::new();
    for p in grays.windows(2) {
        let mut d: Vec<f32> = p[0].iter().zip(&p[1]).step_by(3).filter(|(a, _)| (0.1..0.9).contains(*a)).map(|(a, b)| (a - b).abs()).collect();
        if d.len() > 100 {
            pair_noise.push(quantile(&mut d, 0.5) / std::f32::consts::SQRT_2);
        }
    }
    s.noise = quantile(&mut pair_noise, 0.5);
    if grays.len() >= 6 {
        const BANDS: usize = 32;
        let band_h = (h / BANDS).max(1);
        let rows = (h / band_h).min(BANDS);
        let profiles: Vec<Vec<f32>> = grays.iter().map(|g| (0..rows).map(|b| g[b * band_h * w..(b + 1) * band_h * w].iter().sum::<f32>() / (band_h * w) as f32).collect()).collect();
        let means: Vec<f32> = profiles.iter().map(|p| p.iter().sum::<f32>() / rows as f32).collect();
        let t = profiles.len() as f32;
        let overall = means.iter().sum::<f32>() / t;
        if overall > 0.02 {
            let var = means.iter().map(|m| (m - overall).powi(2)).sum::<f32>() / t;
            s.pulse = var.sqrt() / overall;
            let avg: Vec<f32> = (0..rows).map(|b| profiles.iter().map(|p| p[b]).sum::<f32>() / t).collect();
            let mut per_band: Vec<f32> = (0..rows)
                .map(|b| {
                    let e = profiles.iter().zip(&means).map(|(p, m)| (p[b] - avg[b] - (m - overall)).powi(2)).sum::<f32>() / t;
                    e.sqrt()
                })
                .collect();
            s.bands = quantile(&mut per_band, 0.5) / overall;
        }
    }
    Some(s)
}

/// Picture adjustments' colour, as the compositor does it, on one colour
/// (0..1, as sent). A colour look file isn't included.
pub fn apply_look(rgb: [f32; 3], l: &SourceLook) -> [f32; 3] {
    let (w, t) = (l.warmth.clamp(-1.0, 1.0), l.tint.clamp(-1.0, 1.0));
    let mut c = [rgb[0] * (1.0 + WARMTH_GAIN * w), rgb[1] * (1.0 - TINT_GAIN * t), rgb[2] * (1.0 - WARMTH_GAIN * w)];
    for v in &mut c {
        *v = (*v - 0.5) * l.contrast.clamp(0.0, 3.0) + 0.5 + l.brightness.clamp(-1.0, 1.0);
    }
    let y = luma(c);
    c.map(|v| (y + (v - y) * l.saturation.clamp(0.0, 3.0)).clamp(0.0, 1.0))
}

/// How good one check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Mark {
    /// Couldn't be measured (no page, nothing to compare with).
    Skipped,
    Good,
    Warn,
    Bad,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Check {
    Colour,
    Brightness,
    Exposure,
    Flicker,
    Blacks,
    Focus,
}

impl Check {
    pub fn label(self) -> &'static str {
        match self {
            Check::Colour => "Colour",
            Check::Brightness => "Brightness match",
            Check::Exposure => "Too dark or bright",
            Check::Flicker => "Flicker",
            Check::Blacks => "Black level",
            Check::Focus => "Focus and grain",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub check: Check,
    pub mark: Mark,
    /// What was seen.
    pub seen: String,
    /// What to do about it.
    pub fix: Option<String>,
    /// The remembered settings the fix would change, when it names
    /// numbers: (which, new value), for "Mark as done".
    pub new_settings: Vec<(Setting, String)>,
}

/// A remembered setting a fix can change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    WhiteBalance,
    Iso,
    LookWarmth,
    LookTint,
    LookBrightness,
}

impl Finding {
    fn new(check: Check, mark: Mark, seen: impl Into<String>, fix: Option<String>) -> Self {
        Finding { check, mark, seen: seen.into(), fix, new_settings: Vec::new() }
    }
}

/// One camera's readings and what's known about it.
pub struct Camera<'a> {
    pub name: &'a str,
    pub model: CameraModel,
    pub notes: &'a CameraNotes,
    pub look: &'a SourceLook,
    pub page: Option<Page>,
    pub scene: Option<Scene>,
}

impl Camera<'_> {
    /// The page as it comes out on the program (after Look).
    fn page_out(&self) -> Option<Page> {
        self.page.map(|p| p.with_rgb(apply_look(p.rgb, self.look)))
    }
}

/// The show around the cameras.
#[derive(Clone, Copy, Debug)]
pub struct Context {
    /// The program's frame rate.
    pub show_fps: u32,
}

/// A number in some text: "4300K" → 4300.
fn number_in(s: &str) -> Option<f32> {
    let digits: String = s.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    digits.parse().ok()
}

pub fn kelvin(s: &str) -> Option<f32> {
    number_in(s).filter(|k| (2000.0..=12000.0).contains(k))
}

pub fn iso(s: &str) -> Option<f32> {
    number_in(s).filter(|k| (50.0..=204800.0).contains(k))
}

/// ISO in third stops, as cameras step it.
const ISO_STEPS: [f32; 26] = [
    100.0, 125.0, 160.0, 200.0, 250.0, 320.0, 400.0, 500.0, 640.0, 800.0, 1000.0, 1250.0, 1600.0, 2000.0, 2500.0, 3200.0, 4000.0, 5000.0, 6400.0, 8000.0, 10000.0, 12800.0, 16000.0, 20000.0, 25600.0, 32000.0,
];

/// The camera's ISO step nearest `v`.
pub fn iso_step(v: f32) -> f32 {
    ISO_STEPS.iter().copied().min_by(|a, b| (a / v).ln().abs().total_cmp(&(b / v).ln().abs())).unwrap_or(v)
}

/// A white balance in kelvin moved by `mired`, to the nearest 100 K.
pub fn shift_kelvin(k: f32, mired: f32) -> f32 {
    let m = (1e6 / k + mired).max(1e6 / 12000.0);
    ((1e6 / m / 100.0).round() * 100.0).clamp(2500.0, 10000.0)
}

/// Stops as words: "a third of a stop", "about 1 stop".
fn stops_words(s: f32) -> String {
    let thirds = (s.abs() * 3.0).round() as i32;
    match thirds {
        0 | 1 => "about a third of a stop".into(),
        2 => "about two thirds of a stop".into(),
        3 => "about 1 stop".into(),
        n if n % 3 == 0 => format!("about {} stops", n / 3),
        n => format!("about {:.1} stops", n as f32 / 3.0),
    }
}

/// Look slider values as the page shows them (-100..100).
fn slider(v: f32) -> String {
    format!("{:+.0}", v * 100.0)
}

/// Look's warmth and tint that bring `page` (as sent) to `warmth`/`tint`,
/// with the rest of `look` as it is.
pub fn look_for(page: &Page, look: &SourceLook, warmth: f32, tint: f32) -> (f32, f32) {
    let mut best = (f32::MAX, look.warmth, look.tint);
    for wi in -50..=50 {
        for ti in -50..=50 {
            let l = SourceLook { warmth: wi as f32 / 50.0, tint: ti as f32 / 50.0, ..look.clone() };
            let p = page.with_rgb(apply_look(page.rgb, &l));
            let e = (p.warmth() - warmth).powi(2) + (p.tint() - tint).powi(2) + 1e-6 * (wi * wi + ti * ti) as f32;
            if e < best.0 {
                best = (e, l.warmth, l.tint);
            }
        }
    }
    (best.1, best.2)
}

/// What each check found on `cam`, matched to `reference` (None when it is
/// the reference, or the only camera).
pub fn advise(cam: &Camera, reference: Option<&Camera>, cx: &Context) -> Vec<Finding> {
    let mut out = Vec::new();
    let lumix = cam.model == CameraModel::LumixG85;
    let settings = cam.model.has_settings();
    let ref_name = reference.map(|r| r.name).unwrap_or("white");
    let ref_page = reference.and_then(|r| r.page_out());
    // Colour, from the page.
    out.push(match cam.page_out() {
        None => Finding::new(Check::Colour, Mark::Skipped, "Not measured: no white page was read.", None),
        Some(p) if p.clipped => Finding::new(Check::Colour, Mark::Skipped, "The page came out pure white, so its colour can't be read.", Some("Make the picture darker (see Brightness), then check again.".into())),
        Some(p) => {
            let (tw, tt) = ref_page.filter(|r| !r.clipped).map(|r| (r.warmth(), r.tint())).unwrap_or((0.0, 0.0));
            let (dw, dt) = (p.warmth() - tw, p.tint() - tt);
            let mark = if dw.abs() < 0.05 && dt.abs() < 0.04 {
                Mark::Good
            } else if dw.abs() < 0.13 && dt.abs() < 0.1 {
                Mark::Warn
            } else {
                Mark::Bad
            };
            let than = if reference.is_some() { format!("than {ref_name}") } else { "than white".into() };
            let mut said = Vec::new();
            if dw.abs() >= 0.05 {
                said.push(format!("{} {than}", if dw > 0.0 { "warmer (more orange)" } else { "cooler (bluer)" }));
            }
            if dt.abs() >= 0.04 {
                said.push(format!("{} {than}", if dt > 0.0 { "greener" } else { "pinker" }));
            }
            let mut f = if mark == Mark::Good {
                let seen = if reference.is_some() { format!("Matches {ref_name}.") } else { "The page comes out white.".to_string() };
                Finding::new(Check::Colour, Mark::Good, seen, None)
            } else {
                Finding::new(Check::Colour, mark, format!("The page looks {}.", said.join(" and ")), None)
            };
            if mark != Mark::Good {
                if settings {
                    let mut fix = Vec::new();
                    if dw.abs() >= 0.05 {
                        let way = if dw > 0.0 { "lower (cooler)" } else { "higher (warmer)" };
                        match kelvin(&cam.notes.white_balance) {
                            Some(k) => {
                                let to = shift_kelvin(k, MIRED_PER_WARMTH * dw);
                                fix.push(format!("On the camera, change white balance from {k:.0}K to {to:.0}K."));
                                f.new_settings.push((Setting::WhiteBalance, format!("{to:.0}K")));
                            }
                            None => fix.push(format!("On the camera, set the white balance {way}{}.", if lumix { ", or do a White Set from the same page" } else { "" })),
                        }
                    }
                    if dt.abs() >= 0.04 {
                        fix.push(format!(
                            "Nudge the white balance fine adjustment {} towards {}.",
                            if dt.abs() < 0.08 { "a step" } else { "two or three steps" },
                            if dt > 0.0 { "M (magenta)" } else { "G (green)" }
                        ));
                    }
                    f.fix = Some(fix.join(" "));
                } else {
                    let raw = cam.page.unwrap_or(p);
                    let (w, t) = look_for(&raw, cam.look, tw, tt);
                    f.fix = Some(format!("This camera has no colour settings of its own: in its Picture adjustments, set Warmth to {} and Tint to {}.", slider(w), slider(t)));
                    f.new_settings.push((Setting::LookWarmth, format!("{w}")));
                    f.new_settings.push((Setting::LookTint, format!("{t}")));
                }
            }
            f
        }
    });
    // Brightness, from the page.
    out.push(match cam.page_out() {
        None => Finding::new(Check::Brightness, Mark::Skipped, "Not measured: no white page was read.", None),
        Some(p) if p.clipped => {
            let fix = if settings { "Lower the ISO or close the aperture a click or two (leave the shutter where it is, for flicker)." } else { "Less light on the page, or move it back a little." };
            Finding::new(Check::Brightness, Mark::Bad, "Parts of the page are pure white: the picture is too bright, and faces will be too.", Some(fix.into()))
        }
        Some(p) => {
            let gamma = luma(p.rgb);
            match ref_page.filter(|r| !r.clipped) {
                Some(r) => {
                    let stops = (r.level() / p.level().max(1e-4)).log2();
                    let mark = if stops.abs() < 0.17 {
                        Mark::Good
                    } else if stops.abs() < 0.5 {
                        Mark::Warn
                    } else {
                        Mark::Bad
                    };
                    if mark == Mark::Good {
                        Finding::new(Check::Brightness, Mark::Good, format!("Matches {ref_name}."), None)
                    } else {
                        let mut f = Finding::new(Check::Brightness, mark, format!("{} {} than {ref_name}.", stops_words(stops).replacen("about", "About", 1), if stops > 0.0 { "darker" } else { "brighter" }), None);
                        f.fix = Some(if settings {
                            match iso(&cam.notes.iso) {
                                Some(i) => {
                                    let to = iso_step(i * 2f32.powf(stops));
                                    f.new_settings.push((Setting::Iso, format!("{to:.0}")));
                                    format!(
                                        "On the camera, change ISO from {i:.0} to {to:.0}{}.",
                                        if stops > 0.0 { ", or open the aperture" } else { ", or close the aperture" }
                                    )
                                }
                                None => {
                                    let clicks = ((stops.abs() * 3.0).round() as i32).max(1);
                                    format!(
                                        "On the camera, {} the ISO or the aperture by {clicks} click{} (leave the shutter where it is, for flicker).",
                                        if stops > 0.0 { "raise" } else { "lower" },
                                        if clicks == 1 { "" } else { "s" }
                                    )
                                }
                            }
                        } else {
                            let to = (cam.look.brightness + to_gamma(r.level()) - to_gamma(p.level())).clamp(-0.5, 0.5);
                            f.new_settings.push((Setting::LookBrightness, format!("{to}")));
                            format!("More light on this camera's side is best. Otherwise, in its Picture adjustments, set Brightness to {}.", slider(to))
                        });
                        f
                    }
                }
                None if gamma < 0.5 => Finding::new(
                    Check::Brightness,
                    Mark::Warn,
                    "The page comes out grey, not white: the picture is underexposed.",
                    Some(if settings { "Raise the ISO or open the aperture (leave the shutter where it is), or add light.".into() } else { "Add light in front of the people.".into() }),
                ),
                None => Finding::new(Check::Brightness, Mark::Good, if reference.is_some() { format!("{ref_name}'s page couldn't be read to compare; this one is well exposed.") } else { "The page is bright without being pure white.".into() }, None),
            }
        }
    });
    let Some(s) = cam.scene else {
        for c in [Check::Exposure, Check::Flicker, Check::Blacks, Check::Focus] {
            out.push(Finding::new(c, Mark::Skipped, "Not measured: no picture.", None));
        }
        return out;
    };
    let ref_scene = reference.and_then(|r| r.scene);
    // Exposure, from the normal picture.
    out.push({
        let darker = if settings { "Lower the ISO or close the aperture a click (leave the shutter where it is)." } else { "Less light on the people, or turn down any light pointing at the camera." };
        let brighter = if settings { "Raise the ISO or open the aperture a click, or add light." } else { "Add light in front of the people." };
        if s.clipped > 0.08 {
            Finding::new(Check::Exposure, Mark::Bad, format!("{:.0}% of the picture is pure white.", s.clipped * 100.0), Some(darker.into()))
        } else if s.face.is_some_and(|f| f > 0.86) {
            Finding::new(Check::Exposure, Mark::Warn, "Faces are very bright: skin will look flat or shiny.", Some(darker.into()))
        } else if s.face.is_some_and(|f| f < 0.28) {
            Finding::new(Check::Exposure, Mark::Warn, "Faces are dark.", Some(brighter.into()))
        } else if s.clipped > 0.02 {
            Finding::new(Check::Exposure, Mark::Warn, format!("{:.0}% of the picture is pure white (a window or a lamp?).", s.clipped * 100.0), Some(format!("If that's where people are: {}{}", darker[..1].to_lowercase(), &darker[1..])))
        } else if s.face.is_none() && s.median < 0.15 {
            Finding::new(Check::Exposure, Mark::Warn, "The picture is dark.", Some(brighter.into()))
        } else {
            Finding::new(Check::Exposure, Mark::Good, if s.face.is_some() { "Faces are well exposed." } else { "Nothing too dark or too bright." }, None)
        }
    });
    // Flicker.
    out.push({
        let amount = s.bands.max(s.pulse);
        let camera_fps = number_in(cam.notes.frequency.split(',').nth(1).unwrap_or("")).map(|v| v.round() as u32);
        let judder = camera_fps.filter(|&c| c > 0 && cx.show_fps > 0 && c != cx.show_fps && !cx.show_fps.is_multiple_of(c));
        if s.frames < 6 {
            Finding::new(Check::Flicker, Mark::Skipped, "Not enough different pictures to tell (is the picture frozen?).", None)
        } else if amount > 0.012 {
            let kind = if s.bands >= s.pulse { "Bands are rolling through the picture" } else { "The picture is pulsing brighter and darker" };
            // At 25 or 50 fps, lights flickering at 60 Hz mains; otherwise 50 Hz.
            let fix = if cx.show_fps.is_multiple_of(25) {
                "This is usually 60 Hz mains lighting: set the shutter to 1/60 or 1/120, and run the show at 30 fps.".to_string()
            } else {
                format!(
                    "This is usually 50 Hz mains lighting (Australia, the UK, Europe): set the shutter to 1/50 or 1/100{}, and run the show at 25 fps (Scenes › Program picture) with the camera at 25p.",
                    if lumix { " (or turn on Flicker Decrease at 1/100), set System Frequency to 50 Hz" } else { "" }
                )
            };
            Finding::new(Check::Flicker, if amount > 0.03 { Mark::Bad } else { Mark::Warn }, format!("{kind}: the lights and the camera's shutter are out of step (unless something was moving in the shot: check again with it still)."), Some(fix))
        } else if let Some(c) = judder {
            Finding::new(Check::Flicker, Mark::Warn, format!("No flicker, but the camera is set to {c}p and the show runs at {} fps: movement will judder.", cx.show_fps), Some(format!("Set the camera to {}p, or the show to {c} fps.", cx.show_fps)))
        } else {
            Finding::new(Check::Flicker, Mark::Good, "No flicker seen.", None)
        }
    });
    // Black level.
    out.push({
        let fix = match cam.model {
            CameraModel::LumixG85 => format!("On the camera, set Luminance Level the same as {}'s (16-235 is usual into a capture card).", if reference.is_some() { ref_name } else { "the other cameras" }),
            CameraModel::Other => "Check that the camera's video range (16-235 or 0-255, sometimes called HDMI range or luminance level) is the same on every camera.".into(),
            CameraModel::Webcam => "In its Picture adjustments, raise Contrast a little.".into(),
        };
        match ref_scene {
            Some(r) if s.darkest > r.darkest + 0.06 && s.darkest > 0.08 => Finding::new(Check::Blacks, Mark::Warn, format!("Blacks look grey and washed out next to {ref_name}."), Some(fix)),
            Some(r) if s.crushed > 0.06 && r.crushed < 0.02 => Finding::new(Check::Blacks, Mark::Warn, format!("Shadows are crushed to pure black next to {ref_name}."), Some(fix)),
            Some(_) => Finding::new(Check::Blacks, Mark::Good, format!("Matches {ref_name}."), None),
            None if s.darkest > 0.16 => Finding::new(Check::Blacks, Mark::Warn, "The darkest parts look grey (unless nothing dark is in shot).", Some(fix)),
            None => Finding::new(Check::Blacks, Mark::Good, "Blacks look black.", None),
        }
    });
    // Focus and grain.
    out.push({
        let mut seen = Vec::new();
        let mut fix = Vec::new();
        let mut mark = Mark::Good;
        let same_kind = reference.is_some_and(|r| r.model == cam.model);
        if let Some(r) = ref_scene.filter(|r| same_kind && r.sharpness > 0.0) {
            if s.sharpness < r.sharpness * 0.6 {
                seen.push(format!("softer than {ref_name} (or framed wider)"));
                mark = Mark::Warn;
            }
        }
        if let Some(l) = cam.notes.last_check.as_ref().filter(|l| l.sharpness > 0.0) {
            if s.sharpness < l.sharpness * 0.65 && mark == Mark::Good {
                seen.push(format!("softer than at the last check ({})", l.when));
                mark = Mark::Warn;
            }
        }
        if mark == Mark::Warn {
            fix.push(if settings { "Refocus on where the guest sits; manual focus keeps it there." } else { "Clean the lens, and keep people at least an arm's length away." }.to_string());
        }
        let grainy = match ref_scene {
            Some(r) if r.noise > 0.0 => s.noise > r.noise * 1.8 && s.noise > 0.004,
            _ => s.noise > 0.02,
        };
        if grainy {
            seen.push(if ref_scene.is_some() { format!("grainier than {ref_name}") } else { "grainy".into() });
            fix.push(if settings { "Lower the ISO and add light (or open the aperture)." } else { "Add light: this camera makes grain when it's dim." }.to_string());
            mark = Mark::Warn;
        }
        if mark == Mark::Good {
            Finding::new(Check::Focus, Mark::Good, "Sharp, with little grain.", None)
        } else {
            let mut said = seen.join(", and ");
            if let Some(c) = said.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            Finding::new(Check::Focus, mark, format!("{said}."), Some(fix.join(" ")))
        }
    });
    out
}

/// The worst mark of a camera's findings.
pub fn overall(f: &[Finding]) -> Mark {
    f.iter().map(|f| f.mark).max().unwrap_or(Mark::Skipped)
}

/// One thing to set once on a kind of camera.
pub struct SetupItem {
    /// Saved when ticked (keep it the same between versions).
    pub key: &'static str,
    pub text: &'static str,
}

/// The things worth setting once on every camera of this kind, so they
/// match and don't flicker.
pub fn setup_list(model: CameraModel) -> &'static [SetupItem] {
    const LUMIX: &[SetupItem] = &[
        SetupItem { key: "freq", text: "Set System Frequency to 50 Hz (where mains power is 50 Hz, like Australia) and run the show at 25 fps, so room lights don't flicker." },
        SetupItem { key: "manual", text: "Use Creative Video mode set to M, with the shutter at 1/50 (or 1/100), so the camera can't change its exposure by itself." },
        SetupItem { key: "iso", text: "Set the same fixed ISO on every camera, not Auto ISO." },
        SetupItem { key: "wb", text: "Set white balance to the same number in kelvin on every camera, or do a White Set on each with the same white page." },
        SetupItem { key: "style", text: "Use the same Photo Style (Natural or Standard) with the same contrast, sharpness and saturation." },
        SetupItem { key: "idynamic", text: "Turn off i.Dynamic and i.Resolution: they change the picture shot by shot." },
        SetupItem { key: "luminance", text: "Set the same Luminance Level on every camera (16-235 is usual into a capture card)." },
        SetupItem { key: "focus", text: "Use manual focus (or focus once and switch to MF), so it doesn't hunt during the show." },
        SetupItem { key: "hdmi_info", text: "Turn off the information shown on the HDMI output, so icons don't appear in the picture." },
        SetupItem { key: "sleep", text: "Turn off Sleep Mode (Economy) and run it on mains power (a dummy battery), so it can't switch off mid-show." },
    ];
    const OTHER: &[SetupItem] = &[
        SetupItem { key: "manual", text: "Set exposure (shutter, aperture, ISO or gain) by hand, so the camera can't change it by itself." },
        SetupItem { key: "shutter", text: "Match the shutter to the mains power: 1/50 or 1/100 where it's 50 Hz (Australia, the UK, Europe), 1/60 or 1/120 where it's 60 Hz." },
        SetupItem { key: "wb", text: "Set white balance by hand to the same number on every camera (or from the same white page)." },
        SetupItem { key: "style", text: "Use the same picture profile on every camera, and turn off automatic contrast or HDR modes." },
        SetupItem { key: "focus", text: "Use manual focus, so it doesn't hunt during the show." },
        SetupItem { key: "hdmi_info", text: "Turn off the on-screen information on the HDMI output." },
        SetupItem { key: "sleep", text: "Turn off automatic power off, and run it on mains power for long shows." },
    ];
    const WEBCAM: &[SetupItem] = &[
        SetupItem { key: "light", text: "Light the person from the front (a window or lamp behind the screen, not behind them)." },
        SetupItem { key: "effects", text: "Turn off the computer's camera effects (Portrait, Studio Light, Center Stage, background blur) so the picture doesn't change during the show." },
        SetupItem { key: "lens", text: "Wipe the lens." },
    ];
    match model {
        CameraModel::LumixG85 => LUMIX,
        CameraModel::Other => OTHER,
        CameraModel::Webcam => WEBCAM,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture of a colour, with something else drawn in a box.
    fn picture(w: usize, h: usize, bg: [u8; 3], boxed: Option<([usize; 4], [u8; 3])>) -> Frame {
        let mut rgba = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let mut c = bg;
                // A little texture so the background isn't one even colour.
                if (x / 7 + y / 5) % 3 == 0 {
                    c = c.map(|v| v.saturating_sub(40));
                }
                if let Some((r, colour)) = boxed {
                    if x >= r[0] && x < r[2] && y >= r[1] && y < r[3] {
                        c = colour;
                    }
                }
                rgba[(y * w + x) * 4..(y * w + x) * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        Frame { width: w, height: h, rgba }
    }

    #[test]
    fn finds_the_page_and_reads_its_colour() {
        let before = picture(480, 270, [90, 70, 60], None);
        let with = picture(480, 270, [90, 70, 60], Some(([150, 60, 330, 200], [230, 210, 180])));
        let p = find_page(&with, Some(&before)).expect("page found");
        assert!((p.rect[0] - 150.0 / 480.0).abs() < 0.05, "{:?}", p.rect);
        assert!((p.rect[2] - 330.0 / 480.0).abs() < 0.05, "{:?}", p.rect);
        assert!((p.rgb[0] - 230.0 / 255.0).abs() < 0.01);
        assert!(!p.clipped);
        // Orange light: warmer than white.
        assert!(p.warmth() > 0.2);
        // Nothing new in the picture: no page.
        assert!(find_page(&before, Some(&before)).is_none());
        // A pure white page is clipped.
        let white = picture(480, 270, [90, 70, 60], Some(([150, 60, 330, 200], [255, 255, 255])));
        assert!(find_page(&white, Some(&before)).unwrap().clipped);
    }

    #[test]
    fn a_wall_is_not_a_page() {
        // Without a picture from before, a pale wall filling most of it isn't taken for the page.
        let wall = picture(480, 270, [200, 200, 195], Some(([0, 0, 480, 200], [200, 200, 195])));
        let flat = Frame { rgba: wall.rgba.chunks_exact(4).flat_map(|_| [200, 200, 195, 255]).collect(), ..wall };
        assert!(find_page(&flat, None).is_none());
    }

    #[test]
    fn readings_settle_once_they_agree() {
        let p = Page { rect: [0.3, 0.2, 0.6, 0.7], rgb: [0.8, 0.8, 0.8], clipped: false };
        assert!(settled(&[p, p, p]).is_none());
        let moved = Page { rgb: [0.6, 0.8, 0.8], ..p };
        assert!(settled(&[p, p, p, moved]).is_none());
        assert_eq!(settled(&[moved, p, p, p, p]).unwrap().rgb, p.rgb);
    }

    fn still(level: u8, frames: usize, flicker: impl Fn(usize, usize) -> f32) -> Vec<Frame> {
        (0..frames)
            .map(|t| {
                let (w, h) = (320, 180);
                let mut rgba = vec![255u8; w * h * 4];
                for y in 0..h {
                    for x in 0..w {
                        // Some edges, and a little fixed pattern.
                        let base = if (x / 20 + y / 20) % 2 == 0 { level as f32 } else { level as f32 * 0.6 };
                        let v = (base * (1.0 + flicker(t, y)) + ((x * 7 + y * 13 + t * 31) % 5) as f32 - 2.0).clamp(0.0, 255.0) as u8;
                        rgba[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[v, v, v]);
                    }
                }
                Frame { width: w, height: h, rgba }
            })
            .collect()
    }

    #[test]
    fn sees_rolling_bands_and_pulsing() {
        let steady = still(150, 20, |_, _| 0.0);
        let s = measure_scene(&steady.iter().collect::<Vec<_>>(), &[]).unwrap();
        assert!(s.bands < 0.01 && s.pulse < 0.01, "{s:?}");
        assert_eq!(s.frames, 20);
        // Bands that move a little each frame.
        let rolling = still(150, 20, |t, y| 0.06 * ((y as f32 / 30.0 + t as f32 * 0.9).sin()));
        let r = measure_scene(&rolling.iter().collect::<Vec<_>>(), &[]).unwrap();
        assert!(r.bands > 0.012, "{r:?}");
        // The whole picture pulsing.
        let pulsing = still(150, 20, |t, _| if t % 2 == 0 { 0.05 } else { -0.05 });
        let p = measure_scene(&pulsing.iter().collect::<Vec<_>>(), &[]).unwrap();
        assert!(p.pulse > 0.012, "{p:?}");
    }

    #[test]
    fn soft_pictures_measure_softer() {
        let sharp = still(160, 3, |_, _| 0.0);
        let soft: Vec<Frame> = sharp
            .iter()
            .map(|f| {
                // A box blur.
                let (w, h) = (f.width, f.height);
                let mut rgba = f.rgba.clone();
                for y in 2..h - 2 {
                    for x in 2..w - 2 {
                        let mut s = 0u32;
                        for dy in 0..5 {
                            for dx in 0..5 {
                                s += f.rgba[((y + dy - 2) * w + x + dx - 2) * 4] as u32;
                            }
                        }
                        let v = (s / 25) as u8;
                        rgba[(y * w + x) * 4..(y * w + x) * 4 + 3].copy_from_slice(&[v, v, v]);
                    }
                }
                Frame { width: w, height: h, rgba }
            })
            .collect();
        let a = measure_scene(&sharp.iter().collect::<Vec<_>>(), &[]).unwrap();
        let b = measure_scene(&soft.iter().collect::<Vec<_>>(), &[]).unwrap();
        assert!(b.sharpness < a.sharpness * 0.6, "{} vs {}", b.sharpness, a.sharpness);
    }

    #[test]
    fn white_balance_and_iso_numbers() {
        // A warm picture needs a lower kelvin number.
        assert!(shift_kelvin(4300.0, 10.0) < 4300.0);
        assert_eq!(shift_kelvin(4300.0, 0.0), 4300.0);
        assert_eq!(kelvin("4300K"), Some(4300.0));
        assert_eq!(kelvin("Auto"), None);
        assert_eq!(iso("ISO 800"), Some(800.0));
        assert_eq!(iso_step(800.0 * 2f32.powf(1.0 / 3.0)), 1000.0);
        assert_eq!(iso_step(790.0), 800.0);
    }

    fn page(rgb: [f32; 3]) -> Option<Page> {
        Some(Page { rect: [0.3, 0.3, 0.6, 0.6], rgb, clipped: false })
    }

    fn scene() -> Option<Scene> {
        Some(Scene { median: 0.4, darkest: 0.03, brightest: 0.9, clipped: 0.0, crushed: 0.0, face: Some(0.55), sharpness: 0.2, noise: 0.004, bands: 0.002, pulse: 0.002, frames: 30 })
    }

    #[test]
    fn advice_for_a_warmer_darker_camera() {
        let look = SourceLook::default();
        let notes = CameraNotes { white_balance: "4300K".into(), iso: "800".into(), ..Default::default() };
        let reference = Camera { name: "Cam 1", model: CameraModel::LumixG85, notes: &notes, look: &look, page: page([0.85, 0.85, 0.85]), scene: scene() };
        let cam = Camera { name: "Cam 2", model: CameraModel::LumixG85, notes: &notes, look: &look, page: page([0.82, 0.72, 0.62]), scene: scene() };
        let f = advise(&cam, Some(&reference), &Context { show_fps: 30 });
        let colour = f.iter().find(|f| f.check == Check::Colour).unwrap();
        assert_ne!(colour.mark, Mark::Good);
        assert!(colour.seen.contains("warmer"), "{}", colour.seen);
        let fix = colour.fix.as_deref().unwrap();
        assert!(fix.contains("from 4300K to"), "{fix}");
        let to = kelvin(fix.split(" to ").nth(1).unwrap()).unwrap();
        assert!(to < 4300.0);
        let bright = f.iter().find(|f| f.check == Check::Brightness).unwrap();
        assert!(bright.seen.contains("darker"), "{}", bright.seen);
        assert!(bright.fix.as_deref().unwrap().contains("from 800 to"));
        // The reference on its own: matched against white.
        let f = advise(&reference, None, &Context { show_fps: 30 });
        assert!(f.iter().all(|f| f.mark == Mark::Good), "{f:?}");
        assert_eq!(overall(&f), Mark::Good);
    }

    #[test]
    fn a_webcam_is_fixed_in_look_and_the_fix_works() {
        let look = SourceLook::default();
        let notes = CameraNotes::default();
        let reference = Camera { name: "Cam 1", model: CameraModel::LumixG85, notes: &notes, look: &look, page: page([0.85, 0.83, 0.8]), scene: scene() };
        let cam = Camera { name: "MacBook", model: CameraModel::Webcam, notes: &notes, look: &look, page: page([0.7, 0.75, 0.85]), scene: scene() };
        let f = advise(&cam, Some(&reference), &Context { show_fps: 25 });
        let colour = f.iter().find(|f| f.check == Check::Colour).unwrap();
        assert!(colour.fix.as_deref().unwrap().contains("Warmth"), "{colour:?}");
        let w: f32 = colour.new_settings.iter().find(|s| s.0 == Setting::LookWarmth).unwrap().1.parse().unwrap();
        let t: f32 = colour.new_settings.iter().find(|s| s.0 == Setting::LookTint).unwrap().1.parse().unwrap();
        assert!(w > 0.0, "a blue camera needs warming: {w}");
        // With those set, it matches.
        let fixed = SourceLook { warmth: w, tint: t, ..Default::default() };
        let cam = Camera { look: &fixed, ..cam };
        let f = advise(&cam, Some(&reference), &Context { show_fps: 25 });
        assert_eq!(f.iter().find(|f| f.check == Check::Colour).unwrap().mark, Mark::Good, "{f:?}");
    }

    #[test]
    fn flicker_advice_follows_the_show_rate() {
        let look = SourceLook::default();
        let notes = CameraNotes::default();
        let s = Scene { bands: 0.02, ..scene().unwrap() };
        let cam = Camera { name: "Cam 1", model: CameraModel::LumixG85, notes: &notes, look: &look, page: None, scene: Some(s) };
        let at30 = advise(&cam, None, &Context { show_fps: 30 });
        let f = at30.iter().find(|f| f.check == Check::Flicker).unwrap();
        assert_eq!(f.mark, Mark::Warn);
        assert!(f.fix.as_deref().unwrap().contains("1/50"));
        let at25 = advise(&cam, None, &Context { show_fps: 25 });
        assert!(at25.iter().find(|f| f.check == Check::Flicker).unwrap().fix.as_deref().unwrap().contains("1/60"));
        // No page: the page checks are skipped, not failed.
        assert_eq!(at30.iter().find(|f| f.check == Check::Colour).unwrap().mark, Mark::Skipped);
        // A camera at 25p in a 30 fps show judders.
        let notes = CameraNotes { frequency: "50 Hz, 25p".into(), ..Default::default() };
        let cam = Camera { notes: &notes, scene: scene(), ..cam };
        let f = advise(&cam, None, &Context { show_fps: 30 });
        assert!(f.iter().find(|f| f.check == Check::Flicker).unwrap().seen.contains("judder"));
    }

    #[test]
    fn look_on_a_colour_matches_the_shader() {
        let l = SourceLook { warmth: 0.4, tint: -0.2, ..Default::default() };
        let c = apply_look([0.5, 0.5, 0.5], &l);
        assert!((c[0] - 0.5 * (1.0 + 0.25 * 0.4)).abs() < 1e-5);
        assert!((c[1] - 0.5 * (1.0 + 0.2 * 0.2)).abs() < 1e-5);
        assert!((c[2] - 0.5 * (1.0 - 0.25 * 0.4)).abs() < 1e-5);
        assert_eq!(apply_look([0.3, 0.4, 0.5], &SourceLook::default()), [0.3, 0.4, 0.5]);
    }
}
