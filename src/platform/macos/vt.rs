//! H.264 / HEVC / ProRes encoding with VideoToolbox (the Mac's media engine).

use crate::util::LockExt;
use super::pixel_buffer::{PixelBuffer, NV12_VIDEO_RANGE};
use super::{cmtime_ns, ns_cmtime};
use crate::engine::encode::{Purpose, VideoEncoder, VideoSettings, VideoSink};
use crate::media::{h264, Ns, Pixels, ProRes, VideoCodec, VideoConfig, VideoFrame, VideoPacket};
use anyhow::{anyhow, bail, Result};
use objc2_core_foundation::{CFArray, CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::{
    kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms, kCMSampleAttachmentKey_NotSync, kCMTimeInvalid, kCMVideoCodecType_AppleProRes422,
    kCMVideoCodecType_AppleProRes422HQ, kCMVideoCodecType_AppleProRes422LT, kCMVideoCodecType_AppleProRes422Proxy, kCMVideoCodecType_H264,
    kCMVideoCodecType_HEVC, CMSampleBuffer,
};
use objc2_core_video::{kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, CVPixelBufferPool};
use objc2_video_toolbox::{
    kVTCompressionPropertyKey_AllowFrameReordering, kVTCompressionPropertyKey_AverageBitRate, kVTCompressionPropertyKey_DataRateLimits,
    kVTCompressionPropertyKey_ExpectedFrameRate, kVTCompressionPropertyKey_MaxKeyFrameInterval,
    kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, kVTCompressionPropertyKey_ProfileLevel, kVTCompressionPropertyKey_RealTime,
    kVTEncodeFrameOptionKey_ForceKeyFrame, kVTProfileLevel_H264_High_AutoLevel, kVTProfileLevel_HEVC_Main_AutoLevel,
    kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder, VTCompressionSession, VTEncodeInfoFlags, VTSessionSetProperty,
};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

/// State shared with VideoToolbox's output callback.
struct Output {
    sink: VideoSink,
    codec: VideoCodec,
    width: u32,
    height: u32,
    config: Mutex<Option<Arc<VideoConfig>>>,
    error: Mutex<Option<String>>,
}

pub struct VtEncoder {
    session: CFRetained<VTCompressionSession>,
    pool: Option<CFRetained<CVPixelBufferPool>>,
    output: *mut Output,
    width: u32,
    height: u32,
    finished: bool,
}

// The session is used from one thread at a time; VideoToolbox's callback
// only touches `Output`, which is Sync.
unsafe impl Send for VtEncoder {}

fn cf<T: AsRef<CFType> + ?Sized>(v: &T) -> &CFType {
    v.as_ref()
}

/// The sample description atom (`avcC` / `hvcC`) VideoToolbox attached.
fn decoder_record(sample: &CMSampleBuffer, codec: VideoCodec) -> Option<Vec<u8>> {
    let fd = unsafe { sample.format_description() }?;
    let atoms = unsafe { fd.extension(kCMFormatDescriptionExtension_SampleDescriptionExtensionAtoms) }?;
    let dict = atoms.downcast_ref::<CFDictionary>()?;
    let dict: &CFDictionary<CFString, CFType> = unsafe { dict.cast_unchecked() };
    let key = CFString::from_static_str(if codec == VideoCodec::H264 { "avcC" } else { "hvcC" });
    let data = dict.get(&key)?;
    Some(data.downcast_ref::<CFData>()?.to_vec())
}

fn is_keyframe(sample: &CMSampleBuffer) -> bool {
    let Some(arr) = (unsafe { sample.sample_attachments_array(false) }) else { return true };
    let arr: &CFArray<CFDictionary<CFString, CFType>> = unsafe { arr.cast_unchecked() };
    let Some(att) = arr.get(0) else { return true };
    match att.get(unsafe { kCMSampleAttachmentKey_NotSync }) {
        None => true,
        Some(v) => !v.downcast_ref::<CFBoolean>().map(|b| b.as_bool()).unwrap_or(true),
    }
}

unsafe extern "C-unwind" fn on_output(refcon: *mut c_void, _frame: *mut c_void, status: i32, _flags: VTEncodeInfoFlags, sample: *mut CMSampleBuffer) {
    let out = unsafe { &*(refcon as *const Output) };
    if status != 0 {
        *out.error.lock_ok() = Some(format!("VideoToolbox couldn't encode a frame (error {status})"));
        return;
    }
    let Some(sample) = (unsafe { sample.as_ref() }) else { return }; // frame dropped
    if out.codec.is_prores() {
        // Self-contained frames: no decoder record.
        let mut cfg = out.config.lock_ok();
        if cfg.is_none() {
            *cfg = Some(Arc::new(VideoConfig { codec: out.codec, width: out.width, height: out.height, record: Vec::new(), parameter_sets: Vec::new() }));
        }
    } else if let Some(rec) = decoder_record(sample, out.codec) {
        let mut cfg = out.config.lock_ok();
        if cfg.as_ref().is_none_or(|c| c.record != rec) {
            let parameter_sets = if out.codec == VideoCodec::H264 { h264::parse_avcc(&rec) } else { h264::parse_hvcc(&rec) };
            *cfg = Some(Arc::new(VideoConfig { codec: out.codec, width: out.width, height: out.height, record: rec, parameter_sets }));
        }
    }
    let Some(config) = out.config.lock_ok().clone() else { return };
    let Some(block) = (unsafe { sample.data_buffer() }) else { return };
    let len = unsafe { block.data_length() };
    let mut data = vec![0u8; len];
    let st = unsafe { block.copy_data_bytes(0, len, NonNull::new(data.as_mut_ptr().cast()).unwrap()) };
    if st != 0 {
        return;
    }
    let pts = cmtime_ns(unsafe { sample.presentation_time_stamp() }).unwrap_or(0);
    let dts = cmtime_ns(unsafe { sample.decode_time_stamp() }).unwrap_or(pts);
    (out.sink)(VideoPacket { pts, dts, keyframe: is_keyframe(sample), data: Arc::new(data), config });
}

impl VtEncoder {
    pub fn new(s: &VideoSettings, sink: VideoSink) -> Result<Self> {
        let (codec_type, profile) = match s.codec {
            VideoCodec::H264 => (kCMVideoCodecType_H264, Some(unsafe { kVTProfileLevel_H264_High_AutoLevel })),
            VideoCodec::Hevc => (kCMVideoCodecType_HEVC, Some(unsafe { kVTProfileLevel_HEVC_Main_AutoLevel })),
            VideoCodec::ProRes(p) => (
                match p {
                    ProRes::Proxy => kCMVideoCodecType_AppleProRes422Proxy,
                    ProRes::Lt => kCMVideoCodecType_AppleProRes422LT,
                    ProRes::Standard => kCMVideoCodecType_AppleProRes422,
                    ProRes::Hq => kCMVideoCodecType_AppleProRes422HQ,
                },
                None,
            ),
        };
        let spec = CFDictionary::<CFString, CFType>::from_slices(
            &[unsafe { kVTVideoEncoderSpecification_EnableHardwareAcceleratedVideoEncoder }],
            &[cf(CFBoolean::new(true))],
        );
        let (fmt, w, h) = (CFNumber::new_i32(NV12_VIDEO_RANGE as i32), CFNumber::new_i32(s.width as i32), CFNumber::new_i32(s.height as i32));
        let source = CFDictionary::<CFString, CFType>::from_slices(
            &[unsafe { kCVPixelBufferPixelFormatTypeKey }, unsafe { kCVPixelBufferWidthKey }, unsafe { kCVPixelBufferHeightKey }],
            &[cf(&*fmt), cf(&*w), cf(&*h)],
        );
        let output = Box::into_raw(Box::new(Output {
            sink,
            codec: s.codec,
            width: s.width,
            height: s.height,
            config: Mutex::new(None),
            error: Mutex::new(None),
        }));
        let mut session: *mut VTCompressionSession = std::ptr::null_mut();
        let status = unsafe {
            VTCompressionSession::create(
                None,
                s.width as i32,
                s.height as i32,
                codec_type,
                Some(spec.as_opaque()),
                Some(source.as_opaque()),
                None,
                Some(on_output),
                output.cast(),
                NonNull::from(&mut session),
            )
        };
        let Some(session) = NonNull::new(session).filter(|_| status == 0) else {
            drop(unsafe { Box::from_raw(output) });
            if s.codec.is_prores() {
                bail!("this Mac can't encode {} (error {status}); ProRes needs Apple silicon or a Mac with an Afterburner/T2 chip", crate::engine::encode::codec_name(s.codec));
            }
            bail!("VideoToolbox can't encode {}×{} {:?} (error {status})", s.width, s.height, s.codec);
        };
        let session = unsafe { CFRetained::from_raw(session) };

        // Settings. Recording: quality, 1 s keyframe interval for easy
        // editing. Streaming: steady bitrate, 2 s keyframes (what YouTube,
        // Twitch and co. ask for).
        let set = |key: &CFString, value: &CFType| unsafe { VTSessionSetProperty(&session, key, Some(value)) };
        let fps = s.fps.max(1);
        let gop_secs = if s.purpose == Purpose::Stream { 2 } else { 1 };
        let bitrate = CFNumber::new_i32((s.bitrate_kbps.saturating_mul(1000)).min(i32::MAX as u32) as i32);
        let gop = CFNumber::new_i32((fps * gop_secs) as i32);
        let gop_dur = CFNumber::new_f64(gop_secs as f64);
        let rate = CFNumber::new_i32(fps as i32);
        unsafe {
            set(kVTCompressionPropertyKey_RealTime, cf(CFBoolean::new(true)));
            set(kVTCompressionPropertyKey_ExpectedFrameRate, cf(&*rate));
            // ProRes is intra-only at a fixed quality: no profile, GOP or bitrate.
            if let Some(profile) = profile {
                set(kVTCompressionPropertyKey_ProfileLevel, cf(profile));
                set(kVTCompressionPropertyKey_AllowFrameReordering, cf(CFBoolean::new(false)));
                set(kVTCompressionPropertyKey_AverageBitRate, cf(&*bitrate));
                set(kVTCompressionPropertyKey_MaxKeyFrameInterval, cf(&*gop));
                set(kVTCompressionPropertyKey_MaxKeyFrameIntervalDuration, cf(&*gop_dur));
            }
        }
        if s.purpose == Purpose::Stream {
            // Cap any one second at 1.5× the target so the stream stays
            // within the connection.
            let bytes = CFNumber::new_i64(s.bitrate_kbps as i64 * 1000 / 8 * 3 / 2);
            let secs = CFNumber::new_f64(1.0);
            let limits = CFArray::<CFType>::from_objects(&[cf(&*bytes), cf(&*secs)]);
            unsafe { set(kVTCompressionPropertyKey_DataRateLimits, cf(&*limits)) };
        }
        unsafe { session.prepare_to_encode_frames() };
        let pool = unsafe { session.pixel_buffer_pool() };
        Ok(VtEncoder { session, pool, output, width: s.width, height: s.height, finished: false })
    }

    fn check(&self) -> Result<()> {
        match unsafe { &*self.output }.error.lock_ok().take() {
            Some(e) => Err(anyhow!(e)),
            None => Ok(()),
        }
    }
}

impl VideoEncoder for VtEncoder {
    fn encode(&mut self, frame: &VideoFrame, pts: Ns, force_keyframe: bool) -> Result<()> {
        self.check()?;
        if frame.width != self.width || frame.height != self.height {
            bail!("frame is {}×{}, the encoder was set up for {}×{}", frame.width, frame.height, self.width, self.height);
        }
        let copied;
        let pb = match &*frame.pixels {
            Pixels::PixelBuffer(pb) => pb,
            Pixels::Nv12 { .. } => {
                copied = PixelBuffer::copy_from(self.pool.as_deref(), frame)?;
                &copied
            }
        };
        let props = force_keyframe.then(|| {
            CFDictionary::<CFString, CFType>::from_slices(&[unsafe { kVTEncodeFrameOptionKey_ForceKeyFrame }], &[cf(CFBoolean::new(true))])
        });
        let status = unsafe {
            self.session.encode_frame(
                pb.raw(),
                ns_cmtime(pts),
                kCMTimeInvalid,
                props.as_ref().map(|p| p.as_opaque()),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            bail!("VideoToolbox refused a frame (error {status})");
        }
        Ok(())
    }

    fn set_bitrate(&mut self, kbps: u32) -> bool {
        let bitrate = CFNumber::new_i32((kbps.saturating_mul(1000)).min(i32::MAX as u32) as i32);
        let bytes = CFNumber::new_i64(kbps as i64 * 1000 / 8 * 3 / 2);
        let secs = CFNumber::new_f64(1.0);
        let limits = CFArray::<CFType>::from_objects(&[cf(&*bytes), cf(&*secs)]);
        // SAFETY: properties set on our own live session, as at start.
        unsafe {
            let a = VTSessionSetProperty(&self.session, kVTCompressionPropertyKey_AverageBitRate, Some(cf(&*bitrate)));
            let _ = VTSessionSetProperty(&self.session, kVTCompressionPropertyKey_DataRateLimits, Some(cf(&*limits)));
            a == 0
        }
    }

    fn finish(&mut self) -> Result<()> {
        if !self.finished {
            self.finished = true;
            unsafe { self.session.complete_frames(kCMTimeInvalid) };
        }
        self.check()
    }
}

impl Drop for VtEncoder {
    fn drop(&mut self) {
        unsafe {
            if !self.finished {
                self.session.complete_frames(kCMTimeInvalid);
            }
            self.session.invalidate();
            drop(Box::from_raw(self.output));
        }
    }
}
