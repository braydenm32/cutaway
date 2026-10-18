//! Sound from one app (Syrinscape, Spotify, a browser, a game…), captured
//! with ScreenCaptureKit (macOS 13+): no virtual audio cable needed. Uses
//! the Screen Recording permission.

use super::host_pts;
use super::screen::{error_text, shareable_content};
use crate::engine::sources::AudioStarted;
use crate::media::{clock, AudioBlock, NS_PER_SEC};
use crate::platform::AudioSink;
use anyhow::{anyhow, bail, Result};
use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList};
use objc2_core_foundation::CFRetained;
use objc2_core_media::{CMBlockBuffer, CMSampleBuffer, CMTime, CMTimeFlags};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol};
use objc2_screen_capture_kit::{SCContentFilter, SCRunningApplication, SCStream, SCStreamConfiguration, SCStreamOutput, SCStreamOutputType};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Duration;

/// Apps that can be listened to, by name.
pub fn list_apps() -> Vec<String> {
    let Ok(content) = shareable_content() else { return Vec::new() };
    let me = std::process::id() as i32;
    let apps = unsafe { content.applications() };
    let mut names: Vec<String> = apps
        .iter()
        .filter(|a| unsafe { a.processID() } != me)
        .map(|a| unsafe { a.applicationName() }.to_string())
        .filter(|n| !n.trim().is_empty())
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

struct Shared {
    sink: AudioSink,
    last_at: AtomicI64,
}

struct OutIvars {
    shared: Arc<Shared>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and this class
    // doesn't implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "CutawayAppAudioOutput"]
    #[ivars = OutIvars]
    struct AppAudioOutput;

    unsafe impl NSObjectProtocol for AppAudioOutput {}

    unsafe impl SCStreamOutput for AppAudioOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn did_output(&self, _stream: &SCStream, sample: &CMSampleBuffer, kind: SCStreamOutputType) {
            if kind.0 != SCStreamOutputType::Audio.0 {
                return; // the (tiny) pictures it has to make are ignored
            }
            if let Some((samples, channels)) = samples_of(sample) {
                let s = &self.ivars().shared;
                let pts = host_pts(unsafe { sample.presentation_time_stamp() });
                s.last_at.store(clock::now(), Ordering::Relaxed);
                (s.sink)(AudioBlock { pts, rate: 48_000, channels, samples });
            }
        }
    }
);

impl AppAudioOutput {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutIvars { shared });
        unsafe { msg_send![super(this), init] }
    }
}

/// The (float) samples in an audio sample buffer, interleaved.
fn samples_of(sample: &CMSampleBuffer) -> Option<(Vec<f32>, u16)> {
    let mut needed = 0usize;
    // SAFETY: first call only asks how big the buffer list must be.
    unsafe { sample.audio_buffer_list_with_retained_block_buffer(&mut needed, std::ptr::null_mut(), 0, None, None, 0, std::ptr::null_mut()) };
    if needed == 0 {
        return None;
    }
    let mut storage = vec![0u64; needed.div_ceil(8)];
    let list = storage.as_mut_ptr() as *mut AudioBufferList;
    let mut block: *mut CMBlockBuffer = std::ptr::null_mut();
    // SAFETY: the list has the size CoreMedia asked for; the block buffer
    // that owns the samples is released below, after they're copied.
    let status = unsafe { sample.audio_buffer_list_with_retained_block_buffer(std::ptr::null_mut(), list, needed, None, None, 1, &mut block) };
    let _owner = NonNull::new(block).map(|b| unsafe { CFRetained::<CMBlockBuffer>::from_raw(b) });
    if status != 0 {
        return None;
    }
    unsafe {
        let n = (*list).mNumberBuffers as usize;
        let buffers: &[AudioBuffer] = std::slice::from_raw_parts((*list).mBuffers.as_ptr(), n);
        let data = |b: &AudioBuffer| -> &[f32] {
            if b.mData.is_null() {
                &[]
            } else {
                std::slice::from_raw_parts(b.mData as *const f32, b.mDataByteSize as usize / 4)
            }
        };
        match n {
            0 => None,
            1 => Some((data(&buffers[0]).to_vec(), buffers[0].mNumberChannels.max(1) as u16)),
            _ => {
                // One buffer per channel: interleave.
                let chans: Vec<&[f32]> = buffers.iter().map(data).collect();
                let frames = chans.iter().map(|c| c.len()).min().unwrap_or(0);
                let mut out = Vec::with_capacity(frames * n);
                for f in 0..frames {
                    for c in &chans {
                        out.push(c[f]);
                    }
                }
                Some((out, n as u16))
            }
        }
    }
}

struct AppRunning {
    stream: Retained<SCStream>,
    _output: Retained<AppAudioOutput>,
    _queue: DispatchRetained<DispatchQueue>,
    stop: Arc<AtomicBool>,
    keepalive: Option<JoinHandle<()>>,
}

unsafe impl Send for AppRunning {}

impl Drop for AppRunning {
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

fn find_app(name: &str) -> Result<(Retained<SCRunningApplication>, Retained<objc2_screen_capture_kit::SCDisplay>)> {
    let content = shareable_content()?;
    let want = name.trim().to_lowercase();
    let apps = unsafe { content.applications() };
    let app = apps
        .iter()
        .find(|a| unsafe { a.applicationName() }.to_string().to_lowercase() == want || unsafe { a.bundleIdentifier() }.to_string().to_lowercase() == want)
        .or_else(|| apps.iter().find(|a| unsafe { a.applicationName() }.to_string().to_lowercase().contains(&want)))
        .ok_or_else(|| anyhow!("{name} isn't running"))?;
    let display = unsafe { content.displays() }.firstObject().ok_or_else(|| anyhow!("no display found"))?;
    Ok((app, display))
}

pub fn open_app_audio(name: &str, sink: AudioSink) -> Result<AudioStarted> {
    let (app, display) = find_app(name)?;
    let pid = unsafe { app.processID() };
    let app_name = unsafe { app.applicationName() }.to_string();
    let filter = unsafe {
        SCContentFilter::initWithDisplay_includingApplications_exceptingWindows(SCContentFilter::alloc(), &display, &NSArray::from_retained_slice(&[app]), &NSArray::new())
    };
    let config = unsafe { SCStreamConfiguration::new() };
    unsafe {
        config.setCapturesAudio(true);
        config.setSampleRate(48_000);
        config.setChannelCount(2);
        config.setExcludesCurrentProcessAudio(true);
        // Pictures can't be turned off; make them tiny and rare.
        config.setWidth(2);
        config.setHeight(2);
        config.setMinimumFrameInterval(CMTime { value: 1, timescale: 1, flags: CMTimeFlags::Valid, epoch: 0 });
        config.setQueueDepth(3);
    }
    let stream = unsafe { SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, None) };
    let shared = Arc::new(Shared { sink, last_at: AtomicI64::new(clock::now()) });
    let output = AppAudioOutput::new(shared.clone());
    let queue = DispatchQueue::new("cutaway.appaudio", None);
    for kind in [SCStreamOutputType::Audio, SCStreamOutputType::Screen] {
        unsafe { stream.addStreamOutput_type_sampleHandlerQueue_error(ProtocolObject::from_ref(&*output), kind, Some(&queue)) }
            .map_err(|e| anyhow!("app sound: {}", e.localizedDescription()))?;
    }
    let (tx, rx) = mpsc::channel();
    let block = RcBlock::new(move |err: *mut NSError| {
        let _ = tx.send(error_text(err));
    });
    unsafe { stream.startCaptureWithCompletionHandler(Some(&block)) };
    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(None) => {}
        Ok(Some(e)) => bail!("listening to {app_name} didn't start: {e}"),
        Err(_) => bail!("listening to {app_name} didn't start"),
    }

    // An app that's quiet sends nothing: fill with silence while it runs,
    // so the input doesn't look disconnected. When it quits, stop (the
    // input reconnects when it's back).
    let stop = Arc::new(AtomicBool::new(false));
    let keepalive = {
        let (stop, shared) = (stop.clone(), shared.clone());
        std::thread::Builder::new().name("app sound keepalive".into()).spawn(move || {
            let block = 10_000_000i64;
            let mut checked = std::time::Instant::now();
            let mut running = true;
            let mut silence_at: Option<i64> = None;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                if checked.elapsed() > Duration::from_secs(2) {
                    checked = std::time::Instant::now();
                    // SAFETY: signal 0 only checks the process exists.
                    running = unsafe { libc::kill(pid, 0) } == 0;
                }
                let now = clock::now();
                let last = shared.last_at.load(Ordering::Relaxed);
                if !running || now - last < NS_PER_SEC / 5 {
                    silence_at = None;
                    continue;
                }
                let mut t = silence_at.unwrap_or(now - block);
                while t + block <= now {
                    (shared.sink)(AudioBlock { pts: t, rate: 48_000, channels: 2, samples: vec![0.0; 960] });
                    t += block;
                }
                silence_at = Some(t);
            }
        })?
    };
    Ok(AudioStarted {
        running: Box::new(AppRunning { stream, _output: output, _queue: queue, stop, keepalive: Some(keepalive) }),
        description: format!("sound from {app_name}"),
        rate: 48_000,
        channels: 2,
    })
}
