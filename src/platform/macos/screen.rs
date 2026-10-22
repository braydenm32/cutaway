//! Screen capture through ScreenCaptureKit (macOS 12.3+): a display, one
//! window, or an app's windows; and the list of them, with pictures
//! (macOS 14+), for Settings.

use crate::util::LockExt;
use super::frame_from_sample;
use super::pixel_buffer::NV12_VIDEO_RANGE;
use crate::engine::sources::Running;
use crate::media::{clock, VideoFrame};
use crate::platform::FrameSink;
use anyhow::{anyhow, bail, Result};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, sel, AllocAnyThread, DefinedClass, Message};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use crate::platform::capture::{self, CaptureTarget, ScreenTarget, TargetKind, Thumb};
use objc2_core_foundation::{CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo};
use objc2_screen_capture_kit::{SCContentFilter, SCDisplay, SCRunningApplication, SCShareableContent, SCScreenshotManager, SCStream, SCStreamConfiguration, SCStreamDelegate, SCStreamOutput, SCStreamOutputType, SCWindow};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Carries an Objective-C object out of a completion handler.
pub(super) struct SendBox<T>(pub T);
unsafe impl<T> Send for SendBox<T> {}

struct Shared {
    sink: FrameSink,
    last: Mutex<Option<VideoFrame>>,
    last_at: AtomicI64,
    /// The capture ended by itself (the window closed, the app quit).
    stopped: AtomicBool,
}

struct ScreenIvars {
    shared: Arc<Shared>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class
    // doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "CutawayScreenOutput"]
    #[ivars = ScreenIvars]
    struct ScreenOutput;

    unsafe impl NSObjectProtocol for ScreenOutput {}

    unsafe impl SCStreamOutput for ScreenOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind.0 != SCStreamOutputType::Screen.0 {
                return;
            }
            // Frames without a picture just mean "nothing changed".
            if let Some(frame) = frame_from_sample(sample) {
                let s = &self.ivars().shared;
                s.last_at.store(clock::now(), Ordering::Relaxed);
                *s.last.lock_ok() = Some(frame.clone());
                (s.sink)(frame);
            }
        }
    }
);

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class
    // doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "CutawayScreenDelegate"]
    #[ivars = ScreenIvars]
    struct ScreenDelegate;

    unsafe impl NSObjectProtocol for ScreenDelegate {}

    unsafe impl SCStreamDelegate for ScreenDelegate {
        #[unsafe(method(stream:didStopWithError:))]
        fn did_stop(&self, _stream: &SCStream, _error: &NSError) {
            self.ivars().shared.stopped.store(true, Ordering::Relaxed);
        }
    }
);

impl ScreenDelegate {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ScreenIvars { shared });
        unsafe { msg_send![super(this), init] }
    }
}

impl ScreenOutput {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ScreenIvars { shared });
        unsafe { msg_send![super(this), init] }
    }
}

struct ScreenRunning {
    stream: Retained<SCStream>,
    _output: Retained<ScreenOutput>,
    _delegate: Retained<ScreenDelegate>,
    _queue: DispatchRetained<DispatchQueue>,
    stop: Arc<AtomicBool>,
    keepalive: Option<JoinHandle<()>>,
}

unsafe impl Send for ScreenRunning {}

impl Drop for ScreenRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |_err: *mut NSError| {
            let _ = tx.send(());
        });
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&block)) };
        let _ = rx.recv_timeout(Duration::from_secs(2));
        if let Some(t) = self.keepalive.take() {
            let _ = t.join();
        }
    }
}

pub(super) fn error_text(err: *mut NSError) -> Option<String> {
    unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string())
}

/// What can be captured: displays, windows and apps. The first call is
/// also what asks for Screen Recording permission.
pub(super) fn shareable_content() -> Result<Retained<SCShareableContent>> {
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let r = match unsafe { content.as_ref() } {
            Some(c) => Ok(SendBox(c.retain())),
            None => Err(error_text(err).unwrap_or_else(|| "no screens".into())),
        };
        let _ = tx.send(r);
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    Ok(rx
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| anyhow!("screen capture didn't answer"))?
        .map_err(|e| anyhow!("screen capture isn't allowed ({e}). Turn on Cutaway in System Settings → Privacy & Security → Screen Recording, then restart it."))?
        .0)
}

/// Windows worth offering: on screen, in the normal layer, with a title
/// and a size, and not Cutaway's own.
fn listable_windows(content: &SCShareableContent) -> Vec<(Retained<SCWindow>, Retained<SCRunningApplication>)> {
    let me = std::process::id() as i32;
    unsafe { content.windows() }
        .iter()
        .filter_map(|w| unsafe {
            let app = w.owningApplication()?;
            let f = w.frame();
            let title = w.title().map(|t| t.to_string()).unwrap_or_default();
            (w.isOnScreen() && w.windowLayer() == 0 && !title.trim().is_empty() && f.size.width >= 60.0 && f.size.height >= 40.0 && app.processID() != me).then(|| (w.retain(), app))
        })
        .collect()
}

fn window_title(w: &SCWindow) -> String {
    unsafe { w.title() }.map(|t| t.to_string()).unwrap_or_default()
}

fn app_id(app: &SCRunningApplication) -> String {
    let id = unsafe { app.bundleIdentifier() }.to_string();
    if id.is_empty() {
        unsafe { app.applicationName() }.to_string()
    } else {
        id
    }
}

/// The display a window is mostly on.
fn display_of(content: &SCShareableContent, frame: CGRect) -> Option<Retained<SCDisplay>> {
    let c = (frame.origin.x + frame.size.width / 2.0, frame.origin.y + frame.size.height / 2.0);
    let displays = unsafe { content.displays() };
    let inside = displays.iter().find(|d| {
        let f = unsafe { d.frame() };
        c.0 >= f.origin.x && c.0 < f.origin.x + f.size.width && c.1 >= f.origin.y && c.1 < f.origin.y + f.size.height
    });
    inside.or_else(|| displays.firstObject())
}

/// A display by the name `displays` gives it (the main one if it's gone).
fn find_display(content: &SCShareableContent, name: Option<&str>) -> Result<Retained<SCDisplay>> {
    let displays = unsafe { content.displays() };
    let list = crate::platform::displays::list();
    let wanted = name.and_then(|n| list.iter().find(|d| d.name == n)).or_else(|| list.iter().find(|d| d.main)).map(|d| d.id as u32);
    wanted
        .and_then(|id| displays.iter().find(|d| unsafe { d.displayID() } == id))
        .or_else(|| displays.firstObject())
        .ok_or_else(|| anyhow!("no display found"))
}

/// What to capture, and how big its picture is (in points).
fn filter_for(content: &SCShareableContent, target: &ScreenTarget) -> Result<(Retained<SCContentFilter>, CGSize, bool)> {
    let none: Retained<NSArray<SCWindow>> = NSArray::new();
    match target {
        ScreenTarget::Main | ScreenTarget::Display(_) => {
            let name = if let ScreenTarget::Display(n) = target { Some(n.as_str()) } else { None };
            let display = find_display(content, name)?;
            let size = unsafe { display.frame() }.size;
            Ok((unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &none) }, size, false))
        }
        ScreenTarget::Window { app, title } => {
            let windows: Vec<_> = listable_windows(content).into_iter().filter(|(_, a)| app_id(a) == *app).collect();
            let titled: Vec<(usize, String)> = windows.iter().enumerate().map(|(i, (w, _))| (i, window_title(w))).collect();
            let i = capture::best_window(&titled, title).ok_or_else(|| anyhow!("{} has no window open", capture::short_app(app)))?;
            let w = &windows[i].0;
            let size = unsafe { w.frame() }.size;
            Ok((unsafe { SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), w) }, size, true))
        }
        ScreenTarget::App(app) => {
            let windows: Vec<_> = listable_windows(content).into_iter().filter(|(_, a)| app_id(a) == *app).collect();
            let (biggest, running) = windows
                .iter()
                .max_by(|a, b| {
                    let area = |w: &SCWindow| unsafe { w.frame().size.width * w.frame().size.height };
                    area(&a.0).total_cmp(&area(&b.0))
                })
                .ok_or_else(|| anyhow!("{} isn't open (or has no windows)", capture::short_app(app)))?;
            let display = display_of(content, unsafe { biggest.frame() }).ok_or_else(|| anyhow!("no display found"))?;
            let size = unsafe { display.frame() }.size;
            let apps = NSArray::from_retained_slice(&[running.clone()]);
            Ok((unsafe { SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(SCContentFilter::alloc(), &display, &apps, &none) }, size, false))
        }
    }
}

/// Settings for a capture `w`×`h`: a window or app fits inside it with its
/// shape kept (black around it).
fn stream_config(w: u32, h: u32, single_window: bool) -> Retained<SCStreamConfiguration> {
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setWidth(w as usize);
        config.setHeight(h as usize);
        config.setScalesToFit(true);
        // macOS 14+.
        if config.respondsToSelector(sel!(setPreservesAspectRatio:)) {
            config.setPreservesAspectRatio(true);
        }
        if single_window && config.respondsToSelector(sel!(setIgnoreShadowsSingleWindow:)) {
            config.setIgnoreShadowsSingleWindow(true);
        }
    }
    config
}

pub fn open_screen(target: &ScreenTarget, w: u32, h: u32, fps: u32, sink: FrameSink) -> Result<Box<dyn Running>> {
    let content = shareable_content()?;
    let (filter, _, single) = filter_for(&content, target)?;
    let config = stream_config(w, h, single);
    unsafe {
        config.setMinimumFrameInterval(CMTime { value: 1, timescale: fps.max(1) as i32, flags: CMTimeFlags::Valid, epoch: 0 });
        config.setPixelFormat(NV12_VIDEO_RANGE);
        config.setQueueDepth(8);
        config.setShowsCursor(true);
    }
    let shared = Arc::new(Shared { sink, last: Mutex::new(None), last_at: AtomicI64::new(0), stopped: AtomicBool::new(false) });
    let delegate = ScreenDelegate::new(shared.clone());
    let stream = unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, Some(ProtocolObject::from_ref(&*delegate))) };
    let output = ScreenOutput::new(shared.clone());
    let queue = DispatchQueue::new("cutaway.screen", None);
    unsafe { stream.addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), SCStreamOutputType::Screen, Some(&queue)) }
        .map_err(|e| anyhow!("screen capture: {}", e.localizedDescription()))?;

    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |err: *mut NSError| {
        let _ = tx.send(error_text(err));
    });
    unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(None) => {}
        Ok(Some(e)) => bail!("screen capture didn't start: {e}"),
        Err(_) => bail!("screen capture didn't start"),
    }

    // A still screen sends no new frames; repeat the last one so the
    // source doesn't look disconnected. Once the capture has ended (its
    // window closed), nothing is repeated: the source shows no signal and
    // is opened again, finding the window if it's back.
    let stop = Arc::new(AtomicBool::new(false));
    let keepalive = {
        let (stop, shared) = (stop.clone(), shared.clone());
        let frame_ns = 1_000_000_000 / fps.max(1) as i64;
        std::thread::Builder::new().name("screen keepalive".into()).spawn(move || {
            while !stop.load(Ordering::Relaxed) && !shared.stopped.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                let now = clock::now();
                if now - shared.last_at.load(Ordering::Relaxed) > 4 * frame_ns {
                    let last = shared.last.lock_ok().clone();
                    if let Some(mut f) = last {
                        f.pts = now;
                        shared.last_at.store(now, Ordering::Relaxed);
                        (shared.sink)(f);
                    }
                }
            }
        })?
    };
    Ok(Box::new(ScreenRunning { stream, _output: output, _delegate: delegate, _queue: queue, stop, keepalive: Some(keepalive) }))
}

/// Thumbnails' longest side, in pixels.
const THUMB: f64 = 320.0;

/// A small picture through a filter (macOS 14+; None before).
fn snapshot(filter: &SCContentFilter, size: CGSize, single_window: bool) -> Option<Thumb> {
    objc2::runtime::AnyClass::get(c"SCScreenshotManager")?;
    if size.width < 1.0 || size.height < 1.0 {
        return None;
    }
    let k = THUMB / size.width.max(size.height);
    let (tw, th) = (((size.width * k) as u32).max(2), ((size.height * k) as u32).max(2));
    let config = stream_config(tw, th, single_window);
    unsafe { config.setShowsCursor(false) };
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |img: *mut CGImage, _err: *mut NSError| {
        let img = NonNull::new(img).map(|p| SendBox(unsafe { CFRetained::retain(p) }));
        let _ = tx.send(img);
    });
    unsafe { SCScreenshotManager::captureImageWithFilter_configuration_completionHandler(filter, &config, Some(&block)) };
    let img = rx.recv_timeout(Duration::from_secs(3)).ok()??.0;
    image_rgba(&img)
}

/// A CGImage's pixels, as RGBA.
fn image_rgba(img: &CGImage) -> Option<Thumb> {
    let (w, h) = (CGImage::width(Some(img)), CGImage::height(Some(img)));
    if w == 0 || h == 0 {
        return None;
    }
    let mut rgba = vec![0u8; w * h * 4];
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: the buffer is w*h*4 bytes and outlives the context.
    let ctx = unsafe { CGBitmapContextCreate(rgba.as_mut_ptr().cast(), w, h, 8, w * 4, Some(&space), CGImageAlphaInfo::PremultipliedLast.0) }?;
    CGContext::draw_image(Some(&ctx), CGRect { origin: CGPoint { x: 0.0, y: 0.0 }, size: CGSize { width: w as f64, height: h as f64 } }, Some(img));
    drop(ctx);
    for p in rgba.chunks_exact_mut(4) {
        p[3] = 255;
    }
    Some(Thumb { size: [w, h], rgba })
}

/// Every display, window and app that can be captured, with pictures.
/// Slow (a second or two): call it off the UI thread.
pub fn capture_targets() -> Result<Vec<CaptureTarget>> {
    let content = shareable_content()?;
    let none: Retained<NSArray<SCWindow>> = NSArray::new();
    let names = crate::platform::displays::list();
    let mut out = Vec::new();
    for d in unsafe { content.displays() }.iter() {
        let id = unsafe { d.displayID() };
        let Some(info) = names.iter().find(|n| n.id == id as u64) else { continue };
        let filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &d, &none) };
        out.push(CaptureTarget {
            kind: TargetKind::Display,
            device: if info.main { "screen".into() } else { ScreenTarget::Display(info.name.clone()).device() },
            name: info.name.clone(),
            detail: format!("{}×{}", info.pixels.0, info.pixels.1),
            thumb: snapshot(&filter, unsafe { d.frame() }.size, false),
        });
    }
    let windows = listable_windows(&content);
    let mut apps: Vec<(String, String, Option<Thumb>)> = Vec::new();
    for (w, app) in windows.iter().take(40) {
        let (id, app_name, title) = (app_id(app), unsafe { app.applicationName() }.to_string(), window_title(w));
        let filter = unsafe { SCContentFilter::initWithDesktopIndependentWindow(SCContentFilter::alloc(), w) };
        let thumb = snapshot(&filter, unsafe { w.frame() }.size, true);
        if !apps.iter().any(|a| a.0 == id) {
            apps.push((id.clone(), app_name.clone(), thumb.clone()));
        }
        out.push(CaptureTarget { kind: TargetKind::Window, device: ScreenTarget::Window { app: id, title: title.clone() }.device(), name: title, detail: app_name, thumb });
    }
    apps.sort_by_key(|a| a.1.to_lowercase());
    for (id, name, thumb) in apps {
        out.push(CaptureTarget { kind: TargetKind::App, device: ScreenTarget::App(id).device(), name, detail: "every window of the app".into(), thumb });
    }
    Ok(out)
}
