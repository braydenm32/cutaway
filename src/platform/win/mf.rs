//! Media Foundation encoders and decoders.
//!
//! * Video encoding: the graphics card's H.264/HEVC encoder (NVIDIA NVENC,
//!   AMD, Intel Quick Sync), or Microsoft's software H.264 encoder.
//! * AAC encoding and decoding: Windows' own AAC codecs.
//! * Video decoding (network sources, the media player): Microsoft's
//!   H.264 decoder, and HEVC when the HEVC Video Extensions are installed.
//!
//! Hardware encoders are asynchronous transforms (they say when they want
//! input and have output); the rest are synchronous. Both are driven here.

use super::{explain, mf, pack, planes, string_attr, unpack};
use crate::engine::decode::{AudioDecoder, DecoderSetup, VideoDecoder};
use crate::engine::encode::{AudioEncoder, Purpose, VideoEncoder, VideoSettings, VideoSink};
use crate::media::{h264, AudioConfig, Ns, VideoCodec, VideoConfig, VideoFrame, AAC_FRAME};
use anyhow::{anyhow, bail, Result};
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::sync::{Arc, Mutex, OnceLock};
use windows::core::{Interface, GUID};
use windows::Win32::Foundation::VARIANT_TRUE;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Variant::{VARIANT, VT_BOOL, VT_UI4};

// ---- finding transforms ----------------------------------------------------------

/// A transform Windows offers.
struct Found {
    act: IMFActivate,
    vendor: String,
    hardware: bool,
}

fn enumerate(category: GUID, flags: MFT_ENUM_FLAG, input: Option<(GUID, GUID)>, output: Option<(GUID, GUID)>) -> Vec<Found> {
    if mf().is_err() {
        return Vec::new();
    }
    let info = |t: (GUID, GUID)| MFT_REGISTER_TYPE_INFO { guidMajorType: t.0, guidSubtype: t.1 };
    let (i, o) = (input.map(info), output.map(info));
    let mut list: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut n = 0u32;
    // SAFETY: valid pointers; the returned array is taken apart and freed.
    let ok = unsafe { MFTEnumEx(category, flags, i.as_ref().map(|x| x as *const _), o.as_ref().map(|x| x as *const _), &mut list, &mut n) }.is_ok();
    let mut out = Vec::new();
    if ok && !list.is_null() {
        for k in 0..n as usize {
            if let Some(act) = unsafe { std::ptr::read(list.add(k)) } {
                let vendor = string_attr(&act, &MFT_ENUM_HARDWARE_VENDOR_ID_Attribute).unwrap_or_default();
                out.push(Found { hardware: flags.0 & MFT_ENUM_FLAG_HARDWARE.0 != 0, vendor, act });
            }
        }
        unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(list as *const _)) };
    }
    out
}

fn video_subtype(codec: VideoCodec) -> Option<GUID> {
    match codec {
        VideoCodec::H264 => Some(MFVideoFormat_H264),
        VideoCodec::Hevc => Some(MFVideoFormat_HEVC),
        VideoCodec::ProRes(_) => None,
    }
}

/// Backend name for an encoder: by maker for graphics cards.
fn backend_of(f: &Found) -> &'static str {
    if !f.hardware {
        return "mf-sw";
    }
    let v = f.vendor.to_uppercase();
    if v.contains("10DE") {
        "nvenc"
    } else if v.contains("1002") || v.contains("1022") {
        "amf"
    } else if v.contains("8086") {
        "qsv"
    } else {
        "mf-hw"
    }
}

fn encoders(codec: VideoCodec) -> Vec<Found> {
    let Some(sub) = video_subtype(codec) else { return Vec::new() };
    let input = Some((MFMediaType_Video, MFVideoFormat_NV12));
    let output = Some((MFMediaType_Video, sub));
    let mut v = enumerate(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG(MFT_ENUM_FLAG_HARDWARE.0 | MFT_ENUM_FLAG_SORTANDFILTER.0), input, output);
    v.extend(enumerate(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0), input, output));
    v
}

/// Encoder backends on this PC for `codec`, best first.
pub fn video_backends(codec: VideoCodec) -> Vec<&'static str> {
    static CACHE: OnceLock<Mutex<HashMap<u8, Vec<&'static str>>>> = OnceLock::new();
    let key = match codec {
        VideoCodec::H264 => 0,
        VideoCodec::Hevc => 1,
        VideoCodec::ProRes(_) => return Vec::new(),
    };
    let cache = CACHE.get_or_init(Default::default);
    if let Some(v) = cache.lock().unwrap().get(&key) {
        return v.clone();
    }
    let mut v: Vec<&'static str> = Vec::new();
    for f in encoders(codec) {
        let b = backend_of(&f);
        if !v.contains(&b) {
            v.push(b);
        }
    }
    cache.lock().unwrap().insert(key, v.clone());
    v
}

pub fn describe_backend(backend: &str) -> &'static str {
    match backend {
        "nvenc" => "NVIDIA NVENC (hardware)",
        "amf" => "AMD (hardware)",
        "qsv" => "Intel Quick Sync (hardware)",
        "mf-hw" => "graphics card (hardware)",
        "mf-sw" => "Microsoft (software)",
        _ => "unknown",
    }
}

// ---- helpers ------------------------------------------------------------------------------

fn variant_u32(v: u32) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: writing the type tag and the matching union member.
    unsafe {
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_UI4;
        inner.Anonymous.ulVal = v;
    }
    var
}

fn variant_bool(v: bool) -> VARIANT {
    let mut var = VARIANT::default();
    // SAFETY: as above.
    unsafe {
        let inner = &mut *var.Anonymous.Anonymous;
        inner.vt = VT_BOOL;
        inner.Anonymous.boolVal = if v { VARIANT_TRUE } else { windows::Win32::Foundation::VARIANT_BOOL(0) };
    }
    var
}

/// A sample holding `data`, at `time` (100 ns) for `duration`.
fn sample_with(data: &[u8], time: i64, duration: i64) -> Result<IMFSample> {
    // SAFETY: the buffer is locked while written and sized to the data.
    unsafe {
        let buf = MFCreateMemoryBuffer(data.len().max(1) as u32)?;
        let mut p = std::ptr::null_mut();
        buf.Lock(&mut p, None, None)?;
        std::ptr::copy_nonoverlapping(data.as_ptr(), p, data.len());
        buf.Unlock()?;
        buf.SetCurrentLength(data.len() as u32)?;
        let s = MFCreateSample()?;
        s.AddBuffer(&buf)?;
        s.SetSampleTime(time)?;
        if duration > 0 {
            s.SetSampleDuration(duration)?;
        }
        Ok(s)
    }
}

/// All the bytes in a sample.
fn sample_bytes(s: &IMFSample) -> Result<Vec<u8>> {
    // SAFETY: the buffer is locked while read.
    unsafe {
        let buf = s.ConvertToContiguousBuffer()?;
        let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
        buf.Lock(&mut p, None, Some(&mut len))?;
        let v = std::slice::from_raw_parts(p, len as usize).to_vec();
        buf.Unlock()?;
        Ok(v)
    }
}

/// A synchronous or asynchronous transform, driven the same way.
struct Transform {
    mft: IMFTransform,
    events: Option<IMFMediaEventGenerator>,
    need_input: u32,
    drained: bool,
    provides_samples: bool,
    out_size: u32,
}

/// What came out of one ProcessOutput call.
enum Out {
    Sample(IMFSample),
    /// The output format changed (read it again).
    Changed,
    NeedMore,
}

impl Transform {
    fn new(mft: IMFTransform) -> Result<Transform> {
        // SAFETY: plain calls on a valid transform.
        let events = unsafe {
            let attrs = mft.GetAttributes().ok();
            let asynchronous = attrs.as_ref().and_then(|a| a.GetUINT32(&MF_TRANSFORM_ASYNC).ok()).unwrap_or(0) != 0;
            if asynchronous {
                if let Some(a) = &attrs {
                    a.SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)?;
                }
                Some(mft.cast::<IMFMediaEventGenerator>()?)
            } else {
                None
            }
        };
        Ok(Transform { mft, events, need_input: 0, drained: false, provides_samples: false, out_size: 0 })
    }

    fn refresh_stream_info(&mut self) {
        // SAFETY: plain call.
        if let Ok(info) = unsafe { self.mft.GetOutputStreamInfo(0) } {
            self.provides_samples = info.dwFlags & (MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 | MFT_OUTPUT_STREAM_CAN_PROVIDE_SAMPLES.0 as u32) != 0;
            self.out_size = info.cbSize.max(1 << 16);
        }
    }

    fn start(&mut self) -> Result<()> {
        self.refresh_stream_info();
        self.drained = false;
        self.need_input = 0;
        // Hints for synchronous transforms (Microsoft's H.264 encoder fails
        // them with E_FAIL), but asynchronous ones only start on
        // START_OF_STREAM.
        // SAFETY: plain calls.
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0);
            let started = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
            if self.events.is_some() {
                started.map_err(|e| explain(&e, "Starting the encoder"))?;
            }
        }
        Ok(())
    }

    fn output(&mut self) -> Result<Out> {
        let sample = if self.provides_samples {
            None
        } else {
            // SAFETY: plain calls.
            unsafe {
                let s = MFCreateSample()?;
                s.AddBuffer(&MFCreateMemoryBuffer(self.out_size)?)?;
                Some(s)
            }
        };
        let mut buf = [MFT_OUTPUT_DATA_BUFFER { dwStreamID: 0, pSample: ManuallyDrop::new(sample), dwStatus: 0, pEvents: ManuallyDrop::new(None) }];
        let mut status = 0u32;
        // SAFETY: the buffer array outlives the call; its contents are taken
        // back (and released) right after.
        let r = unsafe { self.mft.ProcessOutput(0, &mut buf, &mut status) };
        let got = unsafe { ManuallyDrop::take(&mut buf[0].pSample) };
        drop(unsafe { ManuallyDrop::take(&mut buf[0].pEvents) });
        match r {
            Ok(()) => Ok(got.map(Out::Sample).unwrap_or(Out::NeedMore)),
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => Ok(Out::NeedMore),
            Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => Ok(Out::Changed),
            Err(e) => Err(explain(&e, "Media Foundation")),
        }
    }

    /// Handle queued events (asynchronous transforms): `wait` blocks until
    /// the transform takes input (or has drained). Outputs go to `on`.
    fn pump(&mut self, wait: bool, on: &mut dyn FnMut(&mut Transform, Out) -> Result<()>) -> Result<()> {
        let Some(events) = self.events.clone() else { return Ok(()) };
        loop {
            if wait && (self.need_input > 0 || self.drained) {
                return Ok(());
            }
            let flags = if wait { MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0) } else { MF_EVENT_FLAG_NO_WAIT };
            // SAFETY: plain call.
            let ev = match unsafe { events.GetEvent(flags) } {
                Ok(e) => e,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(()),
                Err(e) => return Err(explain(&e, "Media Foundation")),
            };
            let kind = unsafe { ev.GetType() }.unwrap_or(0) as i32;
            if kind == METransformNeedInput.0 {
                self.need_input += 1;
            } else if kind == METransformHaveOutput.0 {
                let o = self.output()?;
                on(self, o)?;
            } else if kind == METransformDrainComplete.0 {
                self.drained = true;
            }
        }
    }

    /// Give the transform one sample; outputs that come out meanwhile go
    /// to `on`.
    fn input(&mut self, sample: &IMFSample, on: &mut dyn FnMut(&mut Transform, Out) -> Result<()>) -> Result<()> {
        if self.events.is_some() {
            self.pump(true, on)?;
            self.need_input = self.need_input.saturating_sub(1);
            // SAFETY: plain call.
            unsafe { self.mft.ProcessInput(0, sample, 0) }.map_err(|e| explain(&e, "Media Foundation"))?;
            return self.pump(false, on);
        }
        for _ in 0..64 {
            // SAFETY: plain call.
            match unsafe { self.mft.ProcessInput(0, sample, 0) } {
                Ok(()) => break,
                Err(e) if e.code() == MF_E_NOTACCEPTING => self.drain_outputs(on)?,
                Err(e) => return Err(explain(&e, "Media Foundation")),
            }
        }
        self.drain_outputs(on)
    }

    /// Synchronous transforms: take every output that's ready.
    fn drain_outputs(&mut self, on: &mut dyn FnMut(&mut Transform, Out) -> Result<()>) -> Result<()> {
        for _ in 0..256 {
            match self.output()? {
                Out::NeedMore => return Ok(()),
                o => on(self, o)?,
            }
        }
        Ok(())
    }

    /// End of input: everything still inside comes out.
    fn finish(&mut self, on: &mut dyn FnMut(&mut Transform, Out) -> Result<()>) -> Result<()> {
        // SAFETY: plain calls.
        unsafe {
            let _ = self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0)?;
        }
        if self.events.is_some() {
            self.drained = false;
            self.need_input = 0;
            while !self.drained {
                self.pump(true, on)?;
                if self.need_input > 0 && !self.drained {
                    // Input requests during a drain don't count.
                    self.need_input = 0;
                }
            }
            Ok(())
        } else {
            self.drain_outputs(on)
        }
    }
}

// ---- video encoding ---------------------------------------------------------------------

pub struct MfVideoEncoder {
    t: Transform,
    api: Option<ICodecAPI>,
    codec: VideoCodec,
    width: u32,
    height: u32,
    frame_100ns: i64,
    sink: VideoSink,
    config: Option<Arc<VideoConfig>>,
    header: Option<Vec<u8>>,
}

// Media Foundation objects are free-threaded; the encoder is used from one
// thread at a time.
unsafe impl Send for MfVideoEncoder {}

impl MfVideoEncoder {
    pub fn new(s: &VideoSettings, backend: &str, sink: VideoSink) -> Result<Self> {
        mf()?;
        let sub = video_subtype(s.codec).ok_or_else(|| anyhow!("{:?} can't be encoded on Windows", s.codec))?;
        let found = encoders(s.codec).into_iter().find(|f| backend_of(f) == backend || (backend == "mf-hw" && f.hardware));
        let found = found.ok_or_else(|| anyhow!("encoder \"{backend}\" isn't available"))?;
        // SAFETY: plain Media Foundation calls on valid objects.
        unsafe {
            let mft: IMFTransform = found.act.ActivateObject().map_err(|e| explain(&e, "Starting the encoder"))?;
            let mut t = Transform::new(mft)?;
            let api = t.mft.cast::<ICodecAPI>().ok();
            let gop = (s.fps * 2).max(1);
            let bps = s.bitrate_kbps.saturating_mul(1000);
            let set = |api: &ICodecAPI| {
                let stream = s.purpose == Purpose::Stream;
                let mode = (if stream { eAVEncCommonRateControlMode_CBR.0 } else { eAVEncCommonRateControlMode_PeakConstrainedVBR.0 }) as u32;
                let _ = api.SetValue(&CODECAPI_AVEncCommonRateControlMode, &variant_u32(mode));
                let _ = api.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(bps));
                if !stream {
                    let _ = api.SetValue(&CODECAPI_AVEncCommonMaxBitRate, &variant_u32(bps.saturating_mul(3) / 2));
                }
                let _ = api.SetValue(&CODECAPI_AVEncMPVGOPSize, &variant_u32(gop));
                let _ = api.SetValue(&CODECAPI_AVEncMPVDefaultBPictureCount, &variant_u32(0));
                let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &variant_bool(stream));
            };
            if let Some(a) = &api {
                set(a);
            }
            if s.purpose == Purpose::Stream {
                if let Ok(a) = t.mft.GetAttributes() {
                    let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
                }
            }
            // Output first (encoders want to know what they make), then input.
            let out = MFCreateMediaType()?;
            out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            out.SetGUID(&MF_MT_SUBTYPE, &sub)?;
            out.SetUINT32(&MF_MT_AVG_BITRATE, bps)?;
            out.SetUINT64(&MF_MT_FRAME_SIZE, pack(s.width, s.height))?;
            out.SetUINT64(&MF_MT_FRAME_RATE, pack(s.fps.max(1), 1))?;
            out.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            out.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            let profile = (if s.codec == VideoCodec::Hevc { eAVEncH265VProfile_Main_420_8.0 } else { eAVEncH264VProfile_High.0 }) as u32;
            out.SetUINT32(&MF_MT_MPEG2_PROFILE, profile)?;
            t.mft.SetOutputType(0, &out, 0).map_err(|e| explain(&e, "The encoder refused the output format"))?;
            let input = (0..32)
                .filter_map(|i| t.mft.GetInputAvailableType(0, i).ok())
                .find(|ty| ty.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12));
            let input = match input {
                Some(ty) => ty,
                None => {
                    let ty = MFCreateMediaType()?;
                    ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
                    ty.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
                    ty
                }
            };
            input.SetUINT64(&MF_MT_FRAME_SIZE, pack(s.width, s.height))?;
            input.SetUINT64(&MF_MT_FRAME_RATE, pack(s.fps.max(1), 1))?;
            input.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
            input.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
            t.mft.SetInputType(0, &input, 0).map_err(|e| explain(&e, "The encoder refused NV12 input"))?;
            if let Some(a) = &api {
                set(a);
            }
            t.start()?;
            Ok(MfVideoEncoder {
                t,
                api,
                codec: s.codec,
                width: s.width,
                height: s.height,
                frame_100ns: 10_000_000 / s.fps.max(1) as i64,
                sink,
                config: None,
                header: None,
            })
        }
    }

    fn input_sample(&self, frame: &VideoFrame, pts: Ns) -> Result<IMFSample> {
        let (w, h) = (self.width as usize, self.height as usize);
        let scaled;
        let frame = if (frame.width, frame.height) != (self.width, self.height) {
            scaled = frame.scaled(self.width, self.height);
            &scaled
        } else {
            frame
        };
        let mut data = vec![0u8; w * h * 3 / 2];
        frame.pixels.with_planes(|p| {
            for r in 0..h {
                data[r * w..r * w + w].copy_from_slice(&p.y[r * p.y_stride..r * p.y_stride + w]);
            }
            let base = w * h;
            for r in 0..h / 2 {
                data[base + r * w..base + r * w + w].copy_from_slice(&p.uv[r * p.uv_stride..r * p.uv_stride + w]);
            }
        });
        sample_with(&data, pts / 100, self.frame_100ns)
    }
}

/// What the encoder made, as packets.
fn deliver(t: &mut Transform, out: Out, codec: VideoCodec, size: (u32, u32), header: &mut Option<Vec<u8>>, config: &mut Option<Arc<VideoConfig>>, sink: &VideoSink) -> Result<()> {
    match out {
        Out::NeedMore => Ok(()),
        Out::Changed => {
            // The encoder settled its format: take it, and its header.
            // SAFETY: plain calls.
            unsafe {
                if let Ok(ty) = t.mft.GetOutputAvailableType(0, 0) {
                    t.mft.SetOutputType(0, &ty, 0).map_err(|e| explain(&e, "Encoder format change"))?;
                }
            }
            t.refresh_stream_info();
            Ok(())
        }
        Out::Sample(s) => {
            if header.is_none() {
                // SAFETY: reading an optional attribute.
                *header = unsafe {
                    t.mft.GetOutputCurrentType(0).ok().and_then(|ty| {
                        let n = ty.GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER).ok()?;
                        let mut v = vec![0u8; n as usize];
                        ty.GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut v, None).ok()?;
                        Some(v)
                    })
                };
            }
            let pts = unsafe { s.GetSampleTime() }.unwrap_or(0) * 100;
            let data = sample_bytes(&s)?;
            if let Some(p) = h264::packet_from_annexb(codec, size, &data, header.as_deref(), pts, config) {
                sink(p);
            }
            Ok(())
        }
    }
}

impl VideoEncoder for MfVideoEncoder {
    fn encode(&mut self, frame: &VideoFrame, pts: Ns, force_keyframe: bool) -> Result<()> {
        if force_keyframe {
            if let Some(api) = &self.api {
                // SAFETY: plain call.
                let _ = unsafe { api.SetValue(&CODECAPI_AVEncVideoForceKeyFrame, &variant_u32(1)) };
            }
        }
        let sample = self.input_sample(frame, pts)?;
        let (codec, size, sink) = (self.codec, (self.width, self.height), self.sink.clone());
        let (header, config) = (&mut self.header, &mut self.config);
        self.t.input(&sample, &mut |t, o| deliver(t, o, codec, size, header, config, &sink))
    }

    fn set_bitrate(&mut self, kbps: u32) -> bool {
        let Some(api) = &self.api else { return false };
        // SAFETY: a codec property on our own encoder (most hardware
        // encoders take a new mean bitrate while running).
        unsafe { api.SetValue(&CODECAPI_AVEncCommonMeanBitRate, &variant_u32(kbps.saturating_mul(1000))).is_ok() }
    }

    fn finish(&mut self) -> Result<()> {
        let (codec, size, sink) = (self.codec, (self.width, self.height), self.sink.clone());
        let (header, config) = (&mut self.header, &mut self.config);
        self.t.finish(&mut |t, o| deliver(t, o, codec, size, header, config, &sink))
    }
}

// ---- AAC encoding --------------------------------------------------------------------------

/// Windows' AAC encoder takes 96, 128, 160 or 192 kbit/s.
fn aac_bytes_per_second(kbps: u32) -> u32 {
    let allowed = [12_000u32, 16_000, 20_000, 24_000];
    let want = kbps * 1000 / 8;
    allowed.iter().rev().find(|b| **b <= want).copied().unwrap_or(allowed[0])
}

fn new_aac_encoder(rate: u32, channels: u16, kbps: u32) -> Result<Transform> {
    mf()?;
    let found = enumerate(
        MFT_CATEGORY_AUDIO_ENCODER,
        MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0),
        Some((MFMediaType_Audio, MFAudioFormat_PCM)),
        Some((MFMediaType_Audio, MFAudioFormat_AAC)),
    );
    let found = found.into_iter().next().ok_or_else(|| anyhow!("Windows' AAC encoder isn't available"))?;
    // SAFETY: plain Media Foundation calls on valid objects.
    unsafe {
        let mft: IMFTransform = found.act.ActivateObject().map_err(|e| explain(&e, "Starting the AAC encoder"))?;
        let t = Transform::new(mft)?;
        let input = MFCreateMediaType()?;
        input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        input.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
        input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32)?;
        input.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, channels as u32 * 2)?;
        input.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, rate * channels as u32 * 2)?;
        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
        out.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32)?;
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, aac_bytes_per_second(kbps))?;
        out.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?; // raw frames
        out.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29)?;
        // The AAC encoder wants its input type first.
        if t.mft.SetInputType(0, &input, 0).is_ok() {
            t.mft.SetOutputType(0, &out, 0).map_err(|e| explain(&e, "The AAC encoder refused the format"))?;
        } else {
            t.mft.SetOutputType(0, &out, 0).map_err(|e| explain(&e, "The AAC encoder refused the format"))?;
            t.mft.SetInputType(0, &input, 0).map_err(|e| explain(&e, "The AAC encoder refused the format"))?;
        }
        let mut t = t;
        t.start()?;
        Ok(t)
    }
}

pub struct MfAacEncoder {
    t: Transform,
    config: AudioConfig,
    channels: usize,
    rate: u32,
    input_frames: u64,
    output_frames: u64,
}

unsafe impl Send for MfAacEncoder {}

impl MfAacEncoder {
    pub fn new(rate: u32, channels: u16, kbps: u32) -> Result<Self> {
        let channels = channels.clamp(1, 2);
        let rate = if rate == 44_100 { 44_100 } else { 48_000 };
        let t = new_aac_encoder(rate, channels, kbps)?;
        let config = AudioConfig { rate, channels, asc: h264::aac_asc(rate, channels), priming: aac_priming() };
        Ok(MfAacEncoder { t, config, channels: channels as usize, rate, input_frames: 0, output_frames: 0 })
    }

    fn run(&mut self, pcm: &[i16], limit: Option<u64>) -> Result<Vec<Vec<u8>>> {
        let mut frames = Vec::new();
        let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
        let n = (pcm.len() / self.channels) as i64;
        let time = self.input_frames as i64 * 10_000_000 / self.rate as i64;
        let sample = sample_with(&bytes, time, n * 10_000_000 / self.rate as i64)?;
        let mut produced = Vec::new();
        self.t.input(&sample, &mut |_, o| {
            if let Out::Sample(s) = o {
                produced.push(sample_bytes(&s)?);
            }
            Ok(())
        })?;
        for f in produced {
            if f.is_empty() {
                continue;
            }
            if limit.is_none_or(|l| self.output_frames * AAC_FRAME as u64 <= l) {
                frames.push(f);
            }
            self.output_frames += 1;
        }
        Ok(frames)
    }
}

fn to_i16(samples: &[f32]) -> Vec<i16> {
    samples.iter().map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}

impl AudioEncoder for MfAacEncoder {
    fn config(&self) -> AudioConfig {
        self.config.clone()
    }

    fn push(&mut self, samples: &[f32]) -> Result<Vec<Vec<u8>>> {
        if samples.is_empty() {
            return Ok(Vec::new());
        }
        let out = self.run(&to_i16(samples), None)?;
        self.input_frames += (samples.len() / self.channels) as u64;
        Ok(out)
    }

    fn finish(&mut self) -> Result<Vec<Vec<u8>>> {
        // Silence through the encoder delay, then drain; only frames that
        // cover real input are kept.
        let limit = self.input_frames + self.config.priming as u64;
        let mut out = self.run(&vec![0i16; 2048 * self.channels], Some(limit))?;
        let mut rest = Vec::new();
        self.t.finish(&mut |_, o| {
            if let Out::Sample(s) = o {
                rest.push(sample_bytes(&s)?);
            }
            Ok(())
        })?;
        for f in rest.into_iter().filter(|f| !f.is_empty()) {
            if self.output_frames * AAC_FRAME as u64 <= limit {
                out.push(f);
            }
            self.output_frames += 1;
        }
        Ok(out)
    }
}

/// Windows' AAC encoder's delay, in samples: measured once by encoding a
/// click and finding it after decoding (falls back to the usual 1024).
pub fn aac_priming() -> u32 {
    static PRIMING: OnceLock<u32> = OnceLock::new();
    *PRIMING.get_or_init(|| measure_priming().unwrap_or(1024))
}

fn measure_priming() -> Result<u32> {
    const CLICK: usize = 4096 + 300;
    let rate = 48_000;
    let mut t = new_aac_encoder(rate, 1, 128)?;
    let mut pcm = vec![0i16; 16 * 1024];
    pcm[CLICK] = 30_000;
    let mut frames = Vec::new();
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let sample = sample_with(&bytes, 0, pcm.len() as i64 * 10_000_000 / rate as i64)?;
    {
        let mut collect = |_: &mut Transform, o: Out| -> Result<()> {
            if let Out::Sample(s) = o {
                frames.push(sample_bytes(&s)?);
            }
            Ok(())
        };
        t.input(&sample, &mut collect)?;
        t.finish(&mut collect)?;
    }
    let mut dec = MfAacDecoder::new(&h264::aac_asc(rate, 1))?;
    let mut out = Vec::new();
    for f in frames.iter().filter(|f| !f.is_empty()) {
        out.extend(dec.decode(f)?);
    }
    let peak = out.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map(|(i, _)| i).ok_or_else(|| anyhow!("no output"))?;
    let shift = peak as i64 - CLICK as i64;
    if (0..=4096).contains(&shift) {
        Ok(shift as u32)
    } else {
        bail!("unexpected AAC delay {shift}")
    }
}

// ---- decoding -----------------------------------------------------------------------------

pub struct MfVideoDecoder {
    t: Transform,
    params: Vec<Vec<u8>>,
    hevc: bool,
    sent_params: bool,
    /// Decoded picture size (coded) and the visible part of it.
    coded: (u32, u32),
    visible: (u32, u32),
    output: (u32, u32),
}

unsafe impl Send for MfVideoDecoder {}

fn set_nv12_output(t: &mut Transform) -> Result<(u32, u32)> {
    // SAFETY: plain calls.
    unsafe {
        let ty = (0..64)
            .filter_map(|i| t.mft.GetOutputAvailableType(0, i).ok())
            .find(|ty| ty.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12))
            .ok_or_else(|| anyhow!("the decoder can't produce NV12"))?;
        t.mft.SetOutputType(0, &ty, 0).map_err(|e| explain(&e, "Decoder output"))?;
        t.refresh_stream_info();
        Ok(ty.GetUINT64(&MF_MT_FRAME_SIZE).map(unpack).unwrap_or((0, 0)))
    }
}

impl MfVideoDecoder {
    pub fn new(s: &DecoderSetup) -> Result<Self> {
        mf()?;
        let sub = video_subtype(s.codec).ok_or_else(|| anyhow!("ProRes playback needs a Mac"))?;
        let found = enumerate(
            MFT_CATEGORY_VIDEO_DECODER,
            MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0),
            Some((MFMediaType_Video, sub)),
            Some((MFMediaType_Video, MFVideoFormat_NV12)),
        );
        let found = found.into_iter().next().ok_or_else(|| {
            if s.codec == VideoCodec::Hevc {
                anyhow!("HEVC playback needs Microsoft's HEVC Video Extensions (from the Microsoft Store)")
            } else {
                anyhow!("Windows' H.264 decoder isn't available")
            }
        })?;
        // SAFETY: plain Media Foundation calls on valid objects.
        unsafe {
            let mft: IMFTransform = found.act.ActivateObject().map_err(|e| explain(&e, "Starting the decoder"))?;
            if let Ok(a) = mft.GetAttributes() {
                let _ = a.SetUINT32(&MF_LOW_LATENCY, 1);
            }
            if let Ok(api) = mft.cast::<ICodecAPI>() {
                let _ = api.SetValue(&CODECAPI_AVLowLatencyMode, &variant_bool(true));
            }
            let mut t = Transform::new(mft)?;
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            input.SetGUID(&MF_MT_SUBTYPE, &sub)?;
            if s.width > 0 && s.height > 0 {
                input.SetUINT64(&MF_MT_FRAME_SIZE, pack(s.width, s.height))?;
            }
            t.mft.SetInputType(0, &input, 0).map_err(|e| explain(&e, "The decoder refused the stream"))?;
            let coded = set_nv12_output(&mut t)?;
            t.start()?;
            let visible = if s.width > 0 && s.height > 0 { (s.width, s.height) } else { coded };
            Ok(MfVideoDecoder { t, params: s.parameter_sets.clone(), hevc: s.codec == VideoCodec::Hevc, sent_params: false, coded, visible, output: s.output })
        }
    }
}

fn decoded(t: &mut Transform, o: Out, coded: &mut (u32, u32), visible: (u32, u32), output: (u32, u32), frames: &mut Vec<VideoFrame>) -> Result<()> {
    match o {
        Out::NeedMore => {}
        Out::Changed => *coded = set_nv12_output(t)?,
        Out::Sample(s) => {
            let pts = unsafe { s.GetSampleTime() }.unwrap_or(0) * 100;
            let (w, h) = (visible.0.min(coded.0.max(2)) & !1, visible.1.min(coded.1.max(2)) & !1);
            if let Some((y, uv)) = planes(&s, w, h) {
                let f = VideoFrame::new_nv12(w, h, pts, y, uv, 0);
                frames.push(if (w, h) == output || output.0 == 0 { f } else { f.scaled(output.0, output.1) });
            }
        }
    }
    Ok(())
}

impl VideoDecoder for MfVideoDecoder {
    fn decode(&mut self, data: &[u8], pts: Ns, out: &mut Vec<VideoFrame>) -> Result<()> {
        let nals = h264::split_avcc(data);
        let key = nals.iter().any(|n| if self.hevc { (16..=21).contains(&h264::hevc_nal_type(n)) } else { h264::h264_nal_type(n) == h264::NAL_IDR });
        let mut annexb = Vec::with_capacity(data.len() + 64);
        if key || !self.sent_params {
            annexb.extend(h264::to_annexb(self.params.iter().map(|p| p.as_slice())));
            self.sent_params = true;
        }
        annexb.extend(h264::to_annexb(nals));
        let sample = sample_with(&annexb, pts / 100, 0)?;
        let (visible, output) = (self.visible, self.output);
        let coded = &mut self.coded;
        self.t.input(&sample, &mut |t, o| decoded(t, o, coded, visible, output, out))
    }

    fn flush(&mut self, out: &mut Vec<VideoFrame>) -> Result<()> {
        let (visible, output) = (self.visible, self.output);
        let coded = &mut self.coded;
        self.t.finish(&mut |t, o| decoded(t, o, coded, visible, output, out))?;
        // Ready for more after a flush.
        self.t.start()?;
        self.sent_params = false;
        Ok(())
    }
}

pub struct MfAacDecoder {
    t: Transform,
    rate: u32,
    channels: u16,
}

unsafe impl Send for MfAacDecoder {}

/// Sample rate and channels from an AudioSpecificConfig.
fn asc_format(asc: &[u8]) -> (u32, u16) {
    const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
    if asc.len() < 2 {
        return (48_000, 2);
    }
    let v = u16::from_be_bytes([asc[0], asc[1]]);
    let idx = ((v >> 7) & 0xf) as usize;
    let ch = ((v >> 3) & 0xf).clamp(1, 8);
    (RATES.get(idx).copied().unwrap_or(48_000), ch)
}

impl MfAacDecoder {
    pub fn new(asc: &[u8]) -> Result<Self> {
        mf()?;
        let (rate, channels) = asc_format(asc);
        let found = enumerate(
            MFT_CATEGORY_AUDIO_DECODER,
            MFT_ENUM_FLAG(MFT_ENUM_FLAG_SYNCMFT.0 | MFT_ENUM_FLAG_SORTANDFILTER.0),
            Some((MFMediaType_Audio, MFAudioFormat_AAC)),
            Some((MFMediaType_Audio, MFAudioFormat_Float)),
        );
        let found = found.into_iter().next().ok_or_else(|| anyhow!("Windows' AAC decoder isn't available"))?;
        // SAFETY: plain Media Foundation calls on valid objects.
        unsafe {
            let mft: IMFTransform = found.act.ActivateObject().map_err(|e| explain(&e, "Starting the AAC decoder"))?;
            let mut t = Transform::new(mft)?;
            let input = MFCreateMediaType()?;
            input.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            input.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC)?;
            input.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate)?;
            input.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32)?;
            input.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0)?;
            // HEAACWAVEINFO after its WAVEFORMATEX: payload type, profile,
            // struct type and reserved fields, then the AudioSpecificConfig.
            let mut user = vec![0u8; 12];
            user[2..4].copy_from_slice(&0xFEu16.to_le_bytes());
            user.extend_from_slice(asc);
            input.SetBlob(&MF_MT_USER_DATA, &user)?;
            t.mft.SetInputType(0, &input, 0).map_err(|e| explain(&e, "The AAC decoder refused the stream"))?;
            let out = (0..32)
                .filter_map(|i| t.mft.GetOutputAvailableType(0, i).ok())
                .find(|ty| ty.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFAudioFormat_Float))
                .ok_or_else(|| anyhow!("the AAC decoder can't produce float audio"))?;
            t.mft.SetOutputType(0, &out, 0).map_err(|e| explain(&e, "AAC decoder output"))?;
            t.start()?;
            Ok(MfAacDecoder { t, rate, channels })
        }
    }
}

impl AudioDecoder for MfAacDecoder {
    fn decode(&mut self, frame: &[u8]) -> Result<Vec<f32>> {
        let sample = sample_with(frame, 0, 0)?;
        let mut pcm = Vec::new();
        self.t.input(&sample, &mut |_, o| {
            if let Out::Sample(s) = o {
                let bytes = sample_bytes(&s)?;
                pcm.extend(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
            }
            Ok(())
        })?;
        Ok(pcm)
    }

    fn rate(&self) -> u32 {
        self.rate
    }

    fn channels(&self) -> u16 {
        self.channels
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aac_bitrates_are_ones_windows_takes() {
        assert_eq!(aac_bytes_per_second(256), 24_000);
        assert_eq!(aac_bytes_per_second(160), 20_000);
        assert_eq!(aac_bytes_per_second(64), 12_000);
        assert_eq!(asc_format(&h264::aac_asc(44_100, 1)), (44_100, 1));
    }

    /// Needs Windows: AAC round trip and the measured encoder delay.
    #[test]
    fn aac_round_trip() {
        let mut enc = MfAacEncoder::new(48_000, 2, 192).unwrap();
        let tone: Vec<f32> = (0..48_000).flat_map(|i| [(i as f32 * 0.05).sin() * 0.5; 2]).collect();
        let mut frames = enc.push(&tone).unwrap();
        frames.extend(enc.finish().unwrap());
        assert!((46..=50).contains(&frames.len()), "{} frames", frames.len());
        let priming = enc.config().priming;
        assert!(priming <= 4096, "delay {priming}");
        let mut dec = MfAacDecoder::new(&enc.config().asc).unwrap();
        let pcm: Vec<f32> = frames.iter().flat_map(|f| dec.decode(f).unwrap()).collect();
        assert!(pcm.len() >= 90_000);
        eprintln!("Windows AAC encoder delay: {priming} samples");
    }

    /// Needs Windows: H.264 with the best encoder there is, decoded back.
    #[test]
    fn h264_round_trip() {
        use crate::media::testsrc::TestPattern;
        let packets = Arc::new(Mutex::new(Vec::new()));
        let p2 = packets.clone();
        let backend = video_backends(VideoCodec::H264)[0];
        let s = VideoSettings { codec: VideoCodec::H264, width: 640, height: 360, fps: 30, bitrate_kbps: 2000, purpose: Purpose::Record, backend: backend.into() };
        let mut enc = MfVideoEncoder::new(&s, backend, Arc::new(move |p| p2.lock().unwrap().push(p))).unwrap();
        let mut tp = TestPattern::new("ball", 640, 360);
        for i in 0..60i64 {
            let pts = i * 1_000_000_000 / 30;
            let img = tp.render(pts, 30);
            enc.encode(&VideoFrame::new_nv12(640, 360, pts, img.y, img.uv, i as u64), pts, i == 30).unwrap();
        }
        enc.finish().unwrap();
        let packets = packets.lock().unwrap();
        assert!(packets.len() >= 55, "{backend}: {} packets", packets.len());
        assert!(packets[0].keyframe);
        let setup = DecoderSetup { codec: VideoCodec::H264, parameter_sets: packets[0].config.parameter_sets.clone(), width: 640, height: 360, output: (640, 360) };
        let mut dec = MfVideoDecoder::new(&setup).unwrap();
        let mut frames = Vec::new();
        for p in packets.iter() {
            dec.decode(&p.data, p.pts, &mut frames).unwrap();
        }
        dec.flush(&mut frames).unwrap();
        assert!(frames.len() >= 55, "{} frames decoded", frames.len());
        eprintln!("H.264 with {backend}: {} packets, {} frames back", packets.len(), frames.len());
    }
}
