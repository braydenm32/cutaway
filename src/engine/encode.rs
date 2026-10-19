//! Encoder interface and selection.
//!
//! * macOS: VideoToolbox (H.264 and HEVC, hardware) and AudioToolbox (AAC).
//! * Windows: Media Foundation (the graphics card's H.264/HEVC encoder, or
//!   Microsoft's software H.264; Windows' AAC encoder).
//! * Linux (development builds): OpenH264 and FDK-AAC, in software.

use crate::media::{AudioConfig, Ns, VideoCodec, VideoFrame, VideoPacket};
use anyhow::Result;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Files: quality first.
    Record,
    /// Live: steady bitrate, low latency.
    Stream,
}

#[derive(Clone, Debug)]
pub struct VideoSettings {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub purpose: Purpose,
    /// "auto" or a backend name from [`video_backends`].
    pub backend: String,
}

/// Receives encoded packets (from any thread).
pub type VideoSink = Arc<dyn Fn(VideoPacket) + Send + Sync>;

pub trait VideoEncoder: Send {
    /// Encode one frame. `pts` is the frame's time in the output (ns).
    fn encode(&mut self, frame: &VideoFrame, pts: Ns, force_keyframe: bool) -> Result<()>;
    /// Flush; every packet has been delivered to the sink when this returns.
    fn finish(&mut self) -> Result<()>;
    /// Change the bitrate from the next frame on (streaming, when the
    /// connection struggles). False if this encoder can't.
    fn set_bitrate(&mut self, _kbps: u32) -> bool {
        false
    }
}

pub trait AudioEncoder: Send {
    fn config(&self) -> AudioConfig;
    /// Add interleaved samples; returns any complete AAC frames.
    fn push(&mut self, samples: &[f32]) -> Result<Vec<Vec<u8>>>;
    /// Encode what's left. Frames past the input are not returned.
    fn finish(&mut self) -> Result<Vec<Vec<u8>>>;
}

/// Encoder backends available in this build, best first.
pub fn video_backends(codec: VideoCodec) -> Vec<&'static str> {
    let mut v = Vec::new();
    #[cfg(target_os = "macos")]
    {
        let _ = codec;
        v.push("videotoolbox");
    }
    #[cfg(windows)]
    v.extend(crate::platform::win::mf::video_backends(codec));
    #[cfg(not(any(target_os = "macos", windows)))]
    if codec == VideoCodec::H264 {
        v.push("openh264");
    }
    v
}

/// Why a codec can't be used in this build, if it can't.
pub fn codec_problem(codec: VideoCodec) -> Option<String> {
    if video_backends(codec).is_empty() {
        Some(if codec.is_prores() { "ProRes recording needs a Mac".to_string() } else { format!("{} isn't available in this build", codec_name(codec)) })
    } else {
        None
    }
}

pub fn is_hardware(backend: &str) -> bool {
    !matches!(backend, "openh264" | "mf-sw")
}

pub fn describe_backend(backend: &str) -> &'static str {
    match backend {
        "videotoolbox" => "VideoToolbox (hardware)",
        "openh264" => "OpenH264 (software)",
        #[cfg(windows)]
        other => crate::platform::win::mf::describe_backend(other),
        #[cfg(not(windows))]
        _ => "unknown",
    }
}

pub fn codec_name(codec: VideoCodec) -> &'static str {
    match codec {
        VideoCodec::H264 => "H.264",
        VideoCodec::Hevc => "H.265/HEVC",
        VideoCodec::ProRes(p) => p.label(),
    }
}

/// Resolve "auto" to a real backend name.
pub fn resolve_backend(codec: VideoCodec, wanted: &str) -> Result<&'static str> {
    let list = video_backends(codec);
    if wanted.is_empty() || wanted.eq_ignore_ascii_case("auto") {
        return list.first().copied().ok_or_else(|| anyhow::anyhow!("no {} encoder is available in this build", codec_name(codec)));
    }
    list.into_iter()
        .find(|b| b.eq_ignore_ascii_case(wanted))
        .ok_or_else(|| anyhow::anyhow!("encoder \"{wanted}\" isn't available for {}", codec_name(codec)))
}

pub fn create_video(s: &VideoSettings, sink: VideoSink) -> Result<Box<dyn VideoEncoder>> {
    let backend = resolve_backend(s.codec, &s.backend)?;
    match backend {
        #[cfg(target_os = "macos")]
        "videotoolbox" => Ok(Box::new(crate::platform::macos::vt::VtEncoder::new(s, sink)?)),
        #[cfg(windows)]
        b @ ("nvenc" | "amf" | "qsv" | "mf-hw" | "mf-sw") => Ok(Box::new(crate::platform::win::mf::MfVideoEncoder::new(s, b, sink)?)),
        #[cfg(not(any(target_os = "macos", windows)))]
        "openh264" => Ok(Box::new(software::OpenH264::new(s, sink)?)),
        other => anyhow::bail!("encoder {other} isn't built in"),
    }
}

pub fn create_aac(rate: u32, channels: u16, bitrate_kbps: u32) -> Result<Box<dyn AudioEncoder>> {
    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(crate::platform::macos::aac::AtAacEncoder::new(rate, channels, bitrate_kbps)?))
    }
    #[cfg(windows)]
    {
        Ok(Box::new(crate::platform::win::mf::MfAacEncoder::new(rate, channels, bitrate_kbps)?))
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        Ok(Box::new(software::FdkAac::new(rate, channels, bitrate_kbps)?))
    }
}

/// Sensible recording bitrate (high quality, editing friendly).
pub fn auto_bitrate_kbps(codec: VideoCodec, width: u32, height: u32, fps: u32) -> u32 {
    // ~0.2 bits per pixel per frame for H.264: 1080p30 ≈ 12 Mbps,
    // 1080p60 ≈ 25 Mbps, 4K30 ≈ 50 Mbps, 4K60 ≈ 100 Mbps.
    let bpp = match codec {
        VideoCodec::H264 => 0.2,
        VideoCodec::Hevc => 0.12,
        VideoCodec::ProRes(p) => {
            // Fixed quality: scale Apple's 1080p29.97 figure.
            let k = width as f64 * height as f64 * fps as f64 / (1920.0 * 1080.0 * 29.97);
            return (p.mbps_1080p30() * 1000.0 * k) as u32;
        }
    };
    ((width as f64 * height as f64 * fps as f64 * bpp / 1000.0) as u32).clamp(2_000, 150_000)
}

/// The bitrate a recording will actually use (ProRes ignores the setting).
pub fn record_bitrate_kbps(codec: VideoCodec, setting: u32, width: u32, height: u32, fps: u32) -> u32 {
    if setting > 0 && !codec.is_prores() {
        setting
    } else {
        auto_bitrate_kbps(codec, width, height, fps)
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod software {
    use super::*;
    use crate::media::{h264, VideoConfig};
    use anyhow::{anyhow, Context};
    use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, FrameType, IntraFramePeriod, RateControlMode, UsageType};
    use openh264::formats::YUVSlices;
    use openh264::{OpenH264API, Timestamp};

    pub struct OpenH264 {
        enc: Encoder,
        sink: VideoSink,
        config: Option<Arc<VideoConfig>>,
        width: u32,
        height: u32,
    }

    impl OpenH264 {
        pub fn new(s: &VideoSettings, sink: VideoSink) -> Result<Self> {
            let cfg = EncoderConfig::new()
                .bitrate(BitRate::from_bps(s.bitrate_kbps.saturating_mul(1000)))
                .max_frame_rate(FrameRate::from_hz(s.fps as f32))
                .rate_control_mode(RateControlMode::Bitrate)
                .skip_frames(false)
                .usage_type(UsageType::CameraVideoRealTime)
                .intra_frame_period(IntraFramePeriod::from_num_frames(s.fps * 2));
            let enc = Encoder::with_api_config(OpenH264API::from_source(), cfg).context("starting OpenH264")?;
            Ok(OpenH264 { enc, sink, config: None, width: s.width, height: s.height })
        }
    }

    impl VideoEncoder for OpenH264 {
        fn encode(&mut self, frame: &VideoFrame, pts: Ns, force_keyframe: bool) -> Result<()> {
            if frame.width != self.width || frame.height != self.height {
                return Err(anyhow!("frame is {}x{}, encoder expects {}x{}", frame.width, frame.height, self.width, self.height));
            }
            let (y, u, v) = frame.to_i420();
            let (w, h) = (self.width as usize, self.height as usize);
            let src = YUVSlices::new((&y, &u, &v), (w, h), (w, w / 2, w / 2));
            if force_keyframe {
                self.enc.force_intra_frame();
            }
            let bs = self.enc.encode_at(&src, Timestamp::from_millis((pts.max(0) / 1_000_000) as u64))?;
            let kind = bs.frame_type();
            if matches!(kind, FrameType::Skip | FrameType::Invalid) {
                return Ok(());
            }
            let annexb = bs.to_vec();
            if let Some(p) = h264::packet_from_annexb(VideoCodec::H264, (self.width, self.height), &annexb, None, pts, &mut self.config) {
                (self.sink)(p);
            }
            Ok(())
        }

        fn finish(&mut self) -> Result<()> {
            Ok(())
        }

        fn set_bitrate(&mut self, kbps: u32) -> bool {
            // ENCODER_OPTION_BITRATE with an SBitrateInfo for every layer.
            #[repr(C)]
            struct BitrateInfo {
                layer: std::os::raw::c_int,
                bitrate: std::os::raw::c_int,
            }
            const ENCODER_OPTION_BITRATE: std::os::raw::c_int = 5;
            const SPATIAL_LAYER_ALL: std::os::raw::c_int = 4;
            let mut info = BitrateInfo { layer: SPATIAL_LAYER_ALL, bitrate: (kbps.saturating_mul(1000)).min(i32::MAX as u32) as i32 };
            // SAFETY: the option and its struct are as OpenH264's headers define them.
            unsafe { self.enc.raw_api().set_option(ENCODER_OPTION_BITRATE, &mut info as *mut BitrateInfo as *mut std::ffi::c_void) == 0 }
        }
    }

    pub struct FdkAac {
        enc: fdk_aac::enc::Encoder,
        config: AudioConfig,
        pending: Vec<i16>,
        channels: usize,
        input_frames: u64,
        output_frames: u64,
        out: Vec<u8>,
    }

    impl FdkAac {
        pub fn new(rate: u32, channels: u16, bitrate_kbps: u32) -> Result<Self> {
            use fdk_aac::enc::{AudioObjectType, BitRate, ChannelMode, EncoderParams, Transport};
            let enc = fdk_aac::enc::Encoder::new(EncoderParams {
                bit_rate: BitRate::Cbr(bitrate_kbps * 1000),
                sample_rate: rate,
                transport: Transport::Raw,
                channels: if channels == 1 { ChannelMode::Mono } else { ChannelMode::Stereo },
                audio_object_type: AudioObjectType::Mpeg4LowComplexity,
            })
            .map_err(|e| anyhow!("AAC encoder: {e:?}"))?;
            let info = enc.info().map_err(|e| anyhow!("AAC encoder: {e:?}"))?;
            let asc = info.confBuf[..info.confSize as usize].to_vec();
            let config = AudioConfig {
                rate,
                channels,
                asc: if asc.is_empty() { h264::aac_asc(rate, channels) } else { asc },
                priming: info.nDelay,
            };
            Ok(FdkAac { enc, config, pending: Vec::new(), channels: channels as usize, input_frames: 0, output_frames: 0, out: vec![0; 8192] })
        }

        fn run(&mut self, flush_to: Option<u64>) -> Result<Vec<Vec<u8>>> {
            let chunk = 1024 * self.channels;
            let mut frames = Vec::new();
            while self.pending.len() >= chunk {
                let block: Vec<i16> = self.pending.drain(..chunk).collect();
                let mut used = 0;
                while used < block.len() {
                    let info = self.enc.encode(&block[used..], &mut self.out).map_err(|e| anyhow!("AAC encode: {e:?}"))?;
                    used += info.input_consumed.max(1);
                    if info.output_size > 0 {
                        if flush_to.is_none_or(|limit| self.output_frames * 1024 < limit) {
                            frames.push(self.out[..info.output_size].to_vec());
                        }
                        self.output_frames += 1;
                    }
                }
            }
            Ok(frames)
        }
    }

    impl AudioEncoder for FdkAac {
        fn config(&self) -> AudioConfig {
            self.config.clone()
        }

        fn push(&mut self, samples: &[f32]) -> Result<Vec<Vec<u8>>> {
            self.input_frames += (samples.len() / self.channels) as u64;
            self.pending.extend(samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
            self.run(None)
        }

        fn finish(&mut self) -> Result<Vec<Vec<u8>>> {
            // Push silence through the encoder delay, keeping only frames
            // that cover real input.
            let limit = self.input_frames + self.config.priming as u64;
            let mut out = Vec::new();
            let mut guard = 0;
            while self.output_frames * 1024 < limit && guard < 16 {
                self.pending.extend(std::iter::repeat_n(0i16, 1024 * self.channels));
                out.extend(self.run(Some(limit))?);
                guard += 1;
            }
            Ok(out)
        }
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use crate::media::mp4::{Brand, Mp4Writer};
    use crate::media::testsrc::{TestPattern, TestTone};
    use std::sync::Mutex;

    /// The stream's encoder can ease off mid-stream (adaptive bitrate).
    #[test]
    fn bitrate_changes_while_encoding() {
        let sizes = Arc::new(Mutex::new(Vec::<usize>::new()));
        let s2 = sizes.clone();
        let s = VideoSettings { codec: VideoCodec::H264, width: 320, height: 180, fps: 30, bitrate_kbps: 3000, purpose: Purpose::Stream, backend: "auto".into() };
        let mut enc = create_video(&s, Arc::new(move |p| s2.lock().unwrap().push(p.data.len()))).unwrap();
        // A noisy picture, so the bitrate is what limits it.
        let mut seed = 1u32;
        let mut noise = |n: usize| -> Vec<u8> {
            (0..n)
                .map(|i| {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    // Detail everywhere: a gradient with grain.
                    ((i % 256) as u8 / 2).wrapping_add((seed >> 27) as u8)
                })
                .collect()
        };
        let mut run = |enc: &mut Box<dyn VideoEncoder>, from: i64| {
            for i in from..from + 60 {
                let pts = i * 1_000_000_000 / 30;
                let f = VideoFrame::new_nv12(320, 180, pts, noise(320 * 180), noise(320 * 90), i as u64);
                enc.encode(&f, pts, false).unwrap_or_else(|e| panic!("frame {i}: {e:#}"));
            }
        };
        run(&mut enc, 0);
        let before: usize = sizes.lock().unwrap().drain(..).skip(30).sum();
        assert!(enc.set_bitrate(500), "this encoder takes a new bitrate");
        run(&mut enc, 60);
        let after: usize = sizes.lock().unwrap().drain(..).skip(30).sum();
        assert!(after * 3 < before, "much less after lowering it: {before} → {after} bytes");
    }

    #[test]
    fn encode_real_file() {
        let packets = Arc::new(Mutex::new(Vec::new()));
        let p2 = packets.clone();
        let s = VideoSettings { codec: VideoCodec::H264, width: 320, height: 180, fps: 30, bitrate_kbps: 800, purpose: Purpose::Record, backend: "auto".into() };
        let mut enc = create_video(&s, Arc::new(move |p| p2.lock().unwrap().push(p))).unwrap();
        let mut tp = TestPattern::new("ball", 320, 180);
        for i in 0..60i64 {
            let pts = i * 1_000_000_000 / 30;
            let img = tp.render(pts, 30);
            let f = VideoFrame::new_nv12(320, 180, pts, img.y, img.uv, i as u64);
            enc.encode(&f, pts, false).unwrap();
        }
        enc.finish().unwrap();
        let mut aac = create_aac(48000, 2, 128).unwrap();
        let tone = TestTone::new(48000, 2, 440.0);
        let mut afr = Vec::new();
        for i in 0..200 {
            afr.extend(aac.push(&tone.render(i * 10_000_000, 480)).unwrap());
        }
        afr.extend(aac.finish().unwrap());
        assert_eq!(afr.len(), (96_000 + aac.config().priming as usize).div_ceil(1024));

        let path = std::env::temp_dir().join(format!("cutaway-enc-{}.mp4", std::process::id()));
        let mut w = Mp4Writer::create(&path, Brand::Mp4).unwrap();
        let packets = packets.lock().unwrap();
        assert_eq!(packets.len(), 60);
        assert!(packets[0].keyframe);
        let v = w.add_video(&packets[0].config, 30);
        let a = w.add_audio(&aac.config());
        let mut ai = 0;
        for p in packets.iter() {
            while (ai as i64 * 1024 * 1_000_000_000 / 48000) <= p.pts && ai < afr.len() {
                w.write(a, ai as i64 * 1024 * 1_000_000_000 / 48000, &afr[ai], true).unwrap();
                ai += 1;
            }
            w.write(v, p.dts, &p.data, p.keyframe).unwrap();
        }
        while ai < afr.len() {
            w.write(a, ai as i64 * 1024 * 1_000_000_000 / 48000, &afr[ai], true).unwrap();
            ai += 1;
        }
        w.set_timecode(30, (10 * 3600 + 20 * 60 + 30) * 30 + 15);
        w.set_chapters(vec![(500_000_000, "Marker 1".into()), (1_200_000_000, "Marker 2".into())]);
        w.finish().unwrap();
        if let Ok(o) = std::process::Command::new("ffprobe").args(["-v", "error", "-show_chapters", "-show_entries", "stream_tags=timecode", "-of", "compact"]).arg(&path).output() {
            let out = String::from_utf8_lossy(&o.stdout);
            assert!(out.contains("timecode=10:20:30:15"), "{out}");
            assert!(out.contains("start_time=0.000000|end=500|end_time=0.500000|tag:title=Start"), "{out}");
            assert!(out.contains("start_time=0.500000|end=1200|end_time=1.200000|tag:title=Marker 1"), "{out}");
            assert!(out.contains("start=1200|start_time=1.200000") && out.contains("tag:title=Marker 2"), "{out}");
        }
        // ffmpeg must decode it without errors.
        let out = std::process::Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(&path).args(["-f", "null", "-"]).output();
        if let Ok(o) = out {
            assert!(o.stderr.is_empty(), "{}", String::from_utf8_lossy(&o.stderr));
            let probe = std::process::Command::new("ffprobe")
                .args(["-v", "error", "-show_entries", "stream=codec_name,nb_frames,duration,start_time", "-of", "compact"])
                .arg(&path)
                .output()
                .unwrap();
            let s = String::from_utf8_lossy(&probe.stdout);
            assert!(s.contains("codec_name=h264|start_time=0.000000|duration=2.000000|nb_frames=60"), "{s}");
            assert!(s.contains("codec_name=aac|start_time=0.000000"), "{s}");
        }
    }
}
