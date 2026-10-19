//! Media building blocks that don't depend on any platform: frame and packet
//! types, the clock, bitstream helpers and the file/stream formats.

pub mod audiofile;
pub mod clock;
pub mod cube;
pub mod demux;
pub mod flv;
pub mod h264;
pub mod mirror;
pub mod mp4;
pub mod testsrc;
pub mod ts;
pub mod tsdemux;
pub mod wav;

use std::sync::Arc;

/// Nanoseconds on the host clock (see [`clock::now`]).
pub type Ns = i64;

pub const NS_PER_SEC: i64 = 1_000_000_000;

/// An NV12 picture (BT.709, video range) from a BGRA one (`pitch` bytes
/// per row), scaled to `w`×`h` (bilinear). Screen captures on Windows.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn bgra_to_nv12(src: &[u8], pitch: usize, sw: usize, sh: usize, w: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
    let (w, h) = (w.max(2) & !1, h.max(2) & !1);
    let mut y = vec![16u8; w * h];
    let mut uv = vec![128u8; w * h / 2];
    if sw == 0 || sh == 0 || pitch < sw * 4 || src.len() < pitch * (sh - 1) + sw * 4 {
        return (y, uv);
    }
    let (fx, fy) = (sw as f32 / w as f32, sh as f32 / h as f32);
    let same = sw == w && sh == h;
    let px = |x: usize, y: usize| -> [f32; 3] {
        let i = y * pitch + x * 4;
        [src[i + 2] as f32, src[i + 1] as f32, src[i] as f32]
    };
    let sample = |ox: usize, oy: usize| -> [f32; 3] {
        if same {
            return px(ox, oy);
        }
        let x = ((ox as f32 + 0.5) * fx - 0.5).clamp(0.0, (sw - 1) as f32);
        let yy = ((oy as f32 + 0.5) * fy - 0.5).clamp(0.0, (sh - 1) as f32);
        let (x0, y0) = (x as usize, yy as usize);
        let (x1, y1) = ((x0 + 1).min(sw - 1), (y0 + 1).min(sh - 1));
        let (ax, ay) = (x - x0 as f32, yy - y0 as f32);
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        let mut o = [0.0; 3];
        for k in 0..3 {
            o[k] = (a[k] * (1.0 - ax) + b[k] * ax) * (1.0 - ay) + (c[k] * (1.0 - ax) + d[k] * ax) * ay;
        }
        o
    };
    let luma = |c: [f32; 3]| (16.0 + (0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]) * (219.0 / 255.0) + 0.5) as u8;
    let rows_per = 2 * (h / 2).div_ceil(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 4)).max(1);
    std::thread::scope(|s| {
        for (part, (ys, cs)) in y.chunks_mut(rows_per * w).zip(uv.chunks_mut(rows_per / 2 * w)).enumerate() {
            let (sample, luma) = (&sample, &luma);
            s.spawn(move || {
                let first = part * rows_per;
                for (r2, crow) in cs.chunks_exact_mut(w).enumerate() {
                    let oy = first + r2 * 2;
                    for cx in 0..w / 2 {
                        let mut sum = [0.0f32; 3];
                        for (dy, dx) in [(0, 0), (0, 1), (1, 0), (1, 1)] {
                            let c = sample(cx * 2 + dx, oy + dy);
                            ys[(r2 * 2 + dy) * w + cx * 2 + dx] = luma(c);
                            for k in 0..3 {
                                sum[k] += c[k] * 0.25;
                            }
                        }
                        let cb = 128.0 + (-0.1146 * sum[0] - 0.3854 * sum[1] + 0.5 * sum[2]) * (224.0 / 255.0);
                        let cr = 128.0 + (0.5 * sum[0] - 0.4542 * sum[1] - 0.0458 * sum[2]) * (224.0 / 255.0);
                        crow[cx * 2] = (cb + 0.5).clamp(16.0, 240.0) as u8;
                        crow[cx * 2 + 1] = (cr + 0.5).clamp(16.0, 240.0) as u8;
                    }
                }
            });
        }
    });
    (y, uv)
}

/// Borrowed view of an NV12 picture (Y plane + interleaved CbCr plane).
pub struct Planes<'a> {
    pub y: &'a [u8],
    pub y_stride: usize,
    pub uv: &'a [u8],
    pub uv_stride: usize,
}

/// Pixel storage of a video frame. Everything is NV12, video range, BT.709.
pub enum Pixels {
    /// Ordinary memory.
    Nv12 { y: Vec<u8>, y_stride: usize, uv: Vec<u8>, uv_stride: usize },
    /// A CoreVideo pixel buffer straight from the camera (no copy).
    #[cfg(target_os = "macos")]
    PixelBuffer(crate::platform::macos::pixel_buffer::PixelBuffer),
}

impl Pixels {
    /// Run `f` with access to the planes (locks/unlocks platform buffers).
    pub fn with_planes<R>(&self, f: impl FnOnce(Planes<'_>) -> R) -> R {
        match self {
            Pixels::Nv12 { y, y_stride, uv, uv_stride } => {
                f(Planes { y, y_stride: *y_stride, uv, uv_stride: *uv_stride })
            }
            #[cfg(target_os = "macos")]
            Pixels::PixelBuffer(pb) => pb.with_planes(f),
        }
    }
}

/// A captured or composited video frame. Cheap to clone.
#[derive(Clone)]
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    /// Capture (or render) time on the host clock.
    pub pts: Ns,
    pub pixels: Arc<Pixels>,
    /// Increments for every new frame from a source (for change detection).
    pub serial: u64,
}

impl VideoFrame {
    pub fn new_nv12(width: u32, height: u32, pts: Ns, y: Vec<u8>, uv: Vec<u8>, serial: u64) -> Self {
        VideoFrame {
            width,
            height,
            pts,
            pixels: Arc::new(Pixels::Nv12 { y, y_stride: width as usize, uv, uv_stride: width as usize }),
            serial,
        }
    }

    /// A copy in ordinary memory (releases platform buffers).
    pub fn detached(&self) -> VideoFrame {
        match &*self.pixels {
            Pixels::Nv12 { .. } => self.clone(),
            #[allow(unreachable_patterns)]
            _ => {
                let (w, h) = (self.width as usize, self.height as usize);
                let (mut y, mut uv) = (vec![0u8; w * h], vec![0u8; w * h / 2]);
                self.pixels.with_planes(|p| {
                    for r in 0..h {
                        y[r * w..r * w + w].copy_from_slice(&p.y[r * p.y_stride..r * p.y_stride + w]);
                    }
                    for r in 0..h / 2 {
                        uv[r * w..r * w + w].copy_from_slice(&p.uv[r * p.uv_stride..r * p.uv_stride + w]);
                    }
                });
                VideoFrame::new_nv12(self.width, self.height, self.pts, y, uv, self.serial)
            }
        }
    }

    /// A copy at another size (bilinear), for sources whose pictures don't
    /// match their configured size.
    pub fn scaled(&self, width: u32, height: u32) -> VideoFrame {
        let (ow, oh) = (width.max(2) & !1, height.max(2) & !1);
        if (ow, oh) == (self.width, self.height) {
            return self.detached();
        }
        self.crop_scaled([0.0, 0.0, self.width as f32, self.height as f32], ow, oh)
    }

    /// Part of the picture (`rect`: x, y, width, height in pixels) scaled
    /// to `width` × `height` (bilinear). Large pictures are done on a few
    /// threads at once.
    pub fn crop_scaled(&self, rect: [f32; 4], width: u32, height: u32) -> VideoFrame {
        let (ow, oh) = ((width.max(2) & !1) as usize, (height.max(2) & !1) as usize);
        let (w, h) = (self.width.max(2) as usize, self.height.max(2) as usize);
        let mut y = vec![0u8; ow * oh];
        let mut uv = vec![0u8; ow * oh / 2];
        // For each output column/row: the two source pixels and the weight
        // of the second (0-256).
        fn taps(start: f32, len: f32, out: usize, size: usize) -> Vec<(usize, usize, u32)> {
            let step = len / out as f32;
            (0..out)
                .map(|o| {
                    let x = (start + (o as f32 + 0.5) * step - 0.5).clamp(0.0, (size - 1) as f32);
                    let x0 = x as usize;
                    (x0, (x0 + 1).min(size - 1), ((x - x0 as f32) * 256.0).round() as u32)
                })
                .collect()
        }
        let (yx, yy) = (taps(rect[0], rect[2], ow, w), taps(rect[1], rect[3], oh, h));
        let (cx, cy) = (taps(rect[0] / 2.0, rect[2] / 2.0, ow / 2, w / 2), taps(rect[1] / 2.0, rect[3] / 2.0, oh / 2, h / 2));
        let threads = if ow * oh >= 640 * 360 { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 4) } else { 1 };
        let chroma_rows = (oh / 2).div_ceil(threads).max(1);
        self.pixels.with_planes(|p| {
            let p = &p;
            let (yx, yy, cx, cy) = (&yx, &yy, &cx, &cy);
            let work = move |part: usize, ys: &mut [u8], cs: &mut [u8]| {
                let first = part * chroma_rows;
                for (r, out) in ys.chunks_exact_mut(ow).enumerate() {
                    let (y0, y1, fy) = yy[first * 2 + r];
                    let (a, b) = (&p.y[y0 * p.y_stride..], &p.y[y1 * p.y_stride..]);
                    for (o, &(x0, x1, fx)) in out.iter_mut().zip(yx.iter()) {
                        let top = a[x0] as u32 * (256 - fx) + a[x1] as u32 * fx;
                        let bot = b[x0] as u32 * (256 - fx) + b[x1] as u32 * fx;
                        *o = ((top * (256 - fy) + bot * fy + 32768) >> 16) as u8;
                    }
                }
                for (r, out) in cs.chunks_exact_mut(ow).enumerate() {
                    let (y0, y1, fy) = cy[first + r];
                    let (a, b) = (&p.uv[y0 * p.uv_stride..], &p.uv[y1 * p.uv_stride..]);
                    for (o, &(x0, x1, fx)) in out.chunks_exact_mut(2).zip(cx.iter()) {
                        for c in 0..2 {
                            let top = a[x0 * 2 + c] as u32 * (256 - fx) + a[x1 * 2 + c] as u32 * fx;
                            let bot = b[x0 * 2 + c] as u32 * (256 - fx) + b[x1 * 2 + c] as u32 * fx;
                            o[c] = ((top * (256 - fy) + bot * fy + 32768) >> 16) as u8;
                        }
                    }
                }
            };
            if threads == 1 {
                work(0, &mut y, &mut uv);
            } else {
                std::thread::scope(|s| {
                    for (part, (ys, cs)) in y.chunks_mut(chroma_rows * 2 * ow).zip(uv.chunks_mut(chroma_rows * ow)).enumerate() {
                        s.spawn(move || work(part, ys, cs));
                    }
                });
            }
        });
        VideoFrame::new_nv12(ow as u32, oh as u32, self.pts, y, uv, self.serial)
    }

    /// A smaller copy (box filtered by a whole factor), for proxy files.
    pub fn downscaled(&self, factor: u32) -> VideoFrame {
        let k = factor.max(1) as usize;
        if k == 1 {
            return self.detached();
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let (ow, oh) = ((w / k) & !1, (h / k) & !1);
        let (ow, oh) = (ow.max(2), oh.max(2));
        let mut y = vec![0u8; ow * oh];
        let mut uv = vec![0u8; ow * oh / 2];
        let area = (k * k) as u32;
        self.pixels.with_planes(|p| {
            for oy in 0..oh {
                for ox in 0..ow {
                    let mut sum = 0u32;
                    for dy in 0..k {
                        let row = &p.y[(oy * k + dy) * p.y_stride..];
                        for dx in 0..k {
                            sum += row[ox * k + dx] as u32;
                        }
                    }
                    y[oy * ow + ox] = (sum / area) as u8;
                }
            }
            for oy in 0..oh / 2 {
                for ox in 0..ow / 2 {
                    let (mut su, mut sv) = (0u32, 0u32);
                    for dy in 0..k {
                        let row = &p.uv[((oy * k + dy).min(h / 2 - 1)) * p.uv_stride..];
                        for dx in 0..k {
                            let c = (ox * k + dx).min(w / 2 - 1) * 2;
                            su += row[c] as u32;
                            sv += row[c + 1] as u32;
                        }
                    }
                    uv[oy * ow + ox * 2] = (su / area) as u8;
                    uv[oy * ow + ox * 2 + 1] = (sv / area) as u8;
                }
            }
        });
        VideoFrame::new_nv12(ow as u32, oh as u32, self.pts, y, uv, self.serial)
    }

    /// Planar I420 copy (for software encoders).
    #[cfg(not(any(target_os = "macos", windows)))]
    pub fn to_i420(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (w, h) = (self.width as usize, self.height as usize);
        let (cw, ch) = (w / 2, h / 2);
        let mut y = vec![0u8; w * h];
        let mut u = vec![0u8; cw * ch];
        let mut v = vec![0u8; cw * ch];
        self.pixels.with_planes(|p| {
            for row in 0..h {
                y[row * w..row * w + w].copy_from_slice(&p.y[row * p.y_stride..row * p.y_stride + w]);
            }
            for row in 0..ch {
                let src = &p.uv[row * p.uv_stride..row * p.uv_stride + cw * 2];
                for c in 0..cw {
                    u[row * cw + c] = src[c * 2];
                    v[row * cw + c] = src[c * 2 + 1];
                }
            }
        });
        (y, u, v)
    }
}

/// Interleaved float audio from one input.
#[derive(Clone, Debug)]
pub struct AudioBlock {
    /// Capture time of the first sample on the host clock.
    pub pts: Ns,
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl AudioBlock {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    /// Apple ProRes (every frame a keyframe; QuickTime files only).
    ProRes(ProRes),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProRes {
    Proxy,
    Lt,
    Standard,
    Hq,
}

impl ProRes {
    /// QuickTime codec four-character code.
    pub fn fourcc(self) -> &'static [u8; 4] {
        match self {
            ProRes::Proxy => b"apco",
            ProRes::Lt => b"apcs",
            ProRes::Standard => b"apcn",
            ProRes::Hq => b"apch",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ProRes::Proxy => "Apple ProRes 422 Proxy",
            ProRes::Lt => "Apple ProRes 422 LT",
            ProRes::Standard => "Apple ProRes 422",
            ProRes::Hq => "Apple ProRes 422 HQ",
        }
    }

    /// Apple's published data rate at 1920×1080, 29.97 fps (Mbps).
    pub fn mbps_1080p30(self) -> f64 {
        match self {
            ProRes::Proxy => 45.0,
            ProRes::Lt => 102.0,
            ProRes::Standard => 147.0,
            ProRes::Hq => 220.0,
        }
    }
}

impl VideoCodec {
    pub fn is_prores(self) -> bool {
        matches!(self, VideoCodec::ProRes(_))
    }
}

/// Decoder configuration of an encoded video stream.
#[derive(Clone, Debug, PartialEq)]
pub struct VideoConfig {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    /// `avcC` / `hvcC` record (the payload of the MP4 box).
    pub record: Vec<u8>,
    /// Parameter sets (SPS/PPS, plus VPS for HEVC) without start codes.
    pub parameter_sets: Vec<Vec<u8>>,
}

/// One encoded video frame: NAL units with 4-byte big-endian lengths (AVCC).
#[derive(Clone, Debug)]
pub struct VideoPacket {
    pub pts: Ns,
    pub dts: Ns,
    pub keyframe: bool,
    pub data: Arc<Vec<u8>>,
    pub config: Arc<VideoConfig>,
}

/// AAC-LC configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioConfig {
    pub rate: u32,
    pub channels: u16,
    /// AudioSpecificConfig.
    pub asc: Vec<u8>,
    /// Encoder delay in samples at the start of the stream.
    pub priming: u32,
}

pub const AAC_FRAME: u32 = 1024;

#[cfg(test)]
mod convert_tests {
    #[test]
    fn bgra_to_nv12_colours_and_scaling() {
        // Left half white, right half pure red; 8×4 BGRA, pitch 40.
        let (sw, sh, pitch) = (8usize, 4usize, 40usize);
        let mut src = vec![0u8; pitch * sh];
        for y in 0..sh {
            for x in 0..sw {
                let i = y * pitch + x * 4;
                let (b, g, r) = if x < 4 { (255, 255, 255) } else { (0, 0, 255) };
                src[i..i + 4].copy_from_slice(&[b, g, r, 255]);
            }
        }
        let (y, uv) = super::bgra_to_nv12(&src, pitch, sw, sh, 8, 4);
        assert_eq!(y[0], 235, "white luma");
        assert!((y[7] as i32 - 63).abs() <= 1, "red luma {}", y[7]);
        assert_eq!(&uv[0..2], &[128, 128], "white chroma");
        assert!((uv[6] as i32 - 102).abs() <= 1 && (uv[7] as i32 - 240).abs() <= 1, "red chroma {:?}", &uv[6..8]);
        // Half size keeps the halves.
        let (y2, _) = super::bgra_to_nv12(&src, pitch, sw, sh, 4, 2);
        assert_eq!(y2[0], 235);
        assert!((y2[3] as i32 - 63).abs() <= 1);
    }
}
