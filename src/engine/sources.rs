//! Video and audio inputs: test generators here, real devices through the
//! platform layer. Every source hands its frames to a [`SourceSlot`].

use crate::util::LockExt;
use crate::config::{AudioConfig, SourceConfig};
use crate::devices::DeviceInfo;
use crate::media::testsrc::{TestPattern, TestTone, PATTERNS};
use crate::media::{clock, AudioBlock, Ns, VideoFrame, NS_PER_SEC};
use crate::platform;
use anyhow::{anyhow, Result};
use crossbeam_channel::Sender;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

/// Keeps a source running; dropping it stops the source.
pub trait Running: Send {}
impl<T: Send> Running for T {}

/// Camera buffers held at most per undelayed source (the capture pool is
/// small; holding more makes the camera drop frames).
const MAX_HELD: usize = 5;

struct SlotInner {
    /// Recent frames, oldest first.
    frames: VecDeque<VideoFrame>,
    /// Raw frames for the ISO recorder while recording.
    iso_tap: Option<Sender<VideoFrame>>,
    dropped: u64,
    /// Others that want every frame (lip-sync measurement, auto-framing);
    /// frames are skipped for them when they fall behind.
    watchers: Vec<(u64, Sender<VideoFrame>)>,
}

/// Recent frames of one video source, plus the recording tap.
pub struct SourceSlot {
    pub index: usize,
    /// Hold the picture back by this much (lines it up with slower sources).
    pub delay: Ns,
    /// How much history to keep.
    keep: Ns,
    inner: Mutex<SlotInner>,
    serial: AtomicU64,
    last_arrival: Mutex<Option<Instant>>,
}

impl SourceSlot {
    /// `lookback`: how far behind real time the compositor renders.
    pub fn new(index: usize, delay_ms: u32, lookback: Ns) -> Arc<Self> {
        let delay = delay_ms as i64 * 1_000_000;
        Arc::new(SourceSlot {
            index,
            delay,
            keep: delay + lookback + NS_PER_SEC / 10,
            inner: Mutex::new(SlotInner { frames: VecDeque::new(), iso_tap: None, dropped: 0, watchers: Vec::new() }),
            serial: AtomicU64::new(0),
            last_arrival: Mutex::new(None),
        })
    }

    /// Called by the source for every captured frame.
    pub fn push(&self, mut frame: VideoFrame) {
        frame.serial = self.serial.fetch_add(1, Ordering::Relaxed) + 1;
        *self.last_arrival.lock_ok() = Some(Instant::now());
        // A delay means many frames held (here and in the recorder, which
        // waits for their time to come); holding that many camera buffers
        // would starve the capture pool, so they're copied.
        let frame = if self.delay > 0 { frame.detached() } else { frame };
        let newest = frame.pts;
        let mut g = self.inner.lock_ok();
        if let Some(tap) = g.iso_tap.as_ref() {
            if tap.try_send(frame.clone()).is_err() {
                g.dropped += 1;
            }
        }
        for (_, w) in &g.watchers {
            let _ = w.try_send(frame.clone());
        }
        g.frames.push_back(frame);
        if self.delay == 0 {
            while g.frames.len() > MAX_HELD {
                g.frames.pop_front();
            }
        }
        while g.frames.len() > 1 && g.frames[1].pts + self.keep < newest {
            g.frames.pop_front();
        }
    }

    /// The frame to show at host time `t`: the newest one captured (plus
    /// delay) at or before `t`.
    pub fn pick(&self, t: Ns) -> Option<VideoFrame> {
        let g = self.inner.lock_ok();
        g.frames.iter().rev().find(|f| f.pts + self.delay <= t).or(g.frames.front()).cloned()
    }

    /// Start sending frames to the recorder, beginning with the frame that
    /// is on screen at `from` (so delayed sources start on time too).
    pub fn start_iso_tap(&self, tx: Sender<VideoFrame>, from: Ns) {
        let mut g = self.inner.lock_ok();
        let start = g.frames.iter().rposition(|f| f.pts + self.delay <= from).unwrap_or(0);
        for f in g.frames.iter().skip(start) {
            let _ = tx.try_send(f.clone());
        }
        g.iso_tap = Some(tx);
        g.dropped = 0;
    }

    /// Stop the recorder tap; returns how many frames were dropped because
    /// the encoder couldn't keep up.
    pub fn stop_iso_tap(&self) -> u64 {
        let mut g = self.inner.lock_ok();
        g.iso_tap = None;
        std::mem::take(&mut g.dropped)
    }

    /// Also send every new frame to `tx` (until removed).
    pub fn add_watcher(&self, id: u64, tx: Sender<VideoFrame>) {
        self.inner.lock_ok().watchers.push((id, tx));
    }

    pub fn remove_watcher(&self, id: u64) {
        self.inner.lock_ok().watchers.retain(|w| w.0 != id);
    }

    /// Frames delivered so far.
    pub fn frames(&self) -> u64 {
        self.serial.load(Ordering::Relaxed)
    }

    /// No frames for a while.
    pub fn stale(&self, secs: f32) -> bool {
        self.last_arrival.lock_ok().map(|t| t.elapsed().as_secs_f32() > secs).unwrap_or(true)
    }
}

/// Generated test picture at the configured size and rate.
struct TestVideo {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for TestVideo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn start_test_video(pattern: &str, cfg: &SourceConfig, slot: Arc<SourceSlot>) -> Result<Box<dyn Running>> {
    let stop = Arc::new(AtomicBool::new(false));
    let (w, h, fps) = (cfg.width, cfg.height, cfg.fps.max(1));
    // "clap+120": a camera 120 ms slow (its pictures show the past).
    let (pattern, lag) = match pattern.split_once('+') {
        Some((p, ms)) => (p, ms.trim().parse::<i64>().unwrap_or(0) * 1_000_000),
        None => (pattern, 0),
    };
    let mut tp = TestPattern::new(pattern, w, h);
    let s2 = stop.clone();
    let thread = std::thread::Builder::new().name(format!("test video {}", slot.index)).spawn(move || {
        use crate::engine::{grid_index, grid_time};
        let mut k = grid_index(clock::now(), fps);
        while !s2.load(Ordering::Relaxed) {
            let t = grid_time(k, fps);
            clock::sleep_until(t);
            let img = tp.render(t - lag, fps);
            slot.push(VideoFrame::new_nv12(w, h, t, img.y, img.uv, 0));
            k += 1;
            let now = clock::now();
            if now > grid_time(k + 5, fps) {
                k = grid_index(now, fps); // fell behind: skip ahead
            }
        }
    })?;
    Ok(Box::new(TestVideo { stop, thread: Some(thread) }))
}

/// What a running video source offers besides pictures.
#[derive(Default)]
pub struct Extras {
    pub player: Option<Arc<super::player::PlayerControl>>,
}

/// What a source may need from the rest of the engine.
#[derive(Default)]
pub struct Links {
    /// Every source's frames, by name (an auto-framed source is cut from
    /// another).
    pub peers: Arc<Vec<(String, Arc<SourceSlot>)>>,
    /// Live framing settings, for auto-framed sources.
    pub framing: Option<Arc<super::framing::FramingShared>>,
    /// PTZ controls: a virtual camera's state, or the moves an NDI
    /// receiver passes on.
    pub ptz: Mutex<Option<Arc<super::ptz::PtzCamera>>>,
}

/// Start a video source. Returns the running source and a description.
pub fn start_video(
    index: usize,
    cfg: &SourceConfig,
    devices: &[DeviceInfo],
    slot: Arc<SourceSlot>,
    hub: &Arc<super::feed::AudioHub>,
    net: &super::netsrc::NetStatus,
    links: &Links,
    extras: &mut Extras,
) -> Result<(Box<dyn Running>, String)> {
    let spec = cfg.device.trim();
    let lower = spec.to_lowercase();
    if let Some(from) = cfg.framed_from() {
        let parent = links
            .peers
            .iter()
            .find(|(n, _)| n.trim().eq_ignore_ascii_case(from))
            .map(|p| p.1.clone())
            .ok_or_else(|| anyhow::anyhow!("there's no source called \"{from}\" to cut it from"))?;
        let shared = links.framing.clone().unwrap_or_else(|| super::framing::FramingShared::new(&cfg.framing));
        return super::framing::start(cfg, from, parent, slot, shared);
    }
    if let Some(from) = cfg.virtual_ptz_of() {
        let parent = links
            .peers
            .iter()
            .find(|(n, _)| n.trim().eq_ignore_ascii_case(from))
            .map(|p| p.1.clone())
            .ok_or_else(|| anyhow::anyhow!("there's no source called \"{from}\" to cut it from"))?;
        let virt = links.ptz.lock_ok().as_ref().and_then(|p| p.virt.clone()).unwrap_or_else(|| super::ptz::VirtualPtz::new(&cfg.ptz));
        return super::ptz::start_virtual(cfg, from, parent, slot, virt);
    }
    if lower == "test" || lower.starts_with("test:") {
        let pattern = lower.strip_prefix("test:").map(str::to_string).unwrap_or_else(|| PATTERNS[index % PATTERNS.len()].to_string());
        let r = start_test_video(&pattern, cfg, slot)?;
        return Ok((r, format!("test pattern ({pattern})")));
    }
    if lower == "media" {
        let (r, control, desc) = super::player::start(&cfg.name, &cfg.media, (cfg.width, cfg.height), slot, hub.clone())?;
        extras.player = Some(control);
        return Ok((r, desc));
    }
    if super::netsrc::is_network(spec) {
        return super::netsrc::start(cfg, slot, hub.clone(), net.clone());
    }
    if let Some(name) = spec.strip_prefix("ndi:").or_else(|| spec.strip_prefix("NDI:")) {
        let ptz_rx = links.ptz.lock_ok().as_ref().and_then(|p| p.ndi_rx.clone());
        return super::ndi::start(name.trim(), cfg, slot, hub.clone(), net.clone(), ptz_rx);
    }
    if let Some((_, phone)) = cfg.link_id() {
        let r = super::guests::start_source(cfg, slot, hub.clone())?;
        return Ok((r, if phone { "phone camera" } else { "remote guest" }.into()));
    }
    if cfg.guest_screen_id().is_some() {
        let r = super::guests::start_source(cfg, slot, hub.clone())?;
        return Ok((r, "guest's screen".into()));
    }
    let sink: platform::FrameSink = Arc::new(move |f| slot.push(f));
    if lower == "screen" {
        let r = platform::open_screen(cfg.width, cfg.height, cfg.fps, sink)?;
        return Ok((r, "screen".into()));
    }
    let dev = crate::devices::find(spec, devices)?;
    let r = platform::open_camera(dev, cfg.width, cfg.height, cfg.fps, sink)?;
    Ok((r, dev.name.clone()))
}

/// Generated test tone (a sine with a beep on every second).
struct TestAudio {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for TestAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub struct AudioStarted {
    pub running: Box<dyn Running>,
    pub description: String,
    pub rate: u32,
    pub channels: u16,
}

pub fn start_audio(index: usize, cfg: &AudioConfig, devices: &[DeviceInfo], tx: Sender<AudioBlock>, hub: &Arc<super::feed::AudioHub>) -> Result<AudioStarted> {
    let lower = cfg.device.trim().to_lowercase();
    if let Some(source) = cfg.device.trim().strip_prefix("source:") {
        // Sound from a media player or network source.
        let source = source.trim().to_string();
        hub.register(&source, tx);
        return Ok(AudioStarted {
            running: Box::new(super::feed::HubInput { hub: hub.clone(), name: source.clone() }),
            description: format!("sound from {source}"),
            rate: 48_000,
            channels: 2,
        });
    }
    if let Some(app) = cfg.device.trim().strip_prefix("app:") {
        let sink: platform::AudioSink = Arc::new(move |b| {
            let _ = tx.send(b);
        });
        return platform::app_audio::open(app.trim(), sink);
    }
    if lower == "pads" {
        let bank = hub.pads.lock_ok().clone().ok_or_else(|| anyhow!("the sound pads aren't running"))?;
        return Ok(AudioStarted { running: super::pads::start_strip(bank, tx)?, description: "sound pads".into(), rate: 48_000, channels: 2 });
    }
    if lower == "test" || lower.starts_with("test:") {
        let channels = cfg.channels.clamp(1, 64) as u16;
        let tone = TestTone::new(48_000, channels, 220.0 * (index as f64 + 1.0));
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let thread = std::thread::Builder::new().name(format!("test audio {index}")).spawn(move || {
            let block = 10_000_000i64;
            let mut t = (clock::now() / block + 1) * block;
            while !s2.load(Ordering::Relaxed) {
                clock::sleep_until(t + block); // a block is "captured" once it's complete
                let samples = tone.render(t, 480);
                let _ = tx.send(AudioBlock { pts: t, rate: 48_000, channels, samples });
                t += block;
            }
        })?;
        return Ok(AudioStarted {
            running: Box::new(TestAudio { stop, thread: Some(thread) }),
            description: "test tone".into(),
            rate: 48_000,
            channels,
        });
    }
    let dev = if lower == "default" || lower.is_empty() { None } else { Some(crate::devices::find(&cfg.device, devices)?) };
    let sink: platform::AudioSink = Arc::new(move |b| {
        let _ = tx.send(b);
    });
    platform::open_audio_input(dev, cfg.channels, sink)
}
