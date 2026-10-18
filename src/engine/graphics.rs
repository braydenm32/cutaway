//! The graphics layer: logos, watermarks, name titles and clocks drawn
//! above every scene, each with its own on/off button and a fade.

use super::timers::{clock_text, Timers};
use super::VIDEO_LATENCY;
use crate::config::{parse_color, Graphic, GraphicKind};
use crate::media::{clock, Ns};
use super::text::{draw_text, fonts, text_width};
use ab_glyph::{Font, PxScale, ScaleFont};
use image::RgbaImage;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// One graphic as the compositor draws it.
#[derive(Clone)]
pub struct GraphicLayer {
    /// Changes whenever the picture changes (so the GPU copy is refreshed).
    pub key: (usize, u64),
    pub image: Arc<RgbaImage>,
    /// x, y, w, h as fractions of the program picture.
    pub rect: [f32; 4],
    pub alpha: f32,
    /// Only on the stream, not the recording or monitors (captions).
    pub stream_only: bool,
}

struct Item {
    cfg: Graphic,
    image: Option<Arc<RgbaImage>>,
    error: Option<String>,
    version: u64,
    visible: bool,
    /// Program time of the last on/off change and the opacity before it.
    changed_at: Ns,
    from_alpha: f32,
    /// Clocks: what's drawn now.
    text: (String, bool),
}

impl Item {
    fn target(&self) -> f32 {
        if self.visible {
            self.cfg.opacity.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    fn alpha_at(&self, t: Ns) -> f32 {
        let fade = self.cfg.fade_ms as i64 * 1_000_000;
        let target = self.target();
        if fade <= 0 || t >= self.changed_at + fade {
            return target;
        }
        if t <= self.changed_at {
            return self.from_alpha;
        }
        let k = (t - self.changed_at) as f32 / fade as f32;
        self.from_alpha + (target - self.from_alpha) * k
    }
}

pub struct Graphics {
    canvas: (u32, u32),
    items: Mutex<Vec<Item>>,
    versions: std::sync::atomic::AtomicU64,
    pub timers: Arc<Timers>,
}

impl Graphics {
    pub fn new(cfgs: &[Graphic], canvas: (u32, u32), timers: Arc<Timers>) -> Arc<Self> {
        let g = Arc::new(Graphics { canvas, items: Mutex::new(Vec::new()), versions: Default::default(), timers });
        g.update(cfgs);
        for it in g.items.lock().unwrap().iter_mut() {
            it.visible = it.cfg.visible;
            it.from_alpha = it.target();
        }
        g
    }

    fn next_version(&self) -> u64 {
        self.versions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1
    }

    /// Settings changed: redraw graphics whose look changed; on/off states
    /// carry over by name.
    pub fn update(&self, cfgs: &[Graphic]) {
        let mut items = self.items.lock().unwrap();
        let mut old: Vec<Item> = std::mem::take(&mut *items);
        for cfg in cfgs {
            let prev = old.iter().position(|o| o.cfg.name == cfg.name).map(|i| old.remove(i));
            let item = match prev {
                Some(mut p) if p.cfg == *cfg => {
                    p.cfg = cfg.clone();
                    p
                }
                Some(p) => {
                    let (image, error, text) = self.render(cfg);
                    Item { cfg: cfg.clone(), image, error, version: self.next_version(), visible: p.visible, changed_at: p.changed_at, from_alpha: p.from_alpha, text }
                }
                None => {
                    let (image, error, text) = self.render(cfg);
                    Item { cfg: cfg.clone(), image, error, version: self.next_version(), visible: false, changed_at: 0, from_alpha: 0.0, text }
                }
            };
            items.push(item);
        }
    }

    fn render(&self, cfg: &Graphic) -> (Option<Arc<RgbaImage>>, Option<String>, (String, bool)) {
        let mut text = (String::new(), false);
        let r = match cfg.kind {
            GraphicKind::Image => load_image(&cfg.image),
            GraphicKind::Title => Ok(render_title(cfg, self.canvas)),
            GraphicKind::Clock => {
                text = clock_text(cfg, &self.timers, chrono::Local::now());
                Ok(render_clock(cfg, &text.0, text.1, self.canvas))
            }
        };
        match r {
            Ok(img) => (Some(Arc::new(img)), None, text),
            Err(e) => (None, Some(e), text),
        }
    }

    /// What a clock graphic shows now.
    pub fn clock_text(&self, index: usize) -> Option<(String, bool)> {
        let items = self.items.lock().unwrap();
        let it = items.get(index).filter(|i| i.cfg.kind == GraphicKind::Clock)?;
        Some(clock_text(&it.cfg, &self.timers, chrono::Local::now()))
    }

    /// (name, on, problem) for each graphic.
    pub fn list(&self) -> Vec<(String, bool, Option<String>)> {
        self.items.lock().unwrap().iter().map(|i| (i.cfg.name.clone(), i.visible, i.error.clone())).collect()
    }

    pub fn is_visible(&self, index: usize) -> bool {
        self.items.lock().unwrap().get(index).is_some_and(|i| i.visible)
    }

    /// Show or hide a graphic (fading as set). Timed like a scene change,
    /// so it lands on the program frame the operator saw.
    pub fn set_visible(&self, index: usize, on: bool) {
        let mut items = self.items.lock().unwrap();
        let Some(it) = items.get_mut(index) else { return };
        if it.visible == on {
            return;
        }
        let t = clock::now() - VIDEO_LATENCY;
        it.from_alpha = it.alpha_at(t);
        it.changed_at = t;
        it.visible = on;
    }

    pub fn toggle(&self, index: usize) {
        let on = !self.is_visible(index);
        self.set_visible(index, on);
    }

    /// Change a title's text live.
    pub fn set_text(&self, index: usize, title: &str, subtitle: &str) {
        let mut items = self.items.lock().unwrap();
        let Some(it) = items.get_mut(index) else { return };
        if it.cfg.title == title && it.cfg.subtitle == subtitle {
            return;
        }
        it.cfg.title = title.to_string();
        it.cfg.subtitle = subtitle.to_string();
        if it.cfg.kind == GraphicKind::Title {
            it.image = Some(Arc::new(render_title(&it.cfg, self.canvas)));
            it.version = self.next_version();
        }
    }

    /// What to draw for the program frame at time `t`.
    pub fn layers(&self, t: Ns) -> Vec<GraphicLayer> {
        let (cw, ch) = (self.canvas.0 as f32, self.canvas.1 as f32);
        let mut items = self.items.lock().unwrap();
        let mut out = Vec::new();
        let wall = chrono::Local::now() + chrono::Duration::nanoseconds(t - clock::now());
        for (i, it) in items.iter_mut().enumerate() {
            let alpha = it.alpha_at(t);
            // Clocks redraw when their text changes.
            if it.cfg.kind == GraphicKind::Clock && alpha > 0.001 {
                let text = clock_text(&it.cfg, &self.timers, wall);
                if text != it.text {
                    it.image = Some(Arc::new(render_clock(&it.cfg, &text.0, text.1, self.canvas)));
                    it.text = text;
                    it.version = self.versions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                }
            }
            let Some(img) = &it.image else { continue };
            if alpha <= 0.001 || img.width() == 0 {
                continue;
            }
            let w = it.cfg.width.clamp(0.01, 1.0);
            let h = w * cw / ch * img.height() as f32 / img.width() as f32;
            out.push(GraphicLayer { key: (i, it.version), image: img.clone(), rect: [it.cfg.x, it.cfg.y, w, h], alpha, stream_only: false });
        }
        out
    }
}

fn load_image(path: &Path) -> Result<RgbaImage, String> {
    if path.as_os_str().is_empty() {
        return Err("choose an image file".into());
    }
    let img = image::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut img = img.to_rgba8();
    // Keep GPU textures reasonable.
    if img.width() > 4096 || img.height() > 4096 {
        img = image::imageops::resize(&img, img.width().min(4096), img.height().min(4096), image::imageops::FilterType::Triangle);
    }
    Ok(img)
}

fn rgb_of(s: &str, fallback: u32) -> [u8; 3] {
    let c = parse_color(s).unwrap_or(fallback);
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

/// A lower-third name title: a box with the name and a smaller line under it.
pub fn render_title(cfg: &Graphic, canvas: (u32, u32)) -> RgbaImage {
    let (bold, regular) = fonts(&cfg.font);
    let ch = canvas.1 as f32;
    let title_px = (ch * 0.048).max(10.0);
    let sub_px = (ch * 0.032).max(8.0);
    let pad = ch * 0.022;
    let bar = (ch * 0.008).max(2.0);
    let has_sub = !cfg.subtitle.trim().is_empty();
    let tw = text_width(&bold, title_px, &cfg.title);
    let sw = if has_sub { text_width(&regular, sub_px, &cfg.subtitle) } else { 0.0 };
    let min_w = cfg.width.clamp(0.05, 1.0) * canvas.0 as f32;
    let w = (tw.max(sw) + pad * 2.0 + bar).max(min_w).ceil() as u32;
    let h = (pad * 2.0 + title_px * 1.05 + if has_sub { sub_px * 1.35 } else { 0.0 }).ceil() as u32;
    let box_rgb = rgb_of(&cfg.box_color, 0x101820);
    let text_rgb = rgb_of(&cfg.text_color, 0xffffff);
    let box_a = (cfg.box_opacity.clamp(0.0, 1.0) * 255.0) as u8;
    let mut img = RgbaImage::from_pixel(w.max(2), h.max(2), image::Rgba([box_rgb[0], box_rgb[1], box_rgb[2], box_a]));
    // Accent bar in the text colour.
    for y in 0..img.height() {
        for x in 0..(bar as u32).min(img.width()) {
            img.put_pixel(x, y, image::Rgba([text_rgb[0], text_rgb[1], text_rgb[2], 255]));
        }
    }
    let x = bar + pad;
    let ascent = bold.as_scaled(PxScale::from(title_px)).ascent();
    draw_text(&mut img, &bold, title_px, x, pad + ascent, &cfg.title, text_rgb);
    if has_sub {
        let sub_ascent = regular.as_scaled(PxScale::from(sub_px)).ascent();
        let dim = [((text_rgb[0] as u16 * 3 + box_rgb[0] as u16) / 4) as u8, ((text_rgb[1] as u16 * 3 + box_rgb[1] as u16) / 4) as u8, ((text_rgb[2] as u16 * 3 + box_rgb[2] as u16) / 4) as u8];
        draw_text(&mut img, &regular, sub_px, x, pad + title_px * 1.05 + sub_px * 0.25 + sub_ascent, &cfg.subtitle, dim);
    }
    img
}

/// A clock: an optional label over the time, in a box like a title's.
/// The box is sized for the widest digits so it doesn't twitch each second.
pub fn render_clock(cfg: &Graphic, text: &str, over: bool, canvas: (u32, u32)) -> RgbaImage {
    let (bold, regular) = fonts(&cfg.font);
    let ch = canvas.1 as f32;
    let label = cfg.title.trim();
    let time_px = (ch * 0.06).max(12.0);
    let label_px = (ch * 0.026).max(8.0);
    let pad = ch * 0.018;
    let widest: String = text.chars().map(|c| if c.is_ascii_digit() { '0' } else { c }).collect();
    let tw = text_width(&bold, time_px, &widest).max(text_width(&bold, time_px, text));
    let lw = if label.is_empty() { 0.0 } else { text_width(&regular, label_px, label) };
    let min_w = cfg.width.clamp(0.03, 1.0) * canvas.0 as f32;
    let w = (tw.max(lw) + pad * 2.0).max(min_w).ceil() as u32;
    let label_h = if label.is_empty() { 0.0 } else { label_px * 1.3 };
    let h = (pad * 2.0 + label_h + time_px * 1.0).ceil() as u32;
    let box_rgb = rgb_of(&cfg.box_color, 0x101820);
    let text_rgb = if over { [255, 92, 92] } else { rgb_of(&cfg.text_color, 0xffffff) };
    let box_a = (cfg.box_opacity.clamp(0.0, 1.0) * 255.0) as u8;
    let mut img = RgbaImage::from_pixel(w.max(2), h.max(2), image::Rgba([box_rgb[0], box_rgb[1], box_rgb[2], box_a]));
    if !label.is_empty() {
        let dim = [((text_rgb[0] as u16 * 3 + box_rgb[0] as u16) / 4) as u8, ((text_rgb[1] as u16 * 3 + box_rgb[1] as u16) / 4) as u8, ((text_rgb[2] as u16 * 3 + box_rgb[2] as u16) / 4) as u8];
        let ascent = regular.as_scaled(PxScale::from(label_px)).ascent();
        draw_text(&mut img, &regular, label_px, (w as f32 - lw) / 2.0, pad + ascent, label, dim);
    }
    let ascent = bold.as_scaled(PxScale::from(time_px)).ascent();
    let x = (w as f32 - text_width(&bold, time_px, text)) / 2.0;
    draw_text(&mut img, &bold, time_px, x, pad + label_h + ascent * 0.92, text, text_rgb);
    img
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_render_and_fade() {
        let cfg = Graphic { fade_ms: 1000, ..Graphic::title("Host", "Alex Morgan", "Presenter") };
        let img = render_title(&cfg, (1920, 1080));
        assert!(img.width() >= (0.36 * 1920.0) as u32 && img.height() > 60, "{}x{}", img.width(), img.height());
        // Text was drawn: some pixels are near white.
        assert!(img.pixels().filter(|p| p[0] > 200 && p[1] > 200).count() > 500);

        let g = Graphics::new(&[cfg.clone(), Graphic { name: "Bug".into(), ..Default::default() }], (1920, 1080), Timers::new());
        assert_eq!(g.list()[1].2.as_deref(), Some("choose an image file"));
        assert!(g.layers(clock::now()).is_empty());
        g.set_visible(0, true);
        let t = clock::now() - VIDEO_LATENCY;
        let a_half = g.layers(t + 500_000_000)[0].alpha;
        assert!((a_half - 0.5).abs() < 0.05, "{a_half}");
        assert!((g.layers(t + 2_000_000_000)[0].alpha - 1.0).abs() < 1e-6);
        // Editing the text redraws it.
        let before = g.layers(t + 2_000_000_000)[0].key;
        g.set_text(0, "Sam Lee", "Guest");
        assert_ne!(g.layers(t + 2_000_000_000)[0].key, before);
    }

    #[test]
    fn clocks_redraw_as_they_change() {
        let mut clock = Graphic::clock("Countdown", crate::config::ClockKind::Countdown, "Back in");
        clock.seconds = 120;
        clock.fade_ms = 0;
        let timers = Timers::new();
        let g = Graphics::new(&[clock.clone()], (1280, 720), timers.clone());
        assert_eq!(g.clock_text(0).unwrap().0, "2:00");
        g.set_visible(0, true);
        let t = clock::now();
        let first = g.layers(t)[0].clone();
        assert_eq!(g.layers(t)[0].key, first.key, "no change, no redraw");
        // Started: the text (and picture) change; the box stays the same size.
        timers.start("Countdown");
        timers.inner_for_tests_shift("Countdown", std::time::Duration::from_secs(3));
        let next = g.layers(t)[0].clone();
        assert_ne!(next.key, first.key);
        assert_eq!(g.clock_text(0).unwrap().0, "1:57");
        assert_eq!((next.image.width(), next.image.height()), (first.image.width(), first.image.height()));
        assert!(first.image.pixels().filter(|p| p[0] > 200 && p[1] > 200).count() > 200, "digits drawn");
    }
}
