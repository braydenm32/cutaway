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
///
/// Whole-number arithmetic, a row at a time: a 4K screen is converted
/// every frame, so this is kept quick.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn bgra_to_nv12(src: &[u8], pitch: usize, sw: usize, sh: usize, w: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
    let (w, h) = (w.max(2) & !1, h.max(2) & !1);
    let mut y = vec![16u8; w * h];
    let mut uv = vec![128u8; w * h / 2];
    if sw == 0 || sh == 0 || pitch < sw * 4 || src.len() < pitch * (sh - 1) + sw * 4 {
        return (y, uv);
    }
    // For each output column/row: the two source pixels and the weight of
    // the second (0-256).
    fn taps(out: usize, size: usize) -> Vec<(usize, usize, u32)> {
        let f = size as f32 / out as f32;
        (0..out)
            .map(|o| {
                if out == size {
                    return (o, o, 0);
                }
                let x = ((o as f32 + 0.5) * f - 0.5).clamp(0.0, (size - 1) as f32);
                let x0 = x as usize;
                (x0, (x0 + 1).min(size - 1), ((x - x0 as f32) * 256.0).round() as u32)
            })
            .collect()
    }
    let (xs, ys_taps) = (taps(w, sw), taps(h, sh));
    let same = sw == w && sh == h;
    // BT.709 video range, scaled by 2^14 (inputs carry 8 more bits).
    let c = |v: f64| (v * (1 << 14) as f64).round() as i32;
    let (yr, yg, yb) = (c(0.2126 * 219.0 / 255.0), c(0.7152 * 219.0 / 255.0), c(0.0722 * 219.0 / 255.0));
    let (ur, ug, ub) = (c(-0.1146 * 224.0 / 255.0), c(-0.3854 * 224.0 / 255.0), c(0.5 * 224.0 / 255.0));
    let (vr, vg, vb) = (c(0.5 * 224.0 / 255.0), c(-0.4542 * 224.0 / 255.0), c(-0.0458 * 224.0 / 255.0));
    // One output row, sampled: R, G and B rows, ×256.
    let sample_row = |oy: usize, out: &mut [Vec<i32>; 3]| {
        let (y0, y1, fy) = ys_taps[oy];
        let (a, b) = (&src[y0 * pitch..y0 * pitch + sw * 4], &src[y1 * pitch..y1 * pitch + sw * 4]);
        let [rs, gs, bs] = out;
        let outs = rs.iter_mut().zip(gs.iter_mut()).zip(bs.iter_mut());
        if same {
            for (((r, g), b), px) in outs.zip(a.chunks_exact(4)) {
                (*r, *g, *b) = ((px[2] as i32) << 8, (px[1] as i32) << 8, (px[0] as i32) << 8);
            }
            return;
        }
        let fy = fy as i32;
        for (((r, g), bl), &(x0, x1, fx)) in outs.zip(xs.iter()) {
            let (p0, p1, q0, q1) = (&a[x0 * 4..x0 * 4 + 4], &a[x1 * 4..x1 * 4 + 4], &b[x0 * 4..x0 * 4 + 4], &b[x1 * 4..x1 * 4 + 4]);
            let fx = fx as i32;
            let mix = |ch: usize| {
                let top = p0[ch] as i32 * (256 - fx) + p1[ch] as i32 * fx;
                let bot = q0[ch] as i32 * (256 - fx) + q1[ch] as i32 * fx;
                (top * (256 - fy) + bot * fy + 128) >> 8
            };
            (*r, *g, *bl) = (mix(2), mix(1), mix(0));
        }
    };
    let luma_row = |p: &[Vec<i32>; 3], out: &mut [u8]| {
        for (((o, r), g), b) in out.iter_mut().zip(&p[0]).zip(&p[1]).zip(&p[2]) {
            *o = (((16 << 22) + yr * r + yg * g + yb * b + (1 << 21)) >> 22) as u8;
        }
    };
    let rows_per = 2 * (h / 2).div_ceil(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 4)).max(1);
    std::thread::scope(|s| {
        for (part, (ys, cs)) in y.chunks_mut(rows_per * w).zip(uv.chunks_mut(rows_per / 2 * w)).enumerate() {
            let (sample_row, luma_row) = (&sample_row, &luma_row);
            s.spawn(move || {
                let first = part * rows_per;
                let row = || [vec![0i32; w], vec![0i32; w], vec![0i32; w]];
                let (mut top, mut bot) = (row(), row());
                for (r2, crow) in cs.chunks_exact_mut(w).enumerate() {
                    let oy = first + r2 * 2;
                    sample_row(oy, &mut top);
                    sample_row(oy + 1, &mut bot);
                    let (yt, yb2) = ys[r2 * 2 * w..(r2 * 2 + 2) * w].split_at_mut(w);
                    luma_row(&top, yt);
                    luma_row(&bot, yb2);
                    for (cx, out) in crow.chunks_exact_mut(2).enumerate() {
                        let (i, j) = (cx * 2, cx * 2 + 1);
                        // The 2×2 block's average, ×256.
                        let avg = |k: usize| (top[k][i] + top[k][j] + bot[k][i] + bot[k][j] + 2) >> 2;
                        let (r, g, b) = (avg(0), avg(1), avg(2));
                        let chroma = |cr: i32, cg: i32, cb: i32| (((128 << 22) + cr * r + cg * g + cb * b + (1 << 21)) >> 22).clamp(16, 240) as u8;
                        out[0] = chroma(ur, ug, ub);
                        out[1] = chroma(vr, vg, vb);
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
    /// to `width` × `height` (bilinear). A part the same size as the output
    /// is copied straight across; large pictures are done on a few threads
    /// at once.
    pub fn crop_scaled(&self, rect: [f32; 4], width: u32, height: u32) -> VideoFrame {
        let (ow, oh) = ((width.max(2) & !1) as usize, (height.max(2) & !1) as usize);
        let (w, h) = (self.width.max(2) as usize, self.height.max(2) as usize);
        let mut y = vec![0u8; ow * oh];
        let mut uv = vec![0u8; ow * oh / 2];
        let same_size = (rect[2] - ow as f32).abs() < 0.01 && (rect[3] - oh as f32).abs() < 0.01 && ow <= w && oh <= h;
        if same_size {
            // Pixel for pixel: copy rows (from an even spot, so colour
            // stays lined up with brightness).
            let even = |v: f32, max: usize| (((v / 2.0).round() as isize).max(0) as usize * 2).min(max & !1);
            let (left, top) = (even(rect[0], w - ow), even(rect[1], h - oh));
            self.pixels.with_planes(|p| {
                if p.y.len() < (top + oh - 1) * p.y_stride + left + ow || p.uv.len() < (top / 2 + oh / 2 - 1) * p.uv_stride + left + ow {
                    return;
                }
                for (r, out) in y.chunks_exact_mut(ow).enumerate() {
                    let at = (top + r) * p.y_stride + left;
                    out.copy_from_slice(&p.y[at..at + ow]);
                }
                for (r, out) in uv.chunks_exact_mut(ow).enumerate() {
                    let at = (top / 2 + r) * p.uv_stride + left;
                    out.copy_from_slice(&p.uv[at..at + ow]);
                }
            });
            return VideoFrame::new_nv12(ow as u32, oh as u32, self.pts, y, uv, self.serial);
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
                scale_plane::<1>(p.y, p.y_stride, yx, &yy[first * 2..], ys, ow);
                scale_plane::<2>(p.uv, p.uv_stride, cx, &cy[first..], cs, ow);
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

/// For each output column/row: the two source pixels and the weight of the
/// second (0-256).
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

/// Bilinear scaling of one plane (`C` bytes a pixel) into `out` rows of
/// `row_bytes`: each source row is blended sideways once, then
/// neighbouring rows are mixed, so rows shared by several output rows
/// (zoomed in) aren't redone.
fn scale_plane<const C: usize>(src: &[u8], stride: usize, xs: &[(usize, usize, u32)], ys: &[(usize, usize, u32)], out: &mut [u8], row_bytes: usize) {
    let used = xs.iter().map(|t| t.1 + 1).max().unwrap_or(0) * C;
    let across = |row: usize, buf: &mut [u32]| {
        let Some(line) = src.get(row * stride..row * stride + used) else {
            buf.fill(0);
            return;
        };
        for (o, &(x0, x1, fx)) in buf.chunks_exact_mut(C).zip(xs) {
            let (a, b) = (&line[x0 * C..x0 * C + C], &line[x1 * C..x1 * C + C]);
            for c in 0..C {
                o[c] = a[c] as u32 * (256 - fx) + b[c] as u32 * fx;
            }
        }
    };
    if ys.len() > 1 && ys[ys.len() - 1].0 > ys[0].0 + ys.len() {
        // Shrinking: rows are hardly ever shared, so mix each pixel from
        // its four neighbours straight away.
        for (o, &(y0, y1, fy)) in out.chunks_exact_mut(row_bytes).zip(ys) {
            let (Some(a), Some(b)) = (src.get(y0 * stride..y0 * stride + used), src.get(y1 * stride..y1 * stride + used)) else { continue };
            for (o, &(x0, x1, fx)) in o.chunks_exact_mut(C).zip(xs) {
                for c in 0..C {
                    let top = a[x0 * C + c] as u32 * (256 - fx) + a[x1 * C + c] as u32 * fx;
                    let bot = b[x0 * C + c] as u32 * (256 - fx) + b[x1 * C + c] as u32 * fx;
                    o[c] = ((top * (256 - fy) + bot * fy + 32768) >> 16) as u8;
                }
            }
        }
        return;
    }
    // The two most recent blended rows and which source rows they are.
    let (mut a, mut b) = (vec![0u32; row_bytes], vec![0u32; row_bytes]);
    let (mut ra, mut rb) = (usize::MAX, usize::MAX);
    for (o, &(y0, y1, fy)) in out.chunks_exact_mut(row_bytes).zip(ys) {
        if ra != y0 {
            if rb == y0 {
                std::mem::swap(&mut a, &mut b);
                std::mem::swap(&mut ra, &mut rb);
            } else {
                across(y0, &mut a);
                ra = y0;
            }
        }
        if fy == 0 {
            for (o, &t) in o.iter_mut().zip(&a) {
                *o = ((t + 128) >> 8) as u8;
            }
            continue;
        }
        if rb != y1 {
            across(y1, &mut b);
            rb = y1;
        }
        for ((o, &t), &u) in o.iter_mut().zip(&a).zip(&b) {
            *o = ((t * (256 - fy) + u * fy + 32768) >> 16) as u8;
        }
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
    /// Parameter sets (SPS/PPS, plus VPS for HEVC) without start codes;
    /// for ProRes read from a file, its four-character code.
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
    /// The earlier floating-point conversion, to check the quick one against.
    /// An NV12 picture (BT.709, video range) from a BGRA one (`pitch` bytes
    /// per row), scaled to `w`×`h` (bilinear). Screen captures on Windows.
    pub(super) fn reference(src: &[u8], pitch: usize, sw: usize, sh: usize, w: usize, h: usize) -> (Vec<u8>, Vec<u8>) {
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

    fn picture(sh: usize, pitch: usize) -> Vec<u8> {
        let mut seed = 12345u32;
        (0..pitch * sh)
            .map(|_| {
                seed = seed.wrapping_mul(1103515245).wrapping_add(12345);
                (seed >> 16) as u8
            })
            .collect()
    }

    #[test]
    fn bgra_to_nv12_matches_the_reference() {
        for &(sw, sh, w, h) in &[(64, 36, 64, 36), (100, 60, 64, 36), (64, 36, 100, 58), (33, 17, 20, 10)] {
            let pitch = sw * 4 + 12;
            let src = picture(sh, pitch);
            let (y, uv) = super::bgra_to_nv12(&src, pitch, sw, sh, w, h);
            let (ry, ruv) = reference(&src, pitch, sw, sh, w, h);
            let worst = |a: &[u8], b: &[u8]| a.iter().zip(b).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap_or(0);
            assert!(worst(&y, &ry) <= 1 && worst(&uv, &ruv) <= 1, "{sw}x{sh} -> {w}x{h}: luma off by {}, chroma by {}", worst(&y, &ry), worst(&uv, &ruv));
        }
    }

    /// cargo test --release bgra_speed -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bgra_speed() {
        for &(sw, sh, w, h) in &[(1920, 1080, 1920, 1080), (2560, 1600, 1920, 1080), (3840, 2160, 1920, 1080), (3840, 2160, 3840, 2160)] {
            let src = picture(sh, sw * 4);
            let time = |f: &dyn Fn()| {
                let t = std::time::Instant::now();
                for _ in 0..10 {
                    f();
                }
                t.elapsed().as_secs_f64() * 100.0
            };
            let new = time(&|| drop(super::bgra_to_nv12(&src, sw * 4, sw, sh, w, h)));
            let old = time(&|| drop(reference(&src, sw * 4, sw, sh, w, h)));
            println!("{sw}x{sh} -> {w}x{h}: {old:.1} ms before, {new:.1} ms now");
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain bilinear, one pixel at a time, to check the quicker paths.
    fn reference(f: &VideoFrame, rect: [f32; 4], ow: usize, oh: usize) -> (Vec<u8>, Vec<u8>) {
        let (w, h) = (f.width as usize, f.height as usize);
        let (yx, yy) = (taps(rect[0], rect[2], ow, w), taps(rect[1], rect[3], oh, h));
        let (cx, cy) = (taps(rect[0] / 2.0, rect[2] / 2.0, ow / 2, w / 2), taps(rect[1] / 2.0, rect[3] / 2.0, oh / 2, h / 2));
        let mix = |plane: &[u8], stride: usize, c: usize, n: usize, (x0, x1, fx): (usize, usize, u32), (y0, y1, fy): (usize, usize, u32)| {
            let at = |x: usize, y: usize| plane[y * stride + x * n + c] as u32;
            let top = at(x0, y0) * (256 - fx) + at(x1, y0) * fx;
            let bot = at(x0, y1) * (256 - fx) + at(x1, y1) * fx;
            ((top * (256 - fy) + bot * fy + 32768) >> 16) as u8
        };
        f.pixels.with_planes(|p| {
            let y = yy.iter().flat_map(|&ty| yx.iter().map(move |&tx| (tx, ty))).map(|(tx, ty)| mix(p.y, p.y_stride, 0, 1, tx, ty)).collect();
            let uv = cy.iter().flat_map(|&ty| cx.iter().flat_map(move |&tx| [(tx, ty, 0), (tx, ty, 1)])).map(|(tx, ty, c)| mix(p.uv, p.uv_stride, c, 2, tx, ty)).collect();
            (y, uv)
        })
    }

    #[test]
    fn crop_scaled_matches_plain_bilinear() {
        let (w, h) = (1280usize, 720usize);
        let noise = |i: usize| ((i as u64).wrapping_mul(2654435761) >> 7) as u8;
        let f = VideoFrame::new_nv12(w as u32, h as u32, 0, (0..w * h).map(noise).collect(), (0..w * h / 2).map(|i| noise(i + 7)).collect(), 0);
        // Zoomed in (big and small), shrunk, and stretched.
        for (rect, ow, oh) in [([100.3, 50.7, 600.0, 337.5], 1280, 720), ([10.0, 20.0, 100.0, 60.0], 320, 180), ([0.0, 0.0, 1280.0, 720.0], 640, 360), ([0.0, 0.0, 1280.0, 720.0], 1100, 300)] {
            let out = f.crop_scaled(rect, ow, oh);
            let (y, uv) = reference(&f, rect, ow as usize, oh as usize);
            out.pixels.with_planes(|p| {
                assert!(p.y == &y[..], "brightness differs for {rect:?}");
                assert!(p.uv == &uv[..], "colour differs for {rect:?}");
            });
        }
    }

    #[test]
    fn same_size_crop_is_copied_exactly() {
        let (w, h) = (64usize, 32usize);
        let f = VideoFrame::new_nv12(w as u32, h as u32, 0, (0..w * h).map(|i| i as u8).collect(), (0..w * h / 2).map(|i| (i * 3) as u8).collect(), 0);
        // Half the size, from (20, 10), even when the spot isn't whole pixels.
        for rect in [[20.0, 10.0, 32.0, 16.0], [20.4, 9.8, 32.0, 16.0]] {
            let out = f.crop_scaled(rect, 32, 16);
            f.pixels.with_planes(|src| {
                out.pixels.with_planes(|p| {
                    for r in 0..16 {
                        assert_eq!(&p.y[r * 32..r * 32 + 32], &src.y[(10 + r) * w + 20..(10 + r) * w + 52]);
                    }
                    for r in 0..8 {
                        assert_eq!(&p.uv[r * 32..r * 32 + 32], &src.uv[(5 + r) * w + 20..(5 + r) * w + 52]);
                    }
                })
            });
        }
        // Off the edge: held inside the picture.
        let out = f.crop_scaled([50.0, 30.0, 32.0, 16.0], 32, 16);
        out.pixels.with_planes(|p| assert_eq!(p.y[0], f.pixels.with_planes(|s| s.y[16 * w + 32])));
    }
}
