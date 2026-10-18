//! Screen capture through ScreenCaptureKit (macOS 12.3+).

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
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass, Message};
use objc2_core_media::{CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Carries an Objective-C object out of a completion handler.
struct SendBox<T>(T);
unsafe impl<T> Send for SendBox<T> {}

struct Shared {
    sink: FrameSink,
    last: Mutex<Option<VideoFrame>>,
    last_at: AtomicI64,
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
                *s.last.lock().unwrap() = Some(frame.clone());
                (s.sink)(frame);
            }
        }
    }
);

impl ScreenOutput {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ScreenIvars { shared });
        unsafe { msg_send![super(this), init] }
    }
}

struct ScreenRunning {
    stream: Retained<SCStream>,
    _output: Retained<ScreenOutput>,
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

fn error_text(err: *mut NSError) -> Option<String> {
    unsafe { err.as_ref() }.map(|e| e.localizedDescription().to_string())
}

pub fn open_screen(w: u32, h: u32, fps: u32, sink: FrameSink) -> Result<Box<dyn Running>> {
    // Which displays exist (this is also what asks for permission).
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |content: *mut SCShareableContent, err: *mut NSError| {
        let r = match unsafe { content.as_ref() } {
            Some(c) => Ok(SendBox(c.retain())),
            None => Err(error_text(err).unwrap_or_else(|| "no screens".into())),
        };
        let _ = tx.send(r);
    });
    unsafe { SCShareableContent::getShareableContentWithCompletionHandler(&block) };
    let content = rx
        .recv_timeout(Duration::from_secs(15))
        .map_err(|_| anyhow!("screen capture didn't answer"))?
        .map_err(|e| anyhow!("screen capture isn't allowed ({e}). Turn on Cutaway in System Settings → Privacy & Security → Screen Recording, then restart it."))?
        .0;
    let display = unsafe { content.displays() }.firstObject().ok_or_else(|| anyhow!("no display found"))?;

    let filter = unsafe { SCContentFilter::initWithDisplay_excludingWindows(SCContentFilter::alloc(), &display, &NSArray::new()) };
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setWidth(w as usize);
        config.setHeight(h as usize);
        config.setMinimumFrameInterval(CMTime { value: 1, timescale: fps.max(1) as i32, flags: CMTimeFlags::Valid, epoch: 0 });
        config.setPixelFormat(NV12_VIDEO_RANGE);
        config.setQueueDepth(8);
        config.setShowsCursor(true);
    }
    let stream = unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None) };
    let shared = Arc::new(Shared { sink, last: Mutex::new(None), last_at: AtomicI64::new(0) });
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
    // source doesn't look disconnected.
    let stop = Arc::new(AtomicBool::new(false));
    let keepalive = {
        let (stop, shared) = (stop.clone(), shared.clone());
        let frame_ns = 1_000_000_000 / fps.max(1) as i64;
        std::thread::Builder::new().name("screen keepalive".into()).spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
                let now = clock::now();
                if now - shared.last_at.load(Ordering::Relaxed) > 4 * frame_ns {
                    let last = shared.last.lock().unwrap().clone();
                    if let Some(mut f) = last {
                        f.pts = now;
                        shared.last_at.store(now, Ordering::Relaxed);
                        (shared.sink)(f);
                    }
                }
            }
        })?
    };
    Ok(Box::new(ScreenRunning { stream, _output: output, _queue: queue, stop, keepalive: Some(keepalive) }))
}
