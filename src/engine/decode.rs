//! Decoding for the media player and network sources.
//!
//! * macOS: VideoToolbox (H.264, HEVC, ProRes; hardware, scaled on the
//!   way out) and AudioToolbox (AAC).
//! * Windows: Media Foundation (H.264; HEVC with the HEVC Video
//!   Extensions; AAC).
//! * Linux (development builds): OpenH264 and FDK-AAC.
//!
//! Decoded audio is converted to 48 kHz stereo, the mixer's format.

use crate::media::{Ns, VideoCodec, VideoFrame};
use anyhow::Result;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// What a video decoder needs to start.
#[derive(Clone, Debug)]
pub struct DecoderSetup {
    pub codec: VideoCodec,
    /// SPS/PPS (and VPS) without start codes; empty for ProRes.
    pub parameter_sets: Vec<Vec<u8>>,
    /// Coded size (VideoToolbox needs it for ProRes).
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub width: u32,
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub height: u32,
    /// Picture size wanted out (scaled by the decoder when it can).
    pub output: (u32, u32),
}

pub trait VideoDecoder: Send {
    /// Decode one access unit (AVCC NAL units, or a ProRes frame). Pictures
    /// come out in decode order with the time they were given.
    fn decode(&mut self, data: &[u8], pts: Ns, out: &mut Vec<VideoFrame>) -> Result<()>;
    /// Pictures still inside the decoder.
    fn flush(&mut self, out: &mut Vec<VideoFrame>) -> Result<()>;
}

pub trait AudioDecoder: Send {
    /// One raw AAC frame -> interleaved samples at the stream's rate/channels.
    fn decode(&mut self, frame: &[u8]) -> Result<Vec<f32>>;
    fn rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// How many samples late the decoder's output is (its own latency).
    fn delay(&self) -> u32 {
        0
    }
}

pub fn create_video_decoder(s: &DecoderSetup) -> Result<Box<dyn VideoDecoder>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(crate::platform::macos::vtdec::VtDecoder::new(s)?))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(crate::platform::win::mf::MfVideoDecoder::new(s)?))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        match s.codec {
            VideoCodec::H264 => Ok(Box::new(software::H264Decoder::new(s)?)),
            other => anyhow::bail!("{} playback isn't available in this build", super::encode::codec_name(other)),
        }
    }
}

pub fn create_aac_decoder(asc: &[u8]) -> Result<Box<dyn AudioDecoder>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(crate::platform::macos::aacdec::AtAacDecoder::new(asc)?))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(crate::platform::win::mf::MfAacDecoder::new(asc)?))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Ok(Box::new(software::AacDecoder::new(asc)?))
    }
}

/// Puts decoded pictures back in presentation order (streams with
/// B-frames decode out of order).
pub struct Reorder {
    heap: BinaryHeap<Reverse<(Ns, u64)>>,
    frames: std::collections::HashMap<u64, VideoFrame>,
    depth: usize,
    n: u64,
}

impl Reorder {
    pub fn new(depth: usize) -> Self {
        Reorder { heap: BinaryHeap::new(), frames: Default::default(), depth, n: 0 }
    }

    pub fn push(&mut self, f: VideoFrame) -> Option<VideoFrame> {
        self.n += 1;
        self.heap.push(Reverse((f.pts, self.n)));
        self.frames.insert(self.n, f);
        if self.heap.len() > self.depth {
            self.pop()
        } else {
            None
        }
    }

    pub fn pop(&mut self) -> Option<VideoFrame> {
        let Reverse((_, id)) = self.heap.pop()?;
        self.frames.remove(&id)
    }

    pub fn clear(&mut self) {
        self.heap.clear();
        self.frames.clear();
    }

    /// Add without releasing anything.
    pub fn push_hold(&mut self, f: VideoFrame) {
        self.n += 1;
        self.heap.push(Reverse((f.pts, self.n)));
        self.frames.insert(self.n, f);
    }

    /// The earliest picture, if more than `depth` are waiting.
    pub fn pop_ready(&mut self) -> Option<VideoFrame> {
        if self.heap.len() > self.depth {
            self.pop()
        } else {
            None
        }
    }

    pub fn set_depth(&mut self, depth: usize) {
        self.depth = depth;
    }
}

/// Converts decoded audio to 48 kHz stereo.
pub struct ToMix {
    rate: u32,
    channels: u16,
    resampler: Option<Async<f32>>,
    pending: Vec<f32>,
    out_buf: Vec<f32>,
}

impl ToMix {
    pub fn new(rate: u32, channels: u16) -> Self {
        let resampler = (rate != 48_000).then(|| {
            let params = SincInterpolationParameters::new(64, WindowFunction::BlackmanHarris2).oversampling_factor(64).interpolation(SincInterpolationType::Linear);
            Async::<f32>::new_sinc(48_000.0 / rate.max(1) as f64, 1.0, &params, 1024, 2, FixedAsync::Input).expect("resampler")
        });
        ToMix { rate, channels, resampler, pending: Vec::new(), out_buf: vec![0.0; 8192 * 2] }
    }

    pub fn matches(&self, rate: u32, channels: u16) -> bool {
        self.rate == rate && self.channels == channels
    }

    /// Interleaved input -> interleaved 48 kHz stereo (may hold some back).
    pub fn push(&mut self, samples: &[f32]) -> Vec<f32> {
        let ch = self.channels.max(1) as usize;
        let mut stereo = Vec::with_capacity(samples.len() / ch * 2);
        for f in samples.chunks_exact(ch) {
            let (l, r) = if ch == 1 { (f[0], f[0]) } else { (f[0], f[1]) };
            stereo.push(l);
            stereo.push(r);
        }
        let Some(rs) = self.resampler.as_mut() else { return stereo };
        self.pending.extend(stereo);
        let mut out = Vec::new();
        loop {
            let need = rs.input_frames_next();
            if self.pending.len() / 2 < need {
                break;
            }
            let input = InterleavedSlice::new(&self.pending[..need * 2], 2, need).expect("input");
            let cap = self.out_buf.len() / 2;
            let mut output = InterleavedSlice::new_mut(&mut self.out_buf, 2, cap).expect("output");
            match rs.process_into_buffer(&input, &mut output, None) {
                Ok((_, produced)) => out.extend_from_slice(&self.out_buf[..produced * 2]),
                Err(_) => break,
            }
            self.pending.drain(..need * 2);
        }
        out
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod software {
    use super::*;
    use crate::media::h264;
    use anyhow::anyhow;
    use openh264::decoder::Decoder;
    use openh264::formats::YUVSource;

    pub struct H264Decoder {
        dec: Decoder,
        params: Vec<Vec<u8>>,
        sent_params: bool,
        output: (u32, u32),
    }

    // The decoder is only used from one thread.
    unsafe impl Send for H264Decoder {}

    impl H264Decoder {
        pub fn new(s: &DecoderSetup) -> Result<Self> {
            Ok(H264Decoder { dec: Decoder::new().map_err(|e| anyhow!("OpenH264: {e}"))?, params: s.parameter_sets.clone(), sent_params: false, output: s.output })
        }
    }

    impl VideoDecoder for H264Decoder {
        fn decode(&mut self, data: &[u8], pts: Ns, out: &mut Vec<VideoFrame>) -> Result<()> {
            let nals = h264::split_avcc(data);
            let has_params = nals.iter().any(|n| h264::h264_nal_type(n) == h264::NAL_SPS);
            let mut annexb = Vec::with_capacity(data.len() + 64);
            if !self.sent_params && !has_params {
                annexb.extend(h264::to_annexb(self.params.iter().map(|p| p.as_slice())));
            }
            self.sent_params = true;
            annexb.extend(h264::to_annexb(nals.iter().copied()));
            let Some(yuv) = self.dec.decode(&annexb).map_err(|e| anyhow!("OpenH264: {e}"))? else { return Ok(()) };
            let (w, h) = yuv.dimensions();
            let (ys, us, _) = yuv.strides();
            let mut y = vec![0u8; w * h];
            let mut uv = vec![0u8; w * h / 2];
            for r in 0..h {
                y[r * w..r * w + w].copy_from_slice(&yuv.y()[r * ys..r * ys + w]);
            }
            for r in 0..h / 2 {
                for c in 0..w / 2 {
                    uv[r * w + c * 2] = yuv.u()[r * us + c];
                    uv[r * w + c * 2 + 1] = yuv.v()[r * us + c];
                }
            }
            let mut f = VideoFrame::new_nv12(w as u32, h as u32, pts, y, uv, 0);
            if (f.width, f.height) != self.output {
                f = f.scaled(self.output.0, self.output.1);
            }
            out.push(f);
            Ok(())
        }

        fn flush(&mut self, _out: &mut Vec<VideoFrame>) -> Result<()> {
            Ok(())
        }
    }

    pub struct AacDecoder {
        dec: fdk_aac::dec::Decoder,
        rate: u32,
        channels: u16,
        delay: u32,
        pcm: Vec<i16>,
    }

    unsafe impl Send for AacDecoder {}

    impl AacDecoder {
        pub fn new(asc: &[u8]) -> Result<Self> {
            let mut dec = fdk_aac::dec::Decoder::new(fdk_aac::dec::Transport::Raw);
            dec.config_raw(asc).map_err(|e| anyhow!("AAC decoder: {e:?}"))?;
            let (rate, channels) = crate::media::demux::parse_asc(asc).unwrap_or((48_000, 2));
            Ok(AacDecoder { dec, rate, channels, delay: 0, pcm: vec![0; 2048 * 8] })
        }
    }

    impl AudioDecoder for AacDecoder {
        fn decode(&mut self, frame: &[u8]) -> Result<Vec<f32>> {
            self.dec.fill(frame).map_err(|e| anyhow!("AAC decoder: {e:?}"))?;
            match self.dec.decode_frame(&mut self.pcm) {
                Ok(()) => {}
                Err(e) => return Err(anyhow!("AAC decoder: {e:?}")),
            }
            let info = self.dec.stream_info();
            self.rate = info.sampleRate as u32;
            self.channels = info.numChannels as u16;
            self.delay = info.outputDelay;
            let n = self.dec.decoded_frame_size();
            Ok(self.pcm[..n].iter().map(|s| *s as f32 / 32768.0).collect())
        }
        fn rate(&self) -> u32 {
            self.rate
        }
        fn channels(&self) -> u16 {
            self.channels
        }
        fn delay(&self) -> u32 {
            self.delay
        }
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use crate::media::demux::{Mp4Reader, TrackKind};

    #[test]
    fn decodes_a_file() {
        let dir = std::env::temp_dir().join(format!("cutaway-decode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("clip.mp4");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=1", "-f", "lavfi", "-i", "sine=duration=1:sample_rate=44100:frequency=1000"])
            .args(["-c:v", "libx264", "-profile:v", "baseline", "-c:a", "aac"])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let mut r = Mp4Reader::open(&p).unwrap();
        let vi = r.video().unwrap();
        let TrackKind::Video(cfg) = r.tracks[vi].kind.clone() else { unreachable!() };
        let mut dec = create_video_decoder(&DecoderSetup { codec: cfg.codec, parameter_sets: cfg.parameter_sets.clone(), width: cfg.width, height: cfg.height, output: (160, 120) }).unwrap();
        let mut frames = Vec::new();
        for i in 0..r.tracks[vi].samples.len() {
            let data = r.read(vi, i).unwrap();
            dec.decode(&data, r.tracks[vi].pts(i), &mut frames).unwrap();
        }
        dec.flush(&mut frames).unwrap();
        assert!(frames.len() >= 24, "{}", frames.len());
        assert_eq!((frames[0].width, frames[0].height), (160, 120));

        let ai = r.audio().unwrap();
        let TrackKind::Audio(acfg) = r.tracks[ai].kind.clone() else { unreachable!() };
        let mut adec = create_aac_decoder(&acfg.asc).unwrap();
        let mut mix = ToMix::new(acfg.rate, acfg.channels);
        let mut total = 0;
        let mut peak = 0.0f32;
        for i in 0..r.tracks[ai].samples.len() {
            let pcm = adec.decode(&r.read(ai, i).unwrap()).unwrap();
            if mix.matches(adec.rate(), adec.channels()) {
                let out = mix.push(&pcm);
                peak = out.iter().fold(peak, |m, s| m.max(s.abs()));
                total += out.len() / 2;
            }
        }
        assert_eq!(adec.rate(), 44100);
        // ~1 s at 48 kHz (minus what the resampler holds back).
        assert!(total > 44_000 && total < 50_000, "{total}");
        assert!(peak > 0.05, "{peak}");
        std::fs::remove_dir_all(&dir).ok();
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod sync_probe {
    use super::*;
    use crate::media::demux::{Mp4Reader, TrackKind};

    #[test]
    #[ignore]
    fn probe_clip_sync() {
        let p = std::env::var("CLIP").unwrap();
        let mut r = Mp4Reader::open(std::path::Path::new(&p)).unwrap();
        let vi = r.video().unwrap();
        let TrackKind::Video(cfg) = r.tracks[vi].kind.clone() else { unreachable!() };
        let mut dec = create_video_decoder(&DecoderSetup { codec: cfg.codec, parameter_sets: cfg.parameter_sets.clone(), width: cfg.width, height: cfg.height, output: (cfg.width, cfg.height) }).unwrap();
        let mut frames = Vec::new();
        for i in 0..r.tracks[vi].samples.len() {
            dec.decode(&r.read(vi, i).unwrap(), r.tracks[vi].pts(i), &mut frames).unwrap();
        }
        for f in &frames {
            let lum = f.pixels.with_planes(|p| p.y[0]);
            if lum > 200 {
                eprintln!("flash at {:.3}", f.pts as f64 / 1e9);
            }
        }
        let ai = r.audio().unwrap();
        let TrackKind::Audio(acfg) = r.tracks[ai].kind.clone() else { unreachable!() };
        eprintln!("audio start (edit) {} ticks, timescale {}", r.tracks[ai].start, r.tracks[ai].timescale);
        let mut adec = create_aac_decoder(&acfg.asc).unwrap();
        let mut last_beep = -10.0;
        for i in 0..r.tracks[ai].samples.len() {
            let pts = r.tracks[ai].pts(i);
            let pcm = adec.decode(&r.read(ai, i).unwrap()).unwrap();
            let ch = adec.channels() as usize;
            for (k, f) in pcm.chunks(ch).enumerate() {
                let t = pts as f64 / 1e9 + k as f64 / adec.rate() as f64;
                if f[0].abs() > 0.4 && t - last_beep > 0.5 {
                    eprintln!("beep at {t:.4} (packet {i} pts {:.4}, decoder delay {})", pts as f64 / 1e9, adec.delay());
                    last_beep = t;
                }
            }
        }
    }
}
