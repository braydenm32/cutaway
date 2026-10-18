//! CoreVideo pixel buffers: camera frames go straight from AVFoundation to
//! the GPU upload and to VideoToolbox without being copied.

use crate::media::{Planes, VideoFrame};
use anyhow::{bail, Result};
use objc2_core_foundation::CFRetained;
use objc2_core_video::{
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange, CVPixelBuffer, CVPixelBufferCreate, CVPixelBufferGetBaseAddressOfPlane,
    CVPixelBufferGetBytesPerRowOfPlane, CVPixelBufferGetHeight, CVPixelBufferGetHeightOfPlane, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
};
use std::ptr::NonNull;

pub const NV12_VIDEO_RANGE: u32 = kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange;

/// A retained `CVPixelBuffer` (NV12, video range).
pub struct PixelBuffer(CFRetained<CVPixelBuffer>);

// CoreVideo buffers are reference counted atomically and are only read
// (under a lock) once delivered, so sharing them between threads is fine.
unsafe impl Send for PixelBuffer {}
unsafe impl Sync for PixelBuffer {}

impl PixelBuffer {
    pub fn new(pb: CFRetained<CVPixelBuffer>) -> Self {
        PixelBuffer(pb)
    }

    pub fn raw(&self) -> &CVPixelBuffer {
        &self.0
    }

    pub fn width(&self) -> u32 {
        CVPixelBufferGetWidth(&self.0) as u32
    }

    pub fn height(&self) -> u32 {
        CVPixelBufferGetHeight(&self.0) as u32
    }

    pub fn is_nv12(&self) -> bool {
        CVPixelBufferGetPixelFormatType(&self.0) == NV12_VIDEO_RANGE
    }

    /// Run `f` with the planes mapped for reading.
    pub fn with_planes<R>(&self, f: impl FnOnce(Planes<'_>) -> R) -> R {
        let pb = &*self.0;
        unsafe { CVPixelBufferLockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly) };
        let plane = |i: usize| -> (&[u8], usize) {
            let base = CVPixelBufferGetBaseAddressOfPlane(pb, i) as *const u8;
            let stride = CVPixelBufferGetBytesPerRowOfPlane(pb, i);
            let rows = CVPixelBufferGetHeightOfPlane(pb, i);
            if base.is_null() {
                return (&[], stride);
            }
            (unsafe { std::slice::from_raw_parts(base, stride * rows) }, stride)
        };
        let (y, y_stride) = plane(0);
        let (uv, uv_stride) = plane(1);
        let r = f(Planes { y, y_stride, uv, uv_stride });
        unsafe { CVPixelBufferUnlockBaseAddress(pb, CVPixelBufferLockFlags::ReadOnly) };
        r
    }

    /// A buffer (from `pool` when there is one) holding a copy of `frame`,
    /// for encoding frames that live in ordinary memory like the program.
    pub fn copy_from(pool: Option<&CVPixelBufferPool>, frame: &VideoFrame) -> Result<PixelBuffer> {
        let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
        let status = match pool {
            Some(pool) => unsafe { CVPixelBufferPool::create_pixel_buffer(None, pool, NonNull::from(&mut out)) },
            None => unsafe {
                CVPixelBufferCreate(None, frame.width as usize, frame.height as usize, NV12_VIDEO_RANGE, None, NonNull::from(&mut out))
            },
        };
        let Some(ptr) = NonNull::new(out) else { bail!("couldn't get a pixel buffer (CoreVideo error {status})") };
        let pb = unsafe { CFRetained::from_raw(ptr) };
        unsafe { CVPixelBufferLockBaseAddress(&pb, CVPixelBufferLockFlags(0)) };
        let h = frame.height as usize;
        let w = frame.width as usize;
        frame.pixels.with_planes(|src| {
            for (plane, (data, stride, rows)) in [(src.y, src.y_stride, h), (src.uv, src.uv_stride, h / 2)].into_iter().enumerate() {
                let dst = CVPixelBufferGetBaseAddressOfPlane(&pb, plane) as *mut u8;
                let dst_stride = CVPixelBufferGetBytesPerRowOfPlane(&pb, plane);
                let dst_rows = CVPixelBufferGetHeightOfPlane(&pb, plane).min(rows);
                if dst.is_null() {
                    continue;
                }
                let n = w.min(dst_stride).min(stride);
                for r in 0..dst_rows {
                    if (r + 1) * stride > data.len() && r * stride + n > data.len() {
                        break;
                    }
                    unsafe { std::ptr::copy_nonoverlapping(data.as_ptr().add(r * stride), dst.add(r * dst_stride), n) };
                }
            }
        });
        unsafe { CVPixelBufferUnlockBaseAddress(&pb, CVPixelBufferLockFlags(0)) };
        Ok(PixelBuffer(pb))
    }
}
