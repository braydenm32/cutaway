//! Sends the program to the Cutaway Virtual Camera (a CoreMediaIO camera
//! extension, see macos/virtualcam), through the camera's sink stream.

use super::pixel_buffer::{PixelBuffer, NV12_VIDEO_RANGE};
use crate::engine::sources::Running;
use crate::media::VideoFrame;
use anyhow::{anyhow, bail, Result};
use crossbeam_channel::Receiver;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_media::CMTime;
use objc2_core_video::{kCVPixelBufferHeightKey, kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey, CVPixelBufferPool};
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
const DEVICE_UID: &str = "4E7A6C0B-2B0E-4C55-9C0A-5A1A1D1C0A11";

#[repr(C)]
struct Addr {
    selector: u32,
    scope: u32,
    element: u32,
}

#[repr(C)]
struct SampleTiming {
    duration: CMTime,
    pts: CMTime,
    dts: CMTime,
}

const fn fourcc(s: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*s)
}

#[link(name = "CoreMediaIO", kind = "framework")]
extern "C-unwind" {
    fn CMIOObjectGetPropertyDataSize(obj: u32, addr: *const Addr, qsize: u32, qdata: *const c_void, size: *mut u32) -> i32;
    fn CMIOObjectGetPropertyData(obj: u32, addr: *const Addr, qsize: u32, qdata: *const c_void, size: u32, used: *mut u32, data: *mut c_void) -> i32;
    fn CMIOStreamCopyBufferQueue(stream: u32, altered: Option<unsafe extern "C-unwind" fn(u32, *mut c_void, *mut c_void)>, refcon: *mut c_void, queue: *mut *mut c_void) -> i32;
    fn CMIODeviceStartStream(device: u32, stream: u32) -> i32;
    fn CMIODeviceStopStream(device: u32, stream: u32) -> i32;
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C-unwind" {
    fn CMSimpleQueueEnqueue(q: *mut c_void, element: *const c_void) -> i32;
    fn CMSimpleQueueGetCount(q: *mut c_void) -> i32;
    fn CMSimpleQueueGetCapacity(q: *mut c_void) -> i32;
    fn CMVideoFormatDescriptionCreateForImageBuffer(alloc: *const c_void, pb: *const c_void, out: *mut *const c_void) -> i32;
    fn CMSampleBufferCreateForImageBuffer(
        alloc: *const c_void,
        pb: *const c_void,
        ready: bool,
        callback: *const c_void,
        refcon: *const c_void,
        format: *const c_void,
        timing: *const SampleTiming,
        out: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C-unwind" {
    fn CVPixelBufferPoolCreate(alloc: *const c_void, pool_attrs: *const c_void, pb_attrs: *const c_void, out: *mut *mut CVPixelBufferPool) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(p: *const c_void);
    fn CFStringGetCString(s: *const c_void, buf: *mut i8, size: isize, encoding: u32) -> bool;
}

const SYSTEM_OBJECT: u32 = 1;
const GLOBAL: u32 = fourcc(b"glob");

fn get<T: Copy + Default>(obj: u32, selector: u32) -> Vec<T> {
    let addr = Addr { selector, scope: GLOBAL, element: 0 };
    let mut size = 0u32;
    if unsafe { CMIOObjectGetPropertyDataSize(obj, &addr, 0, std::ptr::null(), &mut size) } != 0 || size == 0 {
        return Vec::new();
    }
    let n = size as usize / std::mem::size_of::<T>();
    let mut v = vec![T::default(); n];
    let mut used = 0u32;
    if unsafe { CMIOObjectGetPropertyData(obj, &addr, 0, std::ptr::null(), size, &mut used, v.as_mut_ptr().cast()) } != 0 {
        return Vec::new();
    }
    v.truncate(used as usize / std::mem::size_of::<T>());
    v
}

fn uid(device: u32) -> Option<String> {
    let addr = Addr { selector: fourcc(b"uid "), scope: GLOBAL, element: 0 };
    let mut s: *const c_void = std::ptr::null();
    let mut used = 0u32;
    let st = unsafe { CMIOObjectGetPropertyData(device, &addr, 0, std::ptr::null(), std::mem::size_of::<*const c_void>() as u32, &mut used, (&mut s as *mut *const c_void).cast()) };
    if st != 0 || s.is_null() {
        return None;
    }
    let mut buf = [0i8; 256];
    let ok = unsafe { CFStringGetCString(s, buf.as_mut_ptr(), buf.len() as isize, 0x0800_0100) };
    unsafe { CFRelease(s) };
    ok.then(|| unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy().to_string())
}

/// The Cutaway camera's device and sink stream, if it's installed.
fn find() -> Option<(u32, u32)> {
    let devices: Vec<u32> = get(SYSTEM_OBJECT, fourcc(b"dev#"));
    let device = devices.into_iter().find(|d| uid(*d).is_some_and(|u| u.eq_ignore_ascii_case(DEVICE_UID)))?;
    let streams: Vec<u32> = get(device, fourcc(b"stm#"));
    // The sink is the stream that takes input (direction 1); else the second.
    let sink = streams.iter().copied().find(|s| get::<u32>(*s, fourcc(b"sdir")).first() == Some(&1)).or_else(|| streams.get(1).copied())?;
    Some((device, sink))
}

pub fn installed() -> bool {
    find().is_some()
}

/// The helper that asks macOS to install the camera (inside the app).
fn helper() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let h = exe.parent()?.join("cutaway-camera");
    h.exists().then_some(h)
}

/// Install (or remove) the camera extension. Returns what macOS said.
pub fn install(remove: bool) -> Result<String> {
    let Some(h) = helper() else {
        bail!("this copy of Cutaway was built without the virtual camera (see \"Virtual camera\" in the README: it needs an Apple Developer signature)");
    };
    let out = std::process::Command::new(h).arg(if remove { "uninstall" } else { "install" }).output()?;
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if let Some(e) = text.lines().find_map(|l| l.strip_prefix("error: ")) {
        bail!("{e}");
    }
    Ok(text.lines().filter_map(|l| l.strip_prefix("approval: ")).next().map(String::from).unwrap_or_else(|| {
        if remove { "The virtual camera was removed.".into() } else { "The virtual camera is installed. Choose \"Cutaway\" as the camera in your video app.".into() }
    }))
}

struct VcamRunning {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for VcamRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Sink {
    device: u32,
    stream: u32,
    queue: *mut c_void,
    pool: CFRetained<CVPixelBufferPool>,
    format: *const c_void,
}

impl Drop for Sink {
    fn drop(&mut self) {
        unsafe {
            CMIODeviceStopStream(self.device, self.stream);
            if !self.format.is_null() {
                CFRelease(self.format);
            }
        }
    }
}

unsafe extern "C-unwind" fn altered(_stream: u32, _token: *mut c_void, _refcon: *mut c_void) {}

fn open_sink() -> Result<Sink> {
    let (device, stream) = find().ok_or_else(|| anyhow!("the Cutaway camera isn't installed (Settings → Screens & webcam → Install)"))?;
    let mut queue: *mut c_void = std::ptr::null_mut();
    let st = unsafe { CMIOStreamCopyBufferQueue(stream, Some(altered), std::ptr::null_mut(), &mut queue) };
    if st != 0 || queue.is_null() {
        bail!("couldn't connect to the virtual camera (error {st})");
    }
    let st = unsafe { CMIODeviceStartStream(device, stream) };
    if st != 0 {
        bail!("the virtual camera didn't start (error {st})");
    }
    // IOSurface-backed buffers, so pictures can cross to the camera's process.
    let (fmt, w, h) = (CFNumber::new_i32(NV12_VIDEO_RANGE as i32), CFNumber::new_i32(WIDTH as i32), CFNumber::new_i32(HEIGHT as i32));
    let surface = CFDictionary::<CFString, CFType>::from_slices(&[], &[]);
    let (fmt_r, w_r, h_r, s_r): (&CFType, &CFType, &CFType, &CFType) = (fmt.as_ref(), w.as_ref(), h.as_ref(), surface.as_ref());
    let attrs = CFDictionary::<CFString, CFType>::from_slices(
        &[unsafe { kCVPixelBufferPixelFormatTypeKey }, unsafe { kCVPixelBufferWidthKey }, unsafe { kCVPixelBufferHeightKey }, unsafe { kCVPixelBufferIOSurfacePropertiesKey }],
        &[fmt_r, w_r, h_r, s_r],
    );
    let mut pool: *mut CVPixelBufferPool = std::ptr::null_mut();
    let attrs_ptr: *const CFDictionary<CFString, CFType> = &*attrs;
    let st = unsafe { CVPixelBufferPoolCreate(std::ptr::null(), std::ptr::null(), attrs_ptr.cast(), &mut pool) };
    let pool = NonNull::new(pool).ok_or_else(|| anyhow!("couldn't make picture buffers (error {st})"))?;
    Ok(Sink { device, stream, queue, pool: unsafe { CFRetained::from_raw(pool) }, format: std::ptr::null() })
}

impl Sink {
    fn send(&mut self, frame: &VideoFrame) -> Result<()> {
        unsafe {
            if CMSimpleQueueGetCount(self.queue) >= CMSimpleQueueGetCapacity(self.queue) {
                return Ok(()); // the camera is behind: skip this picture
            }
        }
        let pb = PixelBuffer::copy_from(Some(&self.pool), frame)?;
        let raw: *const c_void = (pb.raw() as *const objc2_core_video::CVPixelBuffer).cast();
        if self.format.is_null() {
            let st = unsafe { CMVideoFormatDescriptionCreateForImageBuffer(std::ptr::null(), raw, &mut self.format) };
            if st != 0 {
                bail!("couldn't describe the picture (error {st})");
            }
        }
        let now = super::ns_cmtime(crate::media::clock::now());
        let invalid = CMTime { value: 0, timescale: 0, flags: objc2_core_media::CMTimeFlags(0), epoch: 0 };
        let timing = SampleTiming { duration: CMTime { value: 1, timescale: 30, flags: objc2_core_media::CMTimeFlags(1), epoch: 0 }, pts: now, dts: invalid };
        let mut sample: *mut c_void = std::ptr::null_mut();
        let st = unsafe { CMSampleBufferCreateForImageBuffer(std::ptr::null(), raw, true, std::ptr::null(), std::ptr::null(), self.format, &timing, &mut sample) };
        if st != 0 || sample.is_null() {
            bail!("couldn't wrap the picture (error {st})");
        }
        // The queue takes our reference; the camera releases it.
        let st = unsafe { CMSimpleQueueEnqueue(self.queue, sample) };
        if st != 0 {
            unsafe { CFRelease(sample) };
        }
        Ok(())
    }
}

/// Sends program frames from `rx` until dropped.
pub fn start(rx: Receiver<VideoFrame>) -> Result<Box<dyn Running>> {
    let mut sink = open_sink()?;
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let thread = std::thread::Builder::new().name("virtual camera".into()).spawn(move || {
        while !s2.load(Ordering::Relaxed) {
            let Ok(f) = rx.recv_timeout(Duration::from_millis(200)) else { continue };
            let f = if (f.width, f.height) != (WIDTH, HEIGHT) { f.scaled(WIDTH, HEIGHT) } else { f };
            if let Err(e) = sink.send(&f) {
                eprintln!("virtual camera: {e:#}");
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    })?;
    Ok(Box::new(VcamRunning { stop, thread: Some(thread) }))
}

// Raw CoreMedia objects are only touched on the sender thread.
unsafe impl Send for Sink {}
