//! Generated test pictures and tones (for trying things without hardware,
//! and for automated tests).
//!
//! Both include a sync marker on every whole second of the host clock: a
//! white square in the picture's top-left corner and a short 1 kHz beep in
//! the audio. Recording a test source and checking where the flash and the
//! beep land measures audio/video sync exactly.

use super::{Ns, NS_PER_SEC};

pub const PATTERNS: &[&str] = &["smpte", "ball", "pinwheel", "spokes", "gradient", "colors", "snow", "circular"];

/// RGB (0..1) to BT.709 video-range YCbCr.
pub fn rgb_to_yuv(r: f32, g: f32, b: f32) -> (u8, u8, u8) {
    let y = 16.0 + 219.0 * (0.2126 * r + 0.7152 * g + 0.0722 * b);
    let cb = 128.0 + 224.0 * (-0.1146 * r - 0.3854 * g + 0.5 * b);
    let cr = 128.0 + 224.0 * (0.5 * r - 0.4542 * g - 0.0458 * b);
    (y.round().clamp(0.0, 255.0) as u8, cb.round().clamp(0.0, 255.0) as u8, cr.round().clamp(0.0, 255.0) as u8)
}

pub struct Nv12Image {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub uv: Vec<u8>,
}

impl Nv12Image {
    pub fn new(width: usize, height: usize) -> Self {
        Nv12Image { width, height, y: vec![16; width * height], uv: vec![128; width * height / 2] }
    }

    #[inline]
    fn put(&mut self, x: usize, y: usize, c: (u8, u8, u8)) {
        self.y[y * self.width + x] = c.0;
        if x.is_multiple_of(2) && y.is_multiple_of(2) {
            let i = (y / 2) * self.width + x;
            self.uv[i] = c.1;
            self.uv[i + 1] = c.2;
        }
    }

    fn fill_rect(&mut self, x0: usize, y0: usize, w: usize, h: usize, c: (u8, u8, u8)) {
        let (x1, y1) = ((x0 + w).min(self.width), (y0 + h).min(self.height));
        for y in y0..y1 {
            self.y[y * self.width + x0..y * self.width + x1].fill(c.0);
        }
        for y in (y0 / 2)..y1.div_ceil(2).min(self.height / 2) {
            for x in (x0 / 2)..x1.div_ceil(2).min(self.width / 2) {
                let i = y * self.width + x * 2;
                self.uv[i] = c.1;
                self.uv[i + 1] = c.2;
            }
        }
    }
}

fn hue(h: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(1.0) * 6.0;
    let x = 1.0 - (h % 2.0 - 1.0).abs();
    match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    }
}

pub struct TestPattern {
    pattern: String,
    base: Option<Nv12Image>,
    width: usize,
    height: usize,
    seed: u64,
}

impl TestPattern {
    pub fn new(pattern: &str, width: u32, height: u32) -> Self {
        // "clap" (for lip-sync testing) isn't offered as a picture.
        let pattern = if PATTERNS.contains(&pattern) || pattern == "clap" || pattern == "people" { pattern.to_string() } else { "smpte".to_string() };
        TestPattern { pattern, base: None, width: width as usize, height: height as usize, seed: 0x9E37_79B9_7F4A_7C15 }
    }

    fn bars(w: usize, h: usize) -> Nv12Image {
        let mut img = Nv12Image::new(w, h);
        let top = [(0.75, 0.75, 0.75), (0.75, 0.75, 0.0), (0.0, 0.75, 0.75), (0.0, 0.75, 0.0), (0.75, 0.0, 0.75), (0.75, 0.0, 0.0), (0.0, 0.0, 0.75)];
        let bw = w.div_ceil(7);
        for (i, c) in top.iter().enumerate() {
            img.fill_rect(i * bw, 0, bw, h * 2 / 3, rgb_to_yuv(c.0, c.1, c.2));
        }
        let mid = [(0.0, 0.0, 0.75), (0.0, 0.0, 0.0), (0.75, 0.0, 0.75), (0.0, 0.0, 0.0), (0.0, 0.75, 0.75), (0.0, 0.0, 0.0), (0.75, 0.75, 0.75)];
        for (i, c) in mid.iter().enumerate() {
            img.fill_rect(i * bw, h * 2 / 3, bw, h / 12, rgb_to_yuv(c.0, c.1, c.2));
        }
        let low = [(0.0, 0.13, 0.3), (1.0, 1.0, 1.0), (0.2, 0.0, 0.42), (0.0, 0.0, 0.0)];
        let lw = w * 5 / 7 / 4;
        for (i, c) in low.iter().enumerate() {
            img.fill_rect(i * lw, h * 3 / 4, lw, h - h * 3 / 4, rgb_to_yuv(c.0, c.1, c.2));
        }
        img.fill_rect(lw * 4, h * 3 / 4, w - lw * 4, h - h * 3 / 4, rgb_to_yuv(0.04, 0.04, 0.04));
        img
    }

    fn noise(&mut self) -> u64 {
        // xorshift64*
        self.seed ^= self.seed >> 12;
        self.seed ^= self.seed << 25;
        self.seed ^= self.seed >> 27;
        self.seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Render the picture for time `pts` (host clock).
    pub fn render(&mut self, pts: Ns, fps: u32) -> Nv12Image {
        let (w, h) = (self.width, self.height);
        let t = pts as f64 / 1e9;
        let mut img = match self.pattern.as_str() {
            "smpte" => {
                if self.base.is_none() {
                    self.base = Some(Self::bars(w, h));
                }
                let b = self.base.as_ref().unwrap();
                Nv12Image { width: w, height: h, y: b.y.clone(), uv: b.uv.clone() }
            }
            "ball" => {
                let mut img = Nv12Image::new(w, h);
                let r = (h / 10).max(4) as f64;
                let cx = (w as f64 / 2.0) + (w as f64 / 2.0 - r) * (t * 0.9).sin();
                let cy = (h as f64 / 2.0) + (h as f64 / 2.0 - r) * (t * 1.3).cos();
                let white = rgb_to_yuv(1.0, 1.0, 1.0);
                let (x0, x1) = ((cx - r).max(0.0) as usize, ((cx + r) as usize).min(w - 1));
                let (y0, y1) = ((cy - r).max(0.0) as usize, ((cy + r) as usize).min(h - 1));
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let (dx, dy) = (x as f64 - cx, y as f64 - cy);
                        if dx * dx + dy * dy <= r * r {
                            img.put(x, y, white);
                        }
                    }
                }
                img
            }
            "pinwheel" | "spokes" | "circular" => {
                let mut img = Nv12Image::new(w, h);
                let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
                let spin = if self.pattern == "spokes" { t * 0.2 } else { t * 0.6 };
                let wedges = if self.pattern == "spokes" { 48.0 } else { 24.0 };
                for y in 0..h {
                    for x in 0..w {
                        let (dx, dy) = (x as f64 - cx, y as f64 - cy);
                        let on = if self.pattern == "circular" {
                            ((dx * dx + dy * dy).sqrt() / (h as f64 / 24.0) - t * 2.0).rem_euclid(2.0) < 1.0
                        } else {
                            ((dy.atan2(dx) / std::f64::consts::TAU + spin) * wedges).rem_euclid(2.0) < 1.0
                        };
                        if on {
                            img.y[y * w + x] = 235;
                        }
                    }
                }
                img
            }
            "gradient" | "colors" => {
                let mut img = Nv12Image::new(w, h);
                for y in (0..h).step_by(2) {
                    for x in (0..w).step_by(2) {
                        let (r, g, b) = if self.pattern == "colors" {
                            hue(x as f32 / w as f32 + t as f32 * 0.1)
                        } else {
                            let v = ((x as f64 / w as f64) + t * 0.1).rem_euclid(1.0) as f32;
                            (v, v, v)
                        };
                        let c = rgb_to_yuv(r, g * if self.pattern == "colors" { 1.0 - y as f32 / h as f32 * 0.5 } else { 1.0 }, b);
                        for (ox, oy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                            if x + ox < w && y + oy < h {
                                img.put(x + ox, y + oy, c);
                            }
                        }
                    }
                }
                img
            }
            "people" => {
                // Two people side by side, swaying a little: for testing
                // auto-framing and background removal without a camera.
                let mut img = Nv12Image::new(w, h);
                img.fill_rect(0, 0, w, h, rgb_to_yuv(BACKDROP.0, BACKDROP.1, BACKDROP.2));
                for (cx, cy, r) in people_at(t, w, h) {
                    let (i, shirt) = if cx < w as f64 / 2.0 { (0, SHIRTS[0]) } else { (1, SHIRTS[1]) };
                    let _ = i;
                    let bw = (r * 2.6) as usize;
                    let top = (cy + r * 1.1) as usize;
                    img.fill_rect((cx as usize).saturating_sub(bw / 2), top.min(h), bw, h.saturating_sub(top), rgb_to_yuv(shirt.0, shirt.1, shirt.2));
                    let skin = rgb_to_yuv(SKIN.0, SKIN.1, SKIN.2);
                    let (x0, x1) = ((cx - r).max(0.0) as usize, ((cx + r) as usize).min(w - 1));
                    let (y0, y1) = ((cy - r * 1.2).max(0.0) as usize, ((cy + r * 1.2) as usize).min(h - 1));
                    for y in y0..=y1 {
                        for x in x0..=x1 {
                            let (dx, dy) = ((x as f64 - cx) / r, (y as f64 - cy) / (r * 1.2));
                            if dx * dx + dy * dy <= 1.0 {
                                img.put(x, y, skin);
                            }
                        }
                    }
                }
                return img;
            }
            "clap" => {
                // Two "hands" that come together fast on each whole second
                // (when the beep sounds), stay together, then part slowly:
                // for testing lip-sync measurement.
                let mut img = Nv12Image::new(w, h);
                img.fill_rect(0, 0, w, h, rgb_to_yuv(0.12, 0.12, 0.14));
                let p = t.rem_euclid(1.0);
                let open = if p >= 0.9 {
                    1.0 - (p - 0.9) / 0.1
                } else if p < 0.3 {
                    0.0
                } else {
                    (p - 0.3) / 0.6
                };
                let gap = (open * 0.6 * w as f64) as usize;
                let hand = (w / 10).max(2);
                let (cx, top, hh) = (w / 2, h / 4, h / 2);
                let skin = rgb_to_yuv(0.95, 0.8, 0.7);
                img.fill_rect(cx.saturating_sub(gap / 2 + hand), top, hand, hh, skin);
                img.fill_rect((cx + gap / 2).min(w - hand), top, hand, hh, skin);
                return img;
            }
            _ => {
                // snow
                let mut img = Nv12Image::new(w, h);
                for chunk in img.y.chunks_mut(8) {
                    let n = self.noise().to_le_bytes();
                    for (d, s) in chunk.iter_mut().zip(n) {
                        *d = 16 + (s as u16 * 219 / 255) as u8;
                    }
                }
                img
            }
        };
        // A strip of noise along the bottom makes every frame unique.
        let strip = (h / 16).max(2) & !1;
        for y in h - strip..h {
            for x in (w * 3 / 4..w).step_by(8) {
                let n = self.noise().to_le_bytes();
                for (i, b) in n.iter().enumerate() {
                    if x + i < w {
                        img.y[y * w + x + i] = 16 + (*b as u16 * 219 / 255) as u8;
                    }
                }
            }
        }
        // Sync flash on each whole second.
        let frame = NS_PER_SEC / fps.max(1) as i64;
        if pts.rem_euclid(NS_PER_SEC) < frame {
            let s = (h / 8).max(8);
            img.fill_rect(0, 0, s, s, rgb_to_yuv(1.0, 1.0, 1.0));
        }
        img
    }
}

/// Colours of the "people" test picture (RGB 0..1).
pub const SKIN: (f32, f32, f32) = (0.96, 0.76, 0.62);
pub const SHIRTS: [(f32, f32, f32); 2] = [(0.15, 0.35, 0.85), (0.85, 0.25, 0.2)];
pub const BACKDROP: (f32, f32, f32) = (0.22, 0.25, 0.28);

/// Where the heads of the "people" test picture are at time `t`
/// (seconds): (centre x, centre y, radius) in pixels.
pub fn people_at(t: f64, w: usize, h: usize) -> Vec<(f64, f64, f64)> {
    let (w, h) = (w as f64, h as f64);
    let r = h * 0.08;
    vec![
        (w * (0.3 + 0.04 * (t * 0.7).sin()), h * (0.38 + 0.01 * (t * 1.3).sin()), r),
        (w * (0.72 + 0.03 * (t * 0.5 + 1.0).sin()), h * (0.36 + 0.01 * (t * 1.1).cos()), r * 0.95),
    ]
}

/// Sine tone with a beep on every whole second of the host clock.
pub struct TestTone {
    pub rate: u32,
    pub channels: u16,
    freq: f64,
}

impl TestTone {
    pub fn new(rate: u32, channels: u16, freq: f64) -> Self {
        TestTone { rate, channels, freq }
    }

    /// Interleaved samples for `frames` frames starting at host time `pts`.
    pub fn render(&self, pts: Ns, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * self.channels as usize);
        for i in 0..frames {
            let t_ns = pts + i as i64 * NS_PER_SEC / self.rate as i64;
            let t = t_ns as f64 / 1e9;
            let beep = t_ns.rem_euclid(NS_PER_SEC) < 20_000_000;
            for c in 0..self.channels {
                let v = if beep {
                    0.8 * (std::f64::consts::TAU * 1000.0 * t).sin()
                } else {
                    0.2 * (std::f64::consts::TAU * self.freq * (1.0 + c as f64 * 0.25) * t).sin()
                };
                out.push(v as f32);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patterns_render_and_flash() {
        for p in PATTERNS {
            let mut tp = TestPattern::new(p, 64, 36);
            let img = tp.render(2 * NS_PER_SEC, 30); // on a whole second
            assert_eq!(img.y.len(), 64 * 36);
            assert_eq!(img.uv.len(), 64 * 18);
            assert!(img.y[0] > 200, "{p}: flash missing");
            let img = tp.render(2 * NS_PER_SEC + NS_PER_SEC / 2, 30);
            assert!(img.y[0] < 200 || *p == "smpte" || *p == "gradient" || *p == "colors", "{p}: flash stuck");
        }
    }

    #[test]
    fn tone_beeps_on_the_second() {
        let tone = TestTone::new(48000, 2, 440.0);
        let s = tone.render(NS_PER_SEC - 1_000_000, 4800);
        let early: f32 = s[..48 * 2].iter().map(|v| v.abs()).fold(0.0, f32::max);
        let beep: f32 = s[96 * 2..120 * 2 * 4].iter().map(|v| v.abs()).fold(0.0, f32::max);
        assert!(early <= 0.21 && beep > 0.5, "{early} {beep}");
    }
}
