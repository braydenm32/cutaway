//! macOS: AVFoundation cameras and capture cards, ScreenCaptureKit,
//! CoreAudio inputs, VideoToolbox and AudioToolbox encoders. Everything is
//! part of the operating system, so nothing needs installing.

pub mod aac;
mod appaudio;
pub mod aacdec;
mod camera;
pub mod pixel_buffer;
mod screen;
pub mod speech;
pub mod vcam;
pub mod vision;
pub mod vt;
pub mod vtdec;

pub use camera::{check_microphone_access, list_video_devices, open_camera};
pub use appaudio::{list_apps, open_app_audio};
pub use screen::open_screen;

use crate::media::{clock, Ns, Pixels, VideoFrame, NS_PER_SEC};
use objc2_core_foundation::CFString;
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_foundation::NSString;
use pixel_buffer::PixelBuffer;
use std::sync::Arc;

/// Ask for camera and microphone access if macOS hasn't asked yet (only
/// the first time). Doesn't wait for the answer.
pub fn request_permissions() {
    camera::request_access();
}

/// True once after the user granted access; devices need reopening then.
pub fn permissions_changed() -> bool {
    camera::permissions_changed()
}

pub(crate) fn cmtime_ns(t: CMTime) -> Option<Ns> {
    if !t.flags.contains(CMTimeFlags::Valid) || t.timescale <= 0 {
        return None;
    }
    Some((t.value as i128 * NS_PER_SEC as i128 / t.timescale as i128) as Ns)
}

pub(crate) fn ns_cmtime(ns: Ns) -> CMTime {
    CMTime { value: ns, timescale: 1_000_000_000, flags: CMTimeFlags::Valid, epoch: 0 }
}

/// A capture timestamp on the host clock. AVFoundation, ScreenCaptureKit
/// and CoreAudio all stamp media with host time already; anything that
/// isn't (or is wildly off) gets the arrival time instead.
pub(crate) fn host_pts(t: CMTime) -> Ns {
    let now = clock::now();
    match cmtime_ns(t) {
        Some(v) if (v - now).abs() < 10 * NS_PER_SEC => v,
        _ => now,
    }
}

/// CFString and NSString are the same object ("toll-free bridged").
pub(crate) fn ns_str(s: &CFString) -> &NSString {
    unsafe { &*(s as *const CFString as *const NSString) }
}

/// The video frame in a capture sample buffer (NV12 only).
pub(crate) fn frame_from_sample(sample: &CMSampleBuffer) -> Option<VideoFrame> {
    let image = unsafe { sample.image_buffer() }?;
    let pb = PixelBuffer::new(image);
    if !pb.is_nv12() {
        return None;
    }
    let pts = host_pts(unsafe { sample.presentation_time_stamp() });
    Some(VideoFrame { width: pb.width(), height: pb.height(), pts, pixels: Arc::new(Pixels::PixelBuffer(pb)), serial: 0 })
}
