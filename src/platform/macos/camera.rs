//! Cameras and capture cards through AVFoundation.

use super::pixel_buffer::NV12_VIDEO_RANGE;
use super::{frame_from_sample, ns_str};
use crate::devices::{summarize, DeviceInfo, Mode};
use crate::engine::sources::Running;
use crate::platform::FrameSink;
use anyhow::{anyhow, bail, Result};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceFormat, AVCaptureDeviceInput, AVCaptureOutput,
    AVCaptureSession, AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaType, AVMediaTypeAudio,
    AVMediaTypeVideo,
};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags, CMVideoFormatDescriptionGetDimensions};
use objc2_core_video::{kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey};
use objc2_foundation::{NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSString};
use std::sync::atomic::{AtomicBool, Ordering};

fn media(kind: Option<&'static AVMediaType>) -> Option<&'static AVMediaType> {
    kind
}

fn video_type() -> Result<&'static AVMediaType> {
    media(unsafe { AVMediaTypeVideo }).ok_or_else(|| anyhow!("AVFoundation isn't available"))
}

/// Set when the user grants access after the engine started (devices only
/// deliver real pictures and sound once access is granted).
static PERMISSION_GRANTED: AtomicBool = AtomicBool::new(false);

/// Ask for camera and microphone access if macOS hasn't asked yet. Doesn't
/// wait: the answer arrives later (see [`permissions_changed`]).
pub fn request_access() {
    for kind in [unsafe { AVMediaTypeVideo }, unsafe { AVMediaTypeAudio }].into_iter().flatten() {
        let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(kind) };
        if status == AVAuthorizationStatus::NotDetermined {
            let block = RcBlock::new(move |granted: Bool| {
                if granted.as_bool() {
                    PERMISSION_GRANTED.store(true, Ordering::Relaxed);
                }
            });
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(kind, &block) };
        }
    }
}

/// True once after the user granted access (the engine should restart).
pub fn permissions_changed() -> bool {
    PERMISSION_GRANTED.swap(false, Ordering::Relaxed)
}

fn denied(kind: Option<&'static AVMediaType>) -> bool {
    let Some(kind) = kind else { return false };
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(kind) };
    status == AVAuthorizationStatus::Denied || status == AVAuthorizationStatus::Restricted
}

fn check_access() -> Result<()> {
    if denied(unsafe { AVMediaTypeVideo }) {
        bail!("Cutaway isn't allowed to use cameras. Turn it on in System Settings → Privacy & Security → Camera, then restart Cutaway.");
    }
    Ok(())
}

pub fn check_microphone_access() -> Result<()> {
    if denied(unsafe { AVMediaTypeAudio }) {
        bail!("Cutaway isn't allowed to record audio. Turn it on in System Settings → Privacy & Security → Microphone, then restart Cutaway.");
    }
    Ok(())
}

#[allow(deprecated)] // the discovery session needs a fixed list of device types; this finds them all
fn all_devices() -> Vec<Retained<AVCaptureDevice>> {
    let Ok(kind) = video_type() else { return Vec::new() };
    unsafe { AVCaptureDevice::devicesWithMediaType(kind) }.to_vec()
}

fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

struct FormatInfo {
    format: Retained<AVCaptureDeviceFormat>,
    width: u32,
    height: u32,
    /// (min, max) frame rate per supported range.
    ranges: Vec<RateRange>,
    mjpeg: bool,
    nv12: bool,
}

/// One supported frame-rate range of a device format, with the exact frame
/// durations the device reports for its ends.
#[derive(Clone, Copy)]
struct RateRange {
    lo: f64,
    hi: f64,
    /// Frame duration at `hi` (the fastest).
    fastest: CMTime,
    /// Frame duration at `lo`.
    slowest: CMTime,
}

impl RateRange {
    fn contains(&self, fps: f64) -> bool {
        fps >= self.lo - 0.01 && fps <= self.hi + 0.01
    }
}

/// A frame duration the device will accept for `fps`. AVFoundation throws
/// an exception for anything outside the reported range, so the range's own
/// end values are used at the edges (e.g. 30 fps on a camera that reports
/// 30.00003).
fn frame_duration(ranges: &[RateRange], fps: u32) -> Option<CMTime> {
    let want = fps.max(1) as f64;
    if let Some(r) = ranges.iter().find(|r| r.contains(want)) {
        return Some(if want >= r.hi - 0.01 {
            r.fastest
        } else if want <= r.lo + 0.01 {
            r.slowest
        } else {
            CMTime { value: 1, timescale: fps as i32, flags: CMTimeFlags::Valid, epoch: 0 }
        });
    }
    // e.g. 29.97 when 30 was asked: the nearest the device has.
    ranges.iter().min_by(|a, b| (a.hi - want).abs().total_cmp(&(b.hi - want).abs())).map(|r| r.fastest)
}

fn formats(dev: &AVCaptureDevice) -> Vec<FormatInfo> {
    let mut out = Vec::new();
    for f in unsafe { dev.formats() }.to_vec() {
        let fd = unsafe { f.formatDescription() };
        let dims = unsafe { CMVideoFormatDescriptionGetDimensions(&fd) };
        let sub = unsafe { fd.media_sub_type() };
        let ranges = unsafe { f.videoSupportedFrameRateRanges() }
            .to_vec()
            .into_iter()
            .map(|r| unsafe {
                RateRange { lo: r.minFrameRate(), hi: r.maxFrameRate(), fastest: r.minFrameDuration(), slowest: r.maxFrameDuration() }
            })
            .collect();
        out.push(FormatInfo {
            format: f,
            width: dims.width.max(0) as u32,
            height: dims.height.max(0) as u32,
            ranges,
            mjpeg: sub == fourcc(b"jpeg") || sub == fourcc(b"dmb1"),
            nv12: sub == fourcc(b"420v") || sub == fourcc(b"420f"),
        });
    }
    out
}

const COMMON_RATES: [u32; 11] = [15, 24, 25, 30, 48, 50, 60, 90, 100, 120, 240];

fn supports(f: &FormatInfo, fps: u32) -> bool {
    f.ranges.iter().any(|r| fps as f64 >= r.lo - 0.1 && fps as f64 <= r.hi + 0.1)
}

fn modes(list: &[FormatInfo]) -> Vec<Mode> {
    let mut out: Vec<Mode> = Vec::new();
    for f in list {
        let mut rates: Vec<u32> = COMMON_RATES.iter().copied().filter(|&r| supports(f, r)).collect();
        if rates.is_empty() {
            rates.extend(f.ranges.iter().map(|r| r.hi.round() as u32));
        }
        match out.iter_mut().find(|m| m.width == f.width && m.height == f.height) {
            Some(m) => {
                m.fps.extend(rates);
                m.mjpeg_only &= f.mjpeg;
            }
            None => out.push(Mode { width: f.width, height: f.height, fps: rates, mjpeg_only: f.mjpeg }),
        }
    }
    for m in &mut out {
        m.fps.sort_unstable();
        m.fps.dedup();
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.width as u64 * m.height as u64));
    out
}

pub fn list_video_devices() -> Vec<DeviceInfo> {
    all_devices()
        .iter()
        .map(|d| {
            let m = modes(&formats(d));
            DeviceInfo {
                name: unsafe { d.localizedName() }.to_string(),
                id: unsafe { d.uniqueID() }.to_string(),
                summary: summarize(&m),
                max_channels: 0,
                rate: 0,
                modes: m,
            }
        })
        .collect()
}

/// The device format closest to what was asked for: the exact size at the
/// frame rate if possible (uncompressed preferred), otherwise the smallest
/// larger size, otherwise the largest. AVFoundation scales the rest.
fn pick_format(list: &[FormatInfo], w: u32, h: u32, fps: u32) -> Option<&FormatInfo> {
    let score = |f: &FormatInfo| -> (bool, u8, u8, i64) {
        let area = f.width as i64 * f.height as i64;
        let size = if f.width == w && f.height == h {
            2
        } else if f.width >= w && f.height >= h {
            1
        } else {
            0
        };
        let quality = if f.nv12 { 2 } else if f.mjpeg { 0 } else { 1 };
        // Among larger formats the smallest wins; among smaller, the largest.
        (supports(f, fps), size, quality, if size >= 1 { -area } else { area })
    };
    list.iter().max_by_key(|f| score(f))
}

struct CamIvars {
    sink: FrameSink,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class
    // doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "CutawayCameraDelegate"]
    #[ivars = CamIvars]
    struct CameraDelegate;

    unsafe impl NSObjectProtocol for CameraDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for CameraDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output(&self, _output: &AVCaptureOutput, sample: &CMSampleBuffer, _connection: &AVCaptureConnection) {
            if let Some(frame) = frame_from_sample(sample) {
                (self.ivars().sink)(frame);
            }
        }
    }
);

impl CameraDelegate {
    fn new(sink: FrameSink) -> Retained<Self> {
        let this = Self::alloc().set_ivars(CamIvars { sink });
        unsafe { msg_send![super(this), init] }
    }
}

struct CameraRunning {
    session: Retained<AVCaptureSession>,
    output: Retained<AVCaptureVideoDataOutput>,
    _delegate: Retained<CameraDelegate>,
    _queue: DispatchRetained<DispatchQueue>,
}

// The capture session is safe to start and stop from any thread.
unsafe impl Send for CameraRunning {}

impl Drop for CameraRunning {
    fn drop(&mut self) {
        unsafe {
            self.session.stopRunning();
            self.output.setSampleBufferDelegate_queue(None, None);
        }
    }
}

pub fn open_camera(info: &DeviceInfo, w: u32, h: u32, fps: u32, sink: FrameSink) -> Result<Box<dyn Running>> {
    check_access()?;
    let id = NSString::from_str(&info.id);
    let device = unsafe { AVCaptureDevice::deviceWithUniqueID(&id) }.ok_or_else(|| anyhow!("{} is no longer connected", info.name))?;
    let session = unsafe { AVCaptureSession::new() };
    unsafe { session.beginConfiguration() };
    let input = unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }
        .map_err(|e| anyhow!("can't open {}: {}", info.name, e.localizedDescription()))?;
    if !unsafe { session.canAddInput(&input) } {
        bail!("{} is in use by another app or can't be opened", info.name);
    }
    unsafe { session.addInput(&input) };

    // Device mode: the right size and rate if the device has it. If the
    // device refuses, it keeps its current mode and frames are scaled.
    let list = formats(&device);
    if let Some(f) = pick_format(&list, w, h, fps) {
        if unsafe { device.lockForConfiguration() }.is_ok() {
            let set = crate::util::guard("Setting the capture mode", || {
                unsafe { device.setActiveFormat(&f.format) };
                if let Some(d) = frame_duration(&f.ranges, fps) {
                    unsafe {
                        device.setActiveVideoMinFrameDuration(d);
                        device.setActiveVideoMaxFrameDuration(d);
                    }
                }
                Ok(())
            });
            unsafe { device.unlockForConfiguration() };
            if let Err(e) = set {
                eprintln!("warning: {}: {e:#}; using the device's current mode", info.name);
            }
        }
    }

    // Frames: NV12 at exactly the configured size (AVFoundation scales).
    let output = unsafe { AVCaptureVideoDataOutput::new() };
    let format = NSNumber::new_u32(NV12_VIDEO_RANGE);
    let width = NSNumber::new_u32(w);
    let height = NSNumber::new_u32(h);
    let (k1, k2, k3) = unsafe { (ns_str(kCVPixelBufferPixelFormatTypeKey), ns_str(kCVPixelBufferWidthKey), ns_str(kCVPixelBufferHeightKey)) };
    let (v1, v2, v3): (&AnyObject, &AnyObject, &AnyObject) = (&format, &width, &height);
    let sized = NSDictionary::from_slices(&[k1, k2, k3], &[v1, v2, v3]);
    let plain = NSDictionary::from_slices(&[k1], &[v1]);
    if let Err(e) = crate::util::guard("Choosing the frame size", || {
        unsafe { output.setVideoSettings(Some(&sized)) };
        Ok(())
    }) {
        // Frames then arrive at the device's own size; the compositor adapts.
        eprintln!("warning: {}: {e:#}; using the device's frame size", info.name);
        crate::util::guard("Choosing the pixel format", || {
            unsafe { output.setVideoSettings(Some(&plain)) };
            Ok(())
        })?;
    }
    unsafe { output.setAlwaysDiscardsLateVideoFrames(true) };
    let label: String = info.name.chars().filter(|c| *c != '\0').collect();
    let queue = DispatchQueue::new(&format!("cutaway.camera.{label}"), None);
    let delegate = CameraDelegate::new(sink);
    unsafe { output.setSampleBufferDelegate_queue(Some(ProtocolObject::from_ref(&*delegate)), Some(&queue)) };
    if !unsafe { session.canAddOutput(&output) } {
        bail!("{}: can't deliver video in the needed format", info.name);
    }
    unsafe {
        session.addOutput(&output);
        session.commitConfiguration();
        session.startRunning();
    }
    if !unsafe { session.isRunning() } {
        bail!("{} didn't start (in use by another app?)", info.name);
    }
    Ok(Box::new(CameraRunning { session, output, _delegate: delegate, _queue: queue }))
}
