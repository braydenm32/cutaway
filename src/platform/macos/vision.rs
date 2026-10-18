//! Finding people with Apple's Vision framework (on the Neural Engine):
//! faces and upper bodies for auto-framing, a person mask for background
//! removal.

use super::pixel_buffer::PixelBuffer;
use crate::media::VideoFrame;
use crate::platform::vision::{Detection, Detector, Mask, Segmenter};
use anyhow::{anyhow, Result};
use objc2::rc::{autoreleasepool, Retained};
use objc2::AllocAnyThread;
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress,
    CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSArray, NSDictionary};
use objc2_vision::{
    VNDetectFaceRectanglesRequest, VNDetectHumanRectanglesRequest, VNGeneratePersonSegmentationRequest, VNGeneratePersonSegmentationRequestQualityLevel, VNImageRequestHandler,
    VNRequest,
};

/// Vision is quick with a smaller copy, and doesn't need more to find people.
fn small_buffer(frame: &VideoFrame, width: u32) -> Result<PixelBuffer> {
    let w = width.min(frame.width) & !1;
    let h = ((frame.height as u64 * w as u64 / frame.width.max(1) as u64) as u32).max(2) & !1;
    PixelBuffer::copy_from(None, &frame.scaled(w, h))
}

fn handler(pb: &CVPixelBuffer) -> Retained<VNImageRequestHandler> {
    unsafe { VNImageRequestHandler::initWithCVPixelBuffer_options(VNImageRequestHandler::alloc(), pb, &NSDictionary::new()) }
}

fn as_request<T: objc2::Message>(r: &Retained<T>) -> Retained<VNRequest> {
    // Every Vision request is a VNRequest.
    unsafe { Retained::cast_unchecked::<VNRequest>(r.clone()) }
}

/// A Vision rectangle (origin bottom left) as x, y, w, h from the top left.
fn flip(r: objc2_core_foundation::CGRect) -> [f32; 4] {
    [r.origin.x as f32, (1.0 - r.origin.y - r.size.height) as f32, r.size.width as f32, r.size.height as f32]
}

pub struct VisionDetector;

pub fn detector() -> Result<Box<dyn Detector>> {
    Ok(Box::new(VisionDetector))
}

impl Detector for VisionDetector {
    fn detect(&mut self, frame: &VideoFrame) -> Result<Vec<Detection>> {
        let pb = small_buffer(frame, 960)?;
        autoreleasepool(|_| {
            let h = handler(pb.raw());
            let faces = unsafe { VNDetectFaceRectanglesRequest::new() };
            let bodies = unsafe { VNDetectHumanRectanglesRequest::new() };
            unsafe { bodies.setUpperBodyOnly(true) };
            let requests = NSArray::from_retained_slice(&[as_request(&faces), as_request(&bodies)]);
            h.performRequests_error(&requests).map_err(|e| anyhow!("Vision: {}", e.localizedDescription()))?;
            let mut out = Vec::new();
            if let Some(found) = unsafe { faces.results() } {
                for i in 0..found.count() {
                    let o = found.objectAtIndex(i);
                    out.push(Detection { rect: flip(unsafe { o.boundingBox() }), face: true, confidence: unsafe { o.confidence() } });
                }
            }
            if let Some(found) = unsafe { bodies.results() } {
                for i in 0..found.count() {
                    let o = found.objectAtIndex(i);
                    out.push(Detection { rect: flip(unsafe { o.boundingBox() }), face: false, confidence: unsafe { o.confidence() } });
                }
            }
            Ok(out)
        })
    }
}

pub struct VisionSegmenter {
    request: Retained<VNGeneratePersonSegmentationRequest>,
}

// Used from one thread at a time.
unsafe impl Send for VisionSegmenter {}

pub fn segmenter(quality: u8) -> Result<Box<dyn Segmenter>> {
    let request = unsafe { VNGeneratePersonSegmentationRequest::new() };
    let level = match quality {
        0 => VNGeneratePersonSegmentationRequestQualityLevel::Fast,
        2 => VNGeneratePersonSegmentationRequestQualityLevel::Accurate,
        _ => VNGeneratePersonSegmentationRequestQualityLevel::Balanced,
    };
    unsafe {
        request.setQualityLevel(level);
        // kCVPixelFormatType_OneComponent8
        request.setOutputPixelFormat(u32::from_be_bytes(*b"L008"));
    }
    Ok(Box::new(VisionSegmenter { request }))
}

impl Segmenter for VisionSegmenter {
    fn mask(&mut self, frame: &VideoFrame) -> Result<Mask> {
        let pb = small_buffer(frame, 1280)?;
        autoreleasepool(|_| {
            let h = handler(pb.raw());
            let requests = NSArray::from_retained_slice(&[as_request(&self.request)]);
            h.performRequests_error(&requests).map_err(|e| anyhow!("Vision: {}", e.localizedDescription()))?;
            let obs = unsafe { self.request.results() }.and_then(|r| r.firstObject()).ok_or_else(|| anyhow!("no mask"))?;
            let m = unsafe { obs.pixelBuffer() };
            unsafe {
                CVPixelBufferLockBaseAddress(&m, CVPixelBufferLockFlags::ReadOnly);
                let (w, hh, stride) = (CVPixelBufferGetWidth(&m), CVPixelBufferGetHeight(&m), CVPixelBufferGetBytesPerRow(&m));
                let base = CVPixelBufferGetBaseAddress(&m) as *const u8;
                let mut data = vec![0u8; w * hh];
                if !base.is_null() {
                    for y in 0..hh {
                        std::ptr::copy_nonoverlapping(base.add(y * stride), data.as_mut_ptr().add(y * w), w);
                    }
                }
                CVPixelBufferUnlockBaseAddress(&m, CVPixelBufferLockFlags::ReadOnly);
                Ok(Mask { width: w as u32, height: hh as u32, data })
            }
        })
    }
}
