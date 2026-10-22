//! The graphics layer: logos, watermarks, name titles, clocks, tickers and
//! web pages drawn above every scene, each with its own on/off button and
//! a way of coming on.

use crate::util::LockExt;
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
    /// The part of the picture shown (u0, v0, u1, v1).
    pub uv: [f32; 4],
}

pub const FULL_UV: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// A viewer comment to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Comment {
    pub author: String,
    pub text: String,
    /// "#RRGGBB" for the name (the chat's colour, or the platform's).
    pub color: String,
}

/// The comment on screen (or fading out).
#[derive(Default)]
struct CommentItem {
    style: crate::config::CommentsConfig,
    image: Option<Arc<RgbaImage>>,
    version: u64,
    visible: bool,
    changed_at: Ns,
    from_alpha: f32,
}

impl CommentItem {
    fn alpha_at(&self, t: Ns) -> f32 {
        let fade = self.style.fade_ms as i64 * 1_000_000;
        let target = if self.visible { 1.0 } else { 0.0 };
        if fade <= 0 || t >= self.changed_at + fade {
            return target;
        }
        if t <= self.changed_at {
            return self.from_alpha;
        }
        self.from_alpha + (target - self.from_alpha) * (t - self.changed_at) as f32 / fade as f32
    }
}

/// A ticker's text strip, drawn once and slid along.
struct TickerStrip {
    image: Arc<RgbaImage>,
    /// One pass of the text (it repeats), in strip pixels.
    cycle: f32,
    version: u64,
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
    /// Tickers: the moving text.
    strip: Option<TickerStrip>,
    /// Web pages: the page, and which of its pictures is showing.
    web: Option<super::web::WebPage>,
    web_seen: u64,
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
    comment: Mutex<CommentItem>,
}

/// Layer keys for things that aren't in the graphics list.
const COMMENT_KEY: usize = 1 << 20;
const STRIP_KEY: usize = 1 << 21;

impl Graphics {
    pub fn new(cfgs: &[Graphic], canvas: (u32, u32), timers: Arc<Timers>) -> Arc<Self> {
        let g = Arc::new(Graphics { canvas, items: Mutex::new(Vec::new()), versions: Default::default(), timers, comment: Mutex::new(CommentItem::default()) });
        g.update(cfgs);
        for it in g.items.lock_ok().iter_mut() {
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
        let mut items = self.items.lock_ok();
        let mut old: Vec<Item> = std::mem::take(&mut *items);
        for cfg in cfgs {
            let prev = old.iter().position(|o| o.cfg.name == cfg.name).map(|i| old.remove(i));
            let item = match prev {
                Some(mut p) if p.cfg == *cfg => {
                    p.cfg = cfg.clone();
                    p
                }
                Some(mut p) => {
                    let (image, error, text) = self.render(cfg);
                    let strip = self.render_strip(cfg);
                    // A page carries on unless what it shows changed.
                    let same_page = p.cfg.kind == GraphicKind::Web && p.cfg.url == cfg.url && p.cfg.page_size == cfg.page_size;
                    let (web, web_seen) = if same_page { (p.web.take(), p.web_seen) } else { (open_page(cfg), 0) };
                    let image = if same_page { p.image.take() } else { image };
                    Item { cfg: cfg.clone(), image, error, version: self.next_version(), visible: p.visible, changed_at: p.changed_at, from_alpha: p.from_alpha, text, strip, web, web_seen }
                }
                None => {
                    let (image, error, text) = self.render(cfg);
                    let strip = self.render_strip(cfg);
                    Item { cfg: cfg.clone(), image, error, version: self.next_version(), visible: false, changed_at: 0, from_alpha: 0.0, text, strip, web: open_page(cfg), web_seen: 0 }
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
            GraphicKind::Ticker => Ok(render_ticker_band(cfg, self.canvas)),
            // (Drawn by the browser as it goes.)
            GraphicKind::Web => return (None, None, text),
        };
        match r {
            Ok(img) => (Some(Arc::new(img)), None, text),
            Err(e) => (None, Some(e), text),
        }
    }

    fn render_strip(&self, cfg: &Graphic) -> Option<TickerStrip> {
        (cfg.kind == GraphicKind::Ticker).then(|| {
            let (image, cycle) = render_ticker_strip(cfg, self.canvas);
            TickerStrip { image: Arc::new(image), cycle, version: self.next_version() }
        })
    }

    /// Show a viewer comment (None: hide it).
    pub fn set_comment(&self, comment: Option<&Comment>, style: &crate::config::CommentsConfig) {
        let mut c = self.comment.lock_ok();
        let t = clock::now() - VIDEO_LATENCY;
        c.from_alpha = c.alpha_at(t);
        c.changed_at = t;
        c.style = style.clone();
        match comment {
            Some(cm) => {
                c.image = Some(Arc::new(render_comment(cm, style, self.canvas)));
                c.version = self.next_version();
                c.visible = true;
            }
            None => c.visible = false,
        }
    }

    #[cfg(test)]
    pub fn comment_showing(&self) -> bool {
        self.comment.lock_ok().visible
    }

    /// Change a ticker's text live.
    pub fn set_ticker_text(&self, index: usize, text: &str) {
        let mut items = self.items.lock_ok();
        let Some(it) = items.get_mut(index).filter(|i| i.cfg.kind == GraphicKind::Ticker) else { return };
        if it.cfg.title == text {
            return;
        }
        it.cfg.title = text.to_string();
        let (image, cycle) = render_ticker_strip(&it.cfg, self.canvas);
        it.strip = Some(TickerStrip { image: Arc::new(image), cycle, version: self.versions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1 });
    }

    /// What a clock graphic shows now.
    pub fn clock_text(&self, index: usize) -> Option<(String, bool)> {
        let items = self.items.lock_ok();
        let it = items.get(index).filter(|i| i.cfg.kind == GraphicKind::Clock)?;
        Some(clock_text(&it.cfg, &self.timers, chrono::Local::now()))
    }

    /// (name, on, problem) for each graphic.
    pub fn list(&self) -> Vec<(String, bool, Option<String>)> {
        self.items.lock_ok().iter().map(|i| (i.cfg.name.clone(), i.visible, i.error.clone().or_else(|| i.web.as_ref().and_then(|w| w.problem())))).collect()
    }

    /// Load a web page again (to restart its animation or pick up changes).
    pub fn reload(&self, index: usize) {
        if let Some(w) = self.items.lock_ok().get(index).and_then(|i| i.web.as_ref()) {
            w.reload();
        }
    }

    pub fn is_visible(&self, index: usize) -> bool {
        self.items.lock_ok().get(index).is_some_and(|i| i.visible)
    }

    /// Show or hide a graphic (fading as set). Timed like a scene change,
    /// so it lands on the program frame the operator saw.
    pub fn set_visible(&self, index: usize, on: bool) {
        let mut items = self.items.lock_ok();
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
        let mut items = self.items.lock_ok();
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
        let mut items = self.items.lock_ok();
        let mut out = Vec::new();
        let wall = chrono::Local::now() + chrono::Duration::nanoseconds(t - clock::now());
        for (i, it) in items.iter_mut().enumerate() {
            let alpha = it.alpha_at(t);
            // A web page's newest picture.
            if let Some((v, img)) = it.web.as_ref().and_then(|w| w.picture()).filter(|(v, _)| *v != it.web_seen) {
                it.web_seen = v;
                it.image = Some(img);
                it.version = self.versions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            }
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
            out.push(GraphicLayer { key: (i, it.version), image: img.clone(), rect: [it.cfg.x, it.cfg.y, w, h], alpha, stream_only: false, uv: FULL_UV });
            // A ticker's text slides along inside its band, right of the label.
            if let Some(strip) = &it.strip {
                let band_px = w * cw;
                let label_px = ticker_label_width(&it.cfg, self.canvas);
                let shown = (band_px - label_px).max(1.0);
                let sw = strip.image.width() as f32;
                let speed = it.cfg.speed.clamp(0.5, 60.0) / 60.0 * cw; // pixels a second
                let phase = ((t as f64 / 1e9 * speed as f64) % strip.cycle.max(1.0) as f64) as f32;
                let u0 = phase / sw;
                let u1 = (phase + shown) / sw;
                let x = it.cfg.x + label_px / cw;
                out.push(GraphicLayer { key: (STRIP_KEY + i, strip.version), image: strip.image.clone(), rect: [x, it.cfg.y, shown / cw, h], alpha, stream_only: false, uv: [u0, 0.0, u1, 1.0] });
            }
        }
        drop(items);
        let c = self.comment.lock_ok();
        let alpha = c.alpha_at(t);
        if let (Some(img), true) = (&c.image, alpha > 0.001) {
            let w = c.style.width.clamp(0.05, 1.0);
            let h = w * cw / ch * img.height() as f32 / img.width() as f32;
            out.push(GraphicLayer { key: (COMMENT_KEY, c.version), image: img.clone(), rect: [c.style.x, c.style.y, w, h], alpha, stream_only: false, uv: FULL_UV });
        }
        out
    }
}

/// A web graphic's page, opened in the background.
fn open_page(cfg: &Graphic) -> Option<super::web::WebPage> {
    (cfg.kind == GraphicKind::Web).then(|| super::web::WebPage::open(&cfg.url, (cfg.page_size[0], cfg.page_size[1]), Arc::new(|| {})))
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

/// Text sizes of a ticker band.
fn ticker_sizes(canvas: (u32, u32)) -> (f32, f32) {
    let ch = canvas.1 as f32;
    let text_px = (ch * 0.036).max(9.0);
    (text_px, (text_px * 1.9).ceil())
}

pub fn ticker_label_width(cfg: &Graphic, canvas: (u32, u32)) -> f32 {
    let label = cfg.subtitle.trim();
    if label.is_empty() {
        return 0.0;
    }
    let (bold, _) = fonts(&cfg.font);
    let (text_px, _) = ticker_sizes(canvas);
    (text_width(&bold, text_px * 0.9, label) + text_px * 1.6).ceil()
}

/// A ticker's band: the box, and its label at the left in the text
/// colour's box.
pub fn render_ticker_band(cfg: &Graphic, canvas: (u32, u32)) -> RgbaImage {
    let (bold, _) = fonts(&cfg.font);
    let (text_px, band_h) = ticker_sizes(canvas);
    let w = (cfg.width.clamp(0.05, 1.0) * canvas.0 as f32).ceil() as u32;
    let box_rgb = rgb_of(&cfg.box_color, 0x101820);
    let text_rgb = rgb_of(&cfg.text_color, 0xffffff);
    let box_a = (cfg.box_opacity.clamp(0.0, 1.0) * 255.0) as u8;
    let mut img = RgbaImage::from_pixel(w.max(2), band_h as u32, image::Rgba([box_rgb[0], box_rgb[1], box_rgb[2], box_a]));
    let label = cfg.subtitle.trim();
    if !label.is_empty() {
        let lw = ticker_label_width(cfg, canvas) as i32;
        super::text::fill_rect(&mut img, 0, 0, lw, band_h as i32, [text_rgb[0], text_rgb[1], text_rgb[2], 255]);
        let size = text_px * 0.9;
        let tw = text_width(&bold, size, label);
        let ascent = bold.as_scaled(PxScale::from(size)).ascent();
        draw_text(&mut img, &bold, size, (lw as f32 - tw) / 2.0, (band_h + ascent * 0.72) / 2.0, &label.to_uppercase(), box_rgb);
    }
    img
}

/// The moving text: every item once, then again (enough to fill the band
/// at any point in the cycle). Returns the strip and one cycle's width.
pub fn render_ticker_strip(cfg: &Graphic, canvas: (u32, u32)) -> (RgbaImage, f32) {
    let (_, regular) = fonts(&cfg.font);
    let (text_px, band_h) = ticker_sizes(canvas);
    let items: Vec<&str> = cfg.title.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let joined = if items.is_empty() { " ".to_string() } else { format!("{}     •     ", items.join("     •     ")) };
    let cycle = text_width(&regular, text_px, &joined).max(1.0).ceil();
    let band_w = cfg.width.clamp(0.05, 1.0) * canvas.0 as f32;
    let copies = ((band_w / cycle).ceil() as usize + 1).max(2);
    let w = ((cycle * copies as f32).ceil() as u32).min(16_384);
    let text_rgb = rgb_of(&cfg.text_color, 0xffffff);
    let mut img = RgbaImage::new(w.max(2), band_h as u32);
    let ascent = regular.as_scaled(PxScale::from(text_px)).ascent();
    for k in 0..copies {
        draw_text(&mut img, &regular, text_px, k as f32 * cycle, (band_h + ascent * 0.72) / 2.0, &joined, text_rgb);
    }
    (img, cycle)
}

/// A viewer comment: the name in its colour over the message, in a box.
pub fn render_comment(c: &Comment, style: &crate::config::CommentsConfig, canvas: (u32, u32)) -> RgbaImage {
    let (bold, regular) = fonts(Path::new(""));
    let ch = canvas.1 as f32;
    let name_px = (ch * 0.03).max(8.0);
    let text_px = (ch * 0.036).max(9.0);
    let pad = ch * 0.02;
    let w = (style.width.clamp(0.05, 1.0) * canvas.0 as f32).ceil();
    let lines = super::text::wrap(&regular, text_px, &c.text, w - pad * 2.0);
    let lines: Vec<String> = lines.into_iter().take(4).collect();
    let h = (pad * 2.0 + name_px * 1.35 + lines.len().max(1) as f32 * text_px * 1.25).ceil() as u32;
    let box_rgb = rgb_of(&style.box_color, 0x1b1f2a);
    let text_rgb = rgb_of(&style.text_color, 0xffffff);
    let box_a = (style.box_opacity.clamp(0.0, 1.0) * 255.0) as u8;
    let mut img = RgbaImage::from_pixel(w as u32, h.max(2), image::Rgba([box_rgb[0], box_rgb[1], box_rgb[2], box_a]));
    let name_rgb = rgb_of(&c.color, 0x9b6bff);
    let bar = (ch * 0.006).max(2.0) as i32;
    super::text::fill_rect(&mut img, 0, 0, bar, h as i32, [name_rgb[0], name_rgb[1], name_rgb[2], 255]);
    let name = super::text::ellipsize(&bold, name_px, &c.author, w - pad * 2.0);
    let a = bold.as_scaled(PxScale::from(name_px)).ascent();
    draw_text(&mut img, &bold, name_px, pad, pad + a, &name, name_rgb);
    let a2 = regular.as_scaled(PxScale::from(text_px)).ascent();
    for (k, l) in lines.iter().enumerate() {
        draw_text(&mut img, &regular, text_px, pad, pad + name_px * 1.35 + k as f32 * text_px * 1.25 + a2, l, text_rgb);
    }
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

    #[test]
    fn tickers_scroll_and_comments_show() {
        let mut t = Graphic::ticker("Ticker", "Next week: live from the studio\nFollow us @show", "Coming up");
        t.fade_ms = 0;
        t.speed = 6.0;
        let g = Graphics::new(&[t], (1280, 720), Timers::new());
        g.set_visible(0, true);
        let now = clock::now();
        let a = g.layers(now);
        assert_eq!(a.len(), 2, "the band and its moving text");
        let (band, text) = (&a[0], &a[1]);
        assert_eq!(band.uv, FULL_UV);
        assert!(text.rect[0] > 0.05, "text starts right of the label: {}", text.rect[0]);
        assert!((text.rect[0] + text.rect[2] - 1.0).abs() < 1e-3, "and runs to the band's end");
        // Half a second later it has moved (and the picture is the same).
        let b = g.layers(now + 500_000_000);
        assert_eq!(b[1].key, text.key, "no redraw to move");
        let moved_px = (b[1].uv[0] - text.uv[0]).rem_euclid(1.0) * text.image.width() as f32;
        assert!((moved_px - 64.0).abs() < 2.0 || moved_px > text.image.width() as f32 / 2.0, "6 widths a minute is 64 px in half a second: {moved_px}");
        assert!(text.uv[2] <= 1.0 + 1e-4, "never past the strip's end");
        // New text redraws the strip.
        g.set_ticker_text(0, "Breaking");
        assert_ne!(g.layers(now)[1].key, text.key);

        // A comment fades in, then out.
        let style = crate::config::CommentsConfig { fade_ms: 0, ..Default::default() };
        g.set_comment(Some(&Comment { author: "viewer42".into(), text: "Loving the show! Where did you get that microphone?".into(), color: "#ff5500".into() }), &style);
        assert!(g.comment_showing());
        let l = g.layers(clock::now());
        let c = l.iter().find(|l| l.key.0 == COMMENT_KEY).expect("the comment");
        assert!(c.image.pixels().filter(|p| p[0] > 200 && p[1] < 120).count() > 50, "the name in its colour");
        g.set_comment(None, &style);
        assert!(!g.comment_showing());
        assert!(g.layers(clock::now()).iter().all(|l| l.key.0 != COMMENT_KEY));
    }
}
