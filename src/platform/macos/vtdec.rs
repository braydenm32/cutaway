//! Video decoding with VideoToolbox: H.264, HEVC and ProRes, in hardware,
//! scaled to the size wanted on the way out.

use crate::util::LockExt;
use super::pixel_buffer::{PixelBuffer, NV12_VIDEO_RANGE};
use super::{cmtime_ns, ns_cmtime};
use crate::engine::decode::{DecoderSetup, VideoDecoder};
use crate::media::{Ns, Pixels, ProRes, VideoCodec, VideoFrame};
use anyhow::{bail, Result};
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::CMTime;
use objc2_core_video::{kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, CVPixelBuffer};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

#[repr(C)]
struct CallbackRecord {
    callback: unsafe extern "C-unwind" fn(*mut c_void, *mut c_void, i32, u32, *mut c_void, CMTime, CMTime),
    refcon: *mut c_void,
}

#[repr(C)]
struct SampleTiming {
    duration: CMTime,
    pts: CMTime,
    dts: CMTime,
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C-unwind" {
    fn CMVideoFormatDescriptionCreateFromH264ParameterSets(alloc: *const c_void, count: usize, ptrs: *const *const u8, sizes: *const usize, nal_len: i32, out: *mut *const c_void) -> i32;
    fn CMVideoFormatDescriptionCreateFromHEVCParameterSets(
        alloc: *const c_void,
        count: usize,
        ptrs: *const *const u8,
        sizes: *const usize,
        nal_len: i32,
        extensions: *const c_void,
        out: *mut *const c_void,
    ) -> i32;
    fn CMVideoFormatDescriptionCreate(alloc: *const c_void, codec: u32, width: i32, height: i32, extensions: *const c_void, out: *mut *const c_void) -> i32;
    fn CMBlockBufferCreateWithMemoryBlock(
        alloc: *const c_void,
        memory: *mut c_void,
        len: usize,
        block_alloc: *const c_void,
        custom: *const c_void,
        offset: usize,
        data_len: usize,
        flags: u32,
        out: *mut *mut c_void,
    ) -> i32;
    fn CMBlockBufferReplaceDataBytes(src: *const c_void, dest: *mut c_void, offset: usize, len: usize) -> i32;
    fn CMSampleBufferCreateReady(
        alloc: *const c_void,
        data: *mut c_void,
        format: *const c_void,
        num_samples: isize,
        num_timing: isize,
        timing: *const SampleTiming,
        num_sizes: isize,
        sizes: *const usize,
        out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "VideoToolbox", kind = "framework")]
extern "C-unwind" {
    fn VTDecompressionSessionCreate(alloc: *const c_void, format: *const c_void, spec: *const c_void, attrs: *const c_void, callback: *const CallbackRecord, out: *mut *mut c_void) -> i32;
    fn VTDecompressionSessionDecodeFrame(session: *mut c_void, sample: *mut c_void, flags: u32, frame_refcon: *mut c_void, info_out: *mut u32) -> i32;
    fn VTDecompressionSessionWaitForAsynchronousFrames(session: *mut c_void) -> i32;
    fn VTDecompressionSessionInvalidate(session: *mut c_void);
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(p: *const c_void);
    fn CFRetain(p: *const c_void) -> *const c_void;
}

/// Pictures handed back by VideoToolbox's callback.
struct Out {
    frames: Mutex<Vec<VideoFrame>>,
    error: Mutex<Option<String>>,
}

unsafe extern "C-unwind" fn on_frame(refcon: *mut c_void, _frame: *mut c_void, status: i32, _flags: u32, image: *mut c_void, pts: CMTime, _dur: CMTime) {
    let out = unsafe { &*(refcon as *const Out) };
    if status != 0 {
        *out.error.lock_ok() = Some(format!("VideoToolbox couldn't decode a frame (error {status})"));
        return;
    }
    if image.is_null() {
        return; // dropped
    }
    unsafe { CFRetain(image) };
    let pb = PixelBuffer::new(unsafe { CFRetained::from_raw(NonNull::new_unchecked(image as *mut CVPixelBuffer)) });
    if !pb.is_nv12() {
        return;
    }
    let f = VideoFrame { width: pb.width(), height: pb.height(), pts: cmtime_ns(pts).unwrap_or(0), pixels: Arc::new(Pixels::PixelBuffer(pb)), serial: 0 };
    out.frames.lock_ok().push(f);
}

pub struct VtDecoder {
    session: *mut c_void,
    format: *const c_void,
    out: Box<Out>,
}

// Used from one thread at a time; the callback only touches `Out`.
unsafe impl Send for VtDecoder {}

impl VtDecoder {
    pub fn new(s: &DecoderSetup) -> Result<Self> {
        let mut format: *const c_void = std::ptr::null();
        let status = match s.codec {
            VideoCodec::H264 | VideoCodec::Hevc => {
                if s.parameter_sets.is_empty() {
                    bail!("the video has no parameter sets");
                }
                let ptrs: Vec<*const u8> = s.parameter_sets.iter().map(|p| p.as_ptr()).collect();
                let sizes: Vec<usize> = s.parameter_sets.iter().map(|p| p.len()).collect();
                if s.codec == VideoCodec::H264 {
                    unsafe { CMVideoFormatDescriptionCreateFromH264ParameterSets(std::ptr::null(), ptrs.len(), ptrs.as_ptr(), sizes.as_ptr(), 4, &mut format) }
                } else {
                    unsafe { CMVideoFormatDescriptionCreateFromHEVCParameterSets(std::ptr::null(), ptrs.len(), ptrs.as_ptr(), sizes.as_ptr(), 4, std::ptr::null(), &mut format) }
                }
            }
            VideoCodec::ProRes(p) => {
                let fourcc = match p {
                    ProRes::Proxy => b"apco",
                    ProRes::Lt => b"apcs",
                    ProRes::Standard => b"apcn",
                    ProRes::Hq => b"apch",
                };
                unsafe { CMVideoFormatDescriptionCreate(std::ptr::null(), u32::from_be_bytes(*fourcc), s.width as i32, s.height as i32, std::ptr::null(), &mut format) }
            }
        };
        if status != 0 || format.is_null() {
            bail!("VideoToolbox doesn't accept this video's format (error {status})");
        }
        let (fmt, w, h) = (CFNumber::new_i32(NV12_VIDEO_RANGE as i32), CFNumber::new_i32(s.output.0 as i32), CFNumber::new_i32(s.output.1 as i32));
        let fmt_ref: &CFType = fmt.as_ref();
        let w_ref: &CFType = w.as_ref();
        let h_ref: &CFType = h.as_ref();
        let attrs = CFDictionary::<CFString, CFType>::from_slices(
            &[unsafe { kCVPixelBufferPixelFormatTypeKey }, unsafe { kCVPixelBufferWidthKey }, unsafe { kCVPixelBufferHeightKey }],
            &[fmt_ref, w_ref, h_ref],
        );
        let out = Box::new(Out { frames: Mutex::new(Vec::new()), error: Mutex::new(None) });
        let record = CallbackRecord { callback: on_frame, refcon: &*out as *const Out as *mut c_void };
        let mut session: *mut c_void = std::ptr::null_mut();
        let attrs_ptr: *const CFDictionary<CFString, CFType> = &*attrs;
        let status = unsafe { VTDecompressionSessionCreate(std::ptr::null(), format, std::ptr::null(), attrs_ptr.cast(), &record, &mut session) };
        if status != 0 || session.is_null() {
            unsafe { CFRelease(format) };
            bail!("VideoToolbox can't decode this video (error {status})");
        }
        Ok(VtDecoder { session, format, out })
    }

    fn take(&self, out: &mut Vec<VideoFrame>) -> Result<()> {
        out.append(&mut self.out.frames.lock_ok());
        match self.out.error.lock_ok().take() {
            Some(e) => bail!(e),
            None => Ok(()),
        }
    }
}

impl VideoDecoder for VtDecoder {
    fn decode(&mut self, data: &[u8], pts: Ns, out: &mut Vec<VideoFrame>) -> Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        // A block buffer that owns a copy of the data.
        let mut block: *mut c_void = std::ptr::null_mut();
        let st = unsafe { CMBlockBufferCreateWithMemoryBlock(std::ptr::null(), std::ptr::null_mut(), data.len(), std::ptr::null(), std::ptr::null(), 0, data.len(), 1, &mut block) };
        if st != 0 || block.is_null() {
            bail!("couldn't allocate a frame buffer (error {st})");
        }
        let st = unsafe { CMBlockBufferReplaceDataBytes(data.as_ptr().cast(), block, 0, data.len()) };
        if st != 0 {
            unsafe { CFRelease(block) };
            bail!("couldn't fill a frame buffer (error {st})");
        }
        let invalid = CMTime { value: 0, timescale: 0, flags: objc2_core_media::CMTimeFlags(0), epoch: 0 };
        let timing = SampleTiming { duration: invalid, pts: ns_cmtime(pts), dts: invalid };
        let size = data.len();
        let mut sample: *mut c_void = std::ptr::null_mut();
        let st = unsafe { CMSampleBufferCreateReady(std::ptr::null(), block, self.format, 1, 1, &timing, 1, &size, &mut sample) };
        unsafe { CFRelease(block) };
        if st != 0 || sample.is_null() {
            bail!("couldn't wrap a frame (error {st})");
        }
        let mut info = 0u32;
        let st = unsafe { VTDecompressionSessionDecodeFrame(self.session, sample, 0, std::ptr::null_mut(), &mut info) };
        unsafe { CFRelease(sample) };
        if st != 0 {
            bail!("VideoToolbox refused a frame (error {st})");
        }
        self.take(out)
    }

    fn flush(&mut self, out: &mut Vec<VideoFrame>) -> Result<()> {
        unsafe { VTDecompressionSessionWaitForAsynchronousFrames(self.session) };
        self.take(out)
    }
}

impl Drop for VtDecoder {
    fn drop(&mut self) {
        unsafe {
            VTDecompressionSessionWaitForAsynchronousFrames(self.session);
            VTDecompressionSessionInvalidate(self.session);
            CFRelease(self.session);
            CFRelease(self.format);
        }
    }
}
