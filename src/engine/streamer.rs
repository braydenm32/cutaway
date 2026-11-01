//! Live streaming: the program is encoded once and sent to any number of
//! destinations at the same time (simulcast).
//!
//! ```text
//! compositor ─► stream tap ─► H.264 (constant bitrate) ─┬─► destination thread ─► RTMP / RTMPS
//! mixer ──────► AAC ─────────────────────────────────────┼─► destination thread ─► RTMP / RTMPS
//!                                                        └─► destination thread ─► MPEG-TS ─► SRT
//! ```
//!
//! Destinations marked vertical get the vertical (9:16) version instead,
//! from its own encoder (same sound).
//!
//! Each destination has its own thread and queue. A slow connection only
//! drops its own packets (and picks up again at the next keyframe); a dead
//! one reconnects with growing waits. Neither ever holds up the encoder,
//! the other destinations, the program or a recording.

use crate::util::LockExt;
use super::encode::{self, Purpose, VideoSettings};
use super::recorder::{AudioCursor, Retimer};
use super::{audio::MixTaps, next_boundary, LogLevel, Status, StreamTap, VideoTaps, Waker};
use crate::config::{Service, StreamDestination, StreamingConfig};
use crate::media::ts::TsMuxer;
use crate::media::{flv, AudioConfig, Ns, VideoCodec, VideoConfig, VideoFrame, VideoPacket, AAC_FRAME, NS_PER_SEC};
use crate::net::{rtmp::RtmpPublisher, srt::SrtSender};
use anyhow::{bail, Result};
use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum DestState {
    Connecting,
    Live,
    /// Waiting before the next attempt.
    Retrying { at: Instant },
    /// Gave up (or auto-reconnect is off).
    Failed,
    /// Turned off in settings.
    Off,
}

#[derive(Debug, Clone)]
pub struct DestStatus {
    pub name: String,
    pub service: Service,
    pub state: DestState,
    pub kbps: f64,
    pub message: Option<String>,
    pub attempts: u32,
    /// The connection can't keep up and frames are being dropped.
    pub congested: bool,
}

#[derive(Debug, Clone, Default)]
pub struct StreamStatus {
    /// When Go Live was pressed (connecting starts).
    pub since: Option<Instant>,
    /// When the first destination was really live: what the LIVE clock
    /// counts from, on the desk, the phone and OSC alike (None while it's
    /// still connecting).
    pub on_air_since: Option<Instant>,
    pub stopping: bool,
    pub encoder: String,
    pub destinations: Vec<DestStatus>,
    /// Set when the whole stream had to stop.
    pub error: Option<String>,
    /// The video bitrate, while it's lowered for a struggling connection.
    pub lowered_kbps: Option<u32>,
}

impl StreamStatus {
    /// What the LIVE clock counts from: the first destination really on
    /// air (None while it's connecting, or not streaming).
    pub fn live_clock_from(&self) -> Option<Instant> {
        self.on_air_since
    }

    /// Seconds on air so far: the one LIVE time, for the desk and the
    /// controllers alike (0 while connecting).
    pub fn live_seconds(&self) -> f64 {
        self.live_clock_from().map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
    }
}

/// A destination's state as the desk, the phone and OSC show it: with the
/// try it's on while it waits to try again ("Retrying in 4 s (try 2)").
pub fn describe_dest(d: &DestStatus) -> String {
    let text = describe(&d.state);
    if d.attempts >= 1 && matches!(d.state, DestState::Retrying { .. }) {
        format!("{text} (try {})", d.attempts + 1)
    } else {
        text
    }
}

/// Readable name for a destination's state.
pub fn describe(state: &DestState) -> String {
    match state {
        DestState::Connecting => "Connecting…".into(),
        DestState::Live => "Live".into(),
        DestState::Retrying { at } => {
            // Rounded up, so it never says 0 s while it's still waiting.
            let left = at.saturating_duration_since(Instant::now());
            match left.as_millis().div_ceil(1000) {
                0 => "Retrying now…".into(),
                s => format!("Retrying in {s} s"),
            }
        }
        DestState::Failed => "Failed".into(),
        DestState::Off => "Off".into(),
    }
}

/// What the stream needs from the engine.
pub struct Context {
    pub video_taps: Arc<Mutex<VideoTaps>>,
    pub mix_taps: MixTaps,
    pub tap_id: u64,
    pub program_fps: u32,
    /// The vertical version's size, while it's on.
    pub vertical: Option<(u32, u32)>,
    pub status: Arc<Mutex<Status>>,
    pub waker: Waker,
}

#[derive(Clone)]
enum Pkt {
    Video(VideoPacket),
    Audio(Ns, Arc<Vec<u8>>),
}

impl Pkt {
    fn time(&self) -> Ns {
        match self {
            Pkt::Video(v) => v.dts,
            Pkt::Audio(t, _) => *t,
        }
    }
}

/// Flags shared between the encoders and one destination.
#[derive(Default)]
struct DestShared {
    connected: AtomicBool,
    /// Skip video until the next keyframe (just connected, or dropped
    /// packets because the connection is slow).
    need_key: AtomicBool,
    congested: AtomicBool,
}

/// Hands encoded packets to every connected destination.
struct Fanout {
    dests: Vec<(Sender<Pkt>, Arc<DestShared>)>,
}

impl Fanout {
    /// How full the fullest connected destination's queue is (0..1).
    fn backlog(&self) -> f32 {
        self.dests
            .iter()
            .filter(|(_, d)| d.connected.load(Ordering::Relaxed))
            .map(|(tx, _)| tx.len() as f32 / tx.capacity().unwrap_or(1).max(1) as f32)
            .fold(0.0, f32::max)
    }

    fn video(&self, p: &VideoPacket) {
        for (tx, d) in &self.dests {
            if !d.connected.load(Ordering::Relaxed) {
                continue;
            }
            if d.need_key.load(Ordering::Relaxed) && !p.keyframe {
                continue;
            }
            match tx.try_send(Pkt::Video(p.clone())) {
                Ok(()) => {
                    if p.keyframe {
                        d.need_key.store(false, Ordering::Relaxed);
                    }
                }
                Err(TrySendError::Full(_)) => {
                    d.congested.store(true, Ordering::Relaxed);
                    d.need_key.store(true, Ordering::Relaxed);
                }
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }

    fn audio(&self, t: Ns, data: Arc<Vec<u8>>) {
        for (tx, d) in &self.dests {
            if !d.connected.load(Ordering::Relaxed) {
                continue;
            }
            if let Err(TrySendError::Full(_)) = tx.try_send(Pkt::Audio(t, data.clone())) {
                d.congested.store(true, Ordering::Relaxed);
            }
        }
    }
}

/// Adaptive bitrate: steps the bitrate down by a quarter while a
/// destination's queue is backing up (at most every 2 s, never below a
/// quarter of the target), and back up a little at a time once every queue
/// has been clear for 15 s.
struct Adapt {
    target: u32,
    current: u32,
    last_change: Instant,
    calm_since: Instant,
    /// The encoder can't change bitrate.
    disabled: bool,
}

impl Adapt {
    fn new(target: u32, now: Instant) -> Adapt {
        Adapt { target, current: target, last_change: now, calm_since: now, disabled: false }
    }

    fn floor(&self) -> u32 {
        (self.target / 4).max(400).min(self.target)
    }

    /// A new bitrate, when it should change.
    fn step(&mut self, backlog: f32, now: Instant) -> Option<u32> {
        if self.disabled {
            return None;
        }
        if backlog > 0.3 {
            self.calm_since = now;
            if self.current > self.floor() && now.duration_since(self.last_change) >= Duration::from_secs(2) {
                self.current = (self.current * 3 / 4).max(self.floor());
                self.last_change = now;
                return Some(self.current);
            }
            return None;
        }
        if backlog > 0.05 {
            self.calm_since = now;
            return None;
        }
        if self.current < self.target && now.duration_since(self.calm_since) >= Duration::from_secs(15) && now.duration_since(self.last_change) >= Duration::from_secs(10) {
            self.current = (self.current + (self.target / 10).max(100)).min(self.target);
            self.last_change = now;
            return Some(self.current);
        }
        None
    }
}

pub struct StreamSession {
    pub encoder: String,
    ctx: Context,
    stop: Arc<AtomicBool>,
    video_job: Option<JoinHandle<()>>,
    tall_job: Option<JoinHandle<()>>,
    audio_job: Option<JoinHandle<()>>,
    dests: Vec<JoinHandle<()>>,
}

impl StreamSession {
    /// Start the encoders and connect every enabled destination.
    pub fn start(cfg: &StreamingConfig, ctx: Context) -> Result<StreamSession> {
        let enabled: Vec<&StreamDestination> = cfg.destinations.iter().filter(|d| d.enabled).collect();
        if enabled.is_empty() {
            bail!("no stream destinations are turned on (Record › Streaming)");
        }
        for d in &enabled {
            if let Some(p) = d.problem() {
                bail!("{}: {p}", d.name);
            }
            if d.vertical && ctx.vertical.is_none() {
                bail!("{} is set to get the vertical version, which is off (Record › Vertical version)", d.name);
            }
        }
        let backend = encode::resolve_backend(VideoCodec::H264, &cfg.encoder)?;
        let mut aac = encode::create_aac(super::audio::MIX_RATE, 2, cfg.audio_bitrate_kbps)?;
        let acfg = aac.config();
        let fps = cfg.fps.min(ctx.program_fps).max(1);
        let size = (cfg.width & !1, cfg.height & !1);
        let any_tall = enabled.iter().any(|d| d.vertical);
        let any_wide = enabled.iter().any(|d| !d.vertical);
        let encoder = format!("H.264 · {} · {} kbps{}", encode::describe_backend(backend), cfg.video_bitrate_kbps, if any_tall && any_wide { " (and vertical)" } else if any_tall { " · vertical" } else { "" });
        let stop = Arc::new(AtomicBool::new(false));
        // Destinations ask their encoder for a keyframe when they connect.
        let force_key = Arc::new(AtomicBool::new(true));
        let tall_key = Arc::new(AtomicBool::new(true));
        let s0 = next_boundary(ctx.program_fps);

        {
            let mut st = ctx.status.lock_ok();
            st.stream = StreamStatus {
                since: Some(Instant::now()),
                on_air_since: None,
                stopping: false,
                encoder: encoder.clone(),
                destinations: enabled
                    .iter()
                    .map(|d| DestStatus {
                        name: d.name.clone(),
                        service: d.service,
                        state: DestState::Connecting,
                        kbps: 0.0,
                        message: None,
                        attempts: 0,
                        congested: false,
                    })
                    .collect(),
                error: None,
                lowered_kbps: None,
            };
        }

        // Destinations: queue ~3 s of packets each.
        let queue = (fps as usize + 50) * 3;
        let mut fan = Vec::new();
        let mut tall_fan = Vec::new();
        let mut dests = Vec::new();
        for (i, d) in enabled.iter().enumerate() {
            let (tx, rx) = bounded(queue);
            let shared = Arc::new(DestShared::default());
            if d.vertical {
                tall_fan.push((tx, shared.clone()));
            } else {
                fan.push((tx, shared.clone()));
            }
            let dest = Destination {
                index: i,
                cfg: (*d).clone(),
                rx,
                shared,
                force_key: if d.vertical { tall_key.clone() } else { force_key.clone() },
                stop: stop.clone(),
                status: ctx.status.clone(),
                waker: ctx.waker.clone(),
                auto_reconnect: cfg.auto_reconnect,
                max_retries: cfg.max_retries,
                audio: acfg.clone(),
                fps,
                video_kbps: cfg.video_bitrate_kbps,
                audio_kbps: cfg.audio_bitrate_kbps,
            };
            dests.push(std::thread::Builder::new().name(format!("stream {}", d.name)).spawn(move || dest.run())?);
        }
        let fan = Arc::new(Fanout { dests: fan });
        let tall_fan = Arc::new(Fanout { dests: tall_fan });

        // Video: retimed onto the stream's frame rate, encoded once (and the
        // vertical version, once, if a destination gets it).
        let spawn_video = |name: &str, size: (u32, u32), fan: Arc<Fanout>, force_key: Arc<AtomicBool>| -> Result<(Sender<VideoFrame>, JoinHandle<()>)> {
            let (vtx, vrx) = bounded::<VideoFrame>((fps as usize / 2).max(4));
            let (status, waker) = (ctx.status.clone(), ctx.waker.clone());
            let mut settings = VideoSettings {
                codec: VideoCodec::H264,
                width: size.0,
                height: size.1,
                fps,
                bitrate_kbps: cfg.video_bitrate_kbps,
                purpose: Purpose::Stream,
                backend: backend.to_string(),
            };
            let adaptive = cfg.adaptive_bitrate;
            let job = std::thread::Builder::new().name(name.into()).spawn(move || {
                let mut retimer = Retimer::new(s0, fps, 0);
                let mut enc: Option<Box<dyn encode::VideoEncoder>> = None;
                let mut adapt = Adapt::new(settings.bitrate_kbps, Instant::now());
                let mut last_check = Instant::now();
                let result = (|| -> Result<()> {
                    for frame in vrx.iter() {
                        for (pts, f) in retimer.push(frame) {
                            if enc.is_none() {
                                settings.width = f.width;
                                settings.height = f.height;
                                let fan = fan.clone();
                                enc = Some(encode::create_video(&settings, Arc::new(move |p| fan.video(&p)))?);
                            }
                            enc.as_mut().unwrap().encode(&f, pts, force_key.swap(false, Ordering::Relaxed))?;
                        }
                        // Ease off while a connection can't keep up.
                        if adaptive && last_check.elapsed() >= Duration::from_millis(500) {
                            last_check = Instant::now();
                            if let (Some(kbps), Some(e)) = (adapt.step(fan.backlog(), Instant::now()), enc.as_mut()) {
                                if e.set_bitrate(kbps) {
                                    let mut st = status.lock_ok();
                                    st.stream.lowered_kbps = (kbps < adapt.target).then_some(kbps);
                                    st.push(
                                        LogLevel::Info,
                                        if kbps < adapt.target {
                                            format!("Stream: the connection is struggling, so the video bitrate is down to {kbps} kbps")
                                        } else {
                                            format!("Stream: the connection has caught up; the video bitrate is back to {kbps} kbps")
                                        },
                                    );
                                    drop(st);
                                    waker();
                                } else {
                                    adapt.disabled = true;
                                }
                            }
                        }
                    }
                    if let Some(e) = enc.as_mut() {
                        e.finish()?;
                    }
                    Ok(())
                })();
                if let Err(e) = result {
                    let mut st = status.lock_ok();
                    st.push(LogLevel::Error, format!("Stream encoder: {e:#}"));
                    st.stream.error = Some(format!("the stream encoder stopped: {e:#}"));
                    drop(st);
                    waker();
                    for _ in vrx.iter() {} // keep draining until the tap goes away
                }
            })?;
            Ok((vtx, job))
        };
        let mut video_job = None;
        if any_wide {
            let (vtx, job) = spawn_video("stream video", size, fan.clone(), force_key.clone())?;
            video_job = Some(job);
            ctx.video_taps.lock_ok().stream = Some(StreamTap { tx: vtx, from: s0, size, fps });
        }
        let mut tall_job = None;
        if let (true, Some(tall)) = (any_tall, ctx.vertical) {
            let (vtx, job) = spawn_video("stream vertical", tall, tall_fan.clone(), tall_key.clone())?;
            tall_job = Some(job);
            ctx.video_taps.lock_ok().tall_stream = Some(StreamTap { tx: vtx, from: s0, size: tall, fps });
        }

        // Audio: the program mix, AAC.
        let (atx, arx) = bounded(500);
        let audio_job = {
            let (fan, tall_fan) = (fan.clone(), tall_fan.clone());
            let status = ctx.status.clone();
            std::thread::Builder::new().name("stream audio".into()).spawn(move || {
                let rate = acfg.rate as i64;
                let mut cursor = AudioCursor::new(s0, acfg.rate, acfg.channels as usize);
                // FLV and MPEG-TS can't say "skip the encoder's start-up
                // delay" the way MP4 does, so each frame is stamped with the
                // time of the audio it actually plays.
                let priming = acfg.priming as i64;
                let mut n = 0i64;
                let mut send = |frames: Vec<Vec<u8>>| {
                    for f in frames {
                        let (t, data) = ((n * AAC_FRAME as i64 - priming) * NS_PER_SEC / rate, Arc::new(f));
                        fan.audio(t, data.clone());
                        tall_fan.audio(t, data);
                        n += 1;
                    }
                };
                for b in arx.iter() {
                    let samples = cursor.take(&b, i64::MAX);
                    if samples.is_empty() {
                        continue;
                    }
                    match aac.push(&samples) {
                        Ok(frames) => send(frames),
                        Err(e) => {
                            status.lock_ok().push(LogLevel::Error, format!("Stream audio encoder: {e:#}"));
                            for _ in arx.iter() {}
                            return;
                        }
                    }
                }
                if let Ok(frames) = aac.finish() {
                    send(frames);
                }
            })?
        };
        ctx.mix_taps.lock_ok().push((ctx.tap_id, super::audio::MixKind::Stream, atx));

        Ok(StreamSession { encoder, ctx, stop, video_job, tall_job, audio_job: Some(audio_job), dests })
    }

    /// End the stream: the encoders flush, every destination sends what's
    /// left and closes politely. Blocks for up to a few seconds.
    pub fn stop(mut self) {
        // Let the last frames through the compositor and mixer.
        std::thread::sleep(Duration::from_millis(150));
        {
            let mut taps = self.ctx.video_taps.lock_ok();
            taps.stream = None;
            taps.tall_stream = None;
        }
        let id = self.ctx.tap_id;
        self.ctx.mix_taps.lock_ok().retain(|(i, ..)| *i != id);
        for j in [self.video_job.take(), self.tall_job.take(), self.audio_job.take()].into_iter().flatten() {
            let _ = j.join();
        }
        // Encoders are gone, so destination queues end; anyone still
        // connecting or waiting to retry is told to give up.
        self.stop.store(true, Ordering::Relaxed);
        let deadline = Instant::now() + Duration::from_secs(5);
        for d in self.dests.drain(..) {
            while !d.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if d.is_finished() {
                let _ = d.join();
            } // else: stuck in a connect timeout; it ends on its own.
        }
    }
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// One destination

/// A connected output.
enum Output {
    Rtmp(Box<RtmpPublisher>),
    Srt(Box<SrtSender>, Option<TsMuxer>),
}

impl Output {
    fn connect(d: &StreamDestination) -> Result<Output> {
        let url = d.url();
        let timeout = Duration::from_secs(10);
        if d.service == Service::Srt {
            Ok(Output::Srt(Box::new(SrtSender::connect(&url, timeout)?), None))
        } else {
            Ok(Output::Rtmp(Box::new(RtmpPublisher::connect(&url, timeout)?)))
        }
    }

    /// Stream headers (sent before the first keyframe, and again if the
    /// encoder's configuration changes).
    fn configure(&mut self, v: &VideoConfig, a: &AudioConfig, fps: u32, video_kbps: u32, audio_kbps: u32) -> Result<()> {
        match self {
            Output::Rtmp(p) => {
                p.send_metadata(&flv::metadata(v, fps, video_kbps, a, audio_kbps))?;
                p.send_video(0, &flv::video_sequence_header(v))?;
                p.send_audio(0, &flv::audio_sequence_header(a))?;
            }
            Output::Srt(_, mux) => *mux = Some(TsMuxer::new(v.clone(), a.clone())),
        }
        Ok(())
    }

    fn video(&mut self, pts: Ns, dts: Ns, keyframe: bool, data: &[u8]) -> Result<()> {
        match self {
            Output::Rtmp(p) => {
                let cts = ((pts - dts) / 1_000_000) as i32;
                p.send_video((dts / 1_000_000) as u32, &flv::video_frame(data, keyframe, cts))
            }
            Output::Srt(s, Some(m)) => s.send(&m.video(pts, dts, keyframe, data)),
            Output::Srt(_, None) => Ok(()),
        }
    }

    fn audio(&mut self, t: Ns, data: &[u8]) -> Result<()> {
        match self {
            Output::Rtmp(p) => p.send_audio((t / 1_000_000) as u32, &flv::audio_frame(data)),
            Output::Srt(s, Some(m)) => s.send(&m.audio(t, data)),
            Output::Srt(_, None) => Ok(()),
        }
    }

    fn poll(&mut self) -> Result<()> {
        match self {
            Output::Rtmp(p) => p.poll(),
            Output::Srt(..) => Ok(()),
        }
    }

    fn bytes_sent(&self) -> u64 {
        match self {
            Output::Rtmp(p) => p.bytes_sent(),
            Output::Srt(s, _) => s.bytes_sent(),
        }
    }

    fn close(self) {
        match self {
            Output::Rtmp(p) => p.close(),
            Output::Srt(s, _) => s.close(),
        }
    }
}

/// Puts video and audio back into one time order (they come from separate
/// encoders with different delays).
#[derive(Default)]
struct Interleaver {
    video: VecDeque<Pkt>,
    audio: VecDeque<Pkt>,
    seen_video: Option<Ns>,
    seen_audio: Option<Ns>,
}

impl Interleaver {
    fn push(&mut self, p: Pkt) {
        match &p {
            Pkt::Video(_) => {
                self.seen_video = Some(p.time());
                self.video.push_back(p);
            }
            Pkt::Audio(..) => {
                self.seen_audio = Some(p.time());
                self.audio.push_back(p);
            }
        }
    }

    /// The next packet that can go out in time order. A packet waits until
    /// the other track has caught up to it, unless that track has stalled
    /// (queue limits) or `flush` is set.
    fn pop(&mut self, flush: bool) -> Option<Pkt> {
        let v = self.video.front().map(Pkt::time);
        let a = self.audio.front().map(Pkt::time);
        let video_first = match (v, a) {
            (Some(v), Some(a)) => v <= a,
            (Some(v), None) => {
                if flush || self.seen_audio.is_some_and(|s| v <= s) || self.video.len() > 90 {
                    true
                } else {
                    return None;
                }
            }
            (None, Some(a)) => {
                if flush || self.seen_video.is_some_and(|s| a <= s) || self.audio.len() > 150 {
                    false
                } else {
                    return None;
                }
            }
            (None, None) => return None,
        };
        if video_first {
            self.video.pop_front()
        } else {
            self.audio.pop_front()
        }
    }
}

struct Destination {
    index: usize,
    cfg: StreamDestination,
    rx: Receiver<Pkt>,
    shared: Arc<DestShared>,
    force_key: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    status: Arc<Mutex<Status>>,
    waker: Waker,
    auto_reconnect: bool,
    max_retries: u32,
    audio: AudioConfig,
    fps: u32,
    video_kbps: u32,
    audio_kbps: u32,
}

/// How a connection ended.
enum Ended {
    /// The stream is over.
    Done,
    Failed(anyhow::Error),
}

impl Destination {
    fn update(&self, f: impl FnOnce(&mut DestStatus)) {
        let mut st = self.status.lock_ok();
        if let Some(d) = st.stream.destinations.get_mut(self.index) {
            f(d);
        }
        if st.stream.on_air_since.is_none() && st.stream.destinations.iter().any(|d| d.state == DestState::Live) {
            st.stream.on_air_since = Some(Instant::now());
        }
        drop(st);
        (self.waker)();
    }

    fn log(&self, level: LogLevel, text: String) {
        self.status.lock_ok().push(level, text);
    }

    fn run(self) {
        let mut attempts = 0u32;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                break;
            }
            self.update(|d| d.state = DestState::Connecting);
            let ended = match Output::connect(&self.cfg) {
                Ok(out) => {
                    while self.rx.try_recv().is_ok() {} // stale packets
                    self.shared.need_key.store(true, Ordering::Relaxed);
                    self.shared.connected.store(true, Ordering::Relaxed);
                    self.force_key.store(true, Ordering::Relaxed);
                    let r = self.session(out, &mut attempts);
                    self.shared.connected.store(false, Ordering::Relaxed);
                    r
                }
                Err(e) => Ended::Failed(e),
            };
            match ended {
                Ended::Done => break,
                Ended::Failed(e) => {
                    if !self.retry(e, &mut attempts) {
                        break;
                    }
                }
            }
        }
    }

    /// After a failure: wait and return true to try again, or false to stop.
    fn retry(&self, e: anyhow::Error, attempts: &mut u32) -> bool {
        if self.stop.load(Ordering::Relaxed) {
            return false;
        }
        *attempts += 1;
        let msg = format!("{e:#}");
        let n = *attempts;
        if self.auto_reconnect && (self.max_retries == 0 || n <= self.max_retries) {
            let wait = (2u64 << n.min(4)).min(30);
            let at = Instant::now() + Duration::from_secs(wait);
            self.update(|d| {
                d.state = DestState::Retrying { at };
                d.message = Some(msg.clone());
                d.attempts = n;
                d.kbps = 0.0;
            });
            self.log(LogLevel::Warning, format!("Stream to {}: {msg}. Reconnecting in {wait} s.", self.cfg.name));
            while Instant::now() < at {
                if self.stop.load(Ordering::Relaxed) {
                    return false;
                }
                std::thread::sleep(Duration::from_millis(100));
                (self.waker)(); // keep the countdown moving
            }
            true
        } else {
            self.update(|d| {
                d.state = DestState::Failed;
                d.message = Some(msg.clone());
                d.attempts = n;
                d.kbps = 0.0;
            });
            self.log(LogLevel::Error, format!("Stream to {} stopped: {msg}", self.cfg.name));
            false
        }
    }

    fn session(&self, mut out: Output, attempts: &mut u32) -> Ended {
        let mut q = Interleaver::default();
        let mut base: Option<Ns> = None;
        let mut config: Option<Arc<VideoConfig>> = None;
        let connected_at = Instant::now();
        let mut last_sample = Instant::now();
        let mut last_bytes = 0u64;
        let mut kbps = 0.0f64;
        let mut live = false;
        let send = |out: &mut Output, p: Pkt, base: &mut Option<Ns>, config: &mut Option<Arc<VideoConfig>>| -> Result<()> {
            match p {
                Pkt::Video(v) => {
                    if base.is_none() {
                        if !v.keyframe {
                            return Ok(());
                        }
                        *base = Some(v.dts);
                    }
                    if config.as_ref().is_none_or(|c| c.record != v.config.record) {
                        if !v.keyframe {
                            return Ok(());
                        }
                        out.configure(&v.config, &self.audio, self.fps, self.video_kbps, self.audio_kbps)?;
                        *config = Some(v.config.clone());
                    }
                    let b = base.unwrap();
                    out.video(v.pts - b, v.dts - b, v.keyframe, &v.data)
                }
                Pkt::Audio(t, data) => match *base {
                    Some(b) if t >= b && config.is_some() => out.audio(t - b, &data),
                    _ => Ok(()),
                },
            }
        };
        loop {
            let closed = match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(p) => {
                    q.push(p);
                    while let Ok(p) = self.rx.try_recv() {
                        q.push(p);
                    }
                    false
                }
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => true,
            };
            let r = (|| -> Result<()> {
                while let Some(p) = q.pop(closed) {
                    send(&mut out, p, &mut base, &mut config)?;
                }
                out.poll()
            })();
            if let Err(e) = r {
                return Ended::Failed(e);
            }
            if closed || (self.stop.load(Ordering::Relaxed) && self.rx.is_empty()) {
                out.close();
                return Ended::Done;
            }
            if last_sample.elapsed() >= Duration::from_secs(1) {
                let bytes = out.bytes_sent();
                let dt = last_sample.elapsed().as_secs_f64();
                let rate = bytes.saturating_sub(last_bytes) as f64 * 8.0 / 1000.0 / dt;
                kbps = if kbps == 0.0 { rate } else { kbps * 0.6 + rate * 0.4 };
                last_bytes = bytes;
                last_sample = Instant::now();
                let congested = self.shared.congested.swap(false, Ordering::Relaxed);
                let became_live = !live && config.is_some();
                if became_live {
                    live = true;
                    if *attempts > 0 {
                        self.log(LogLevel::Info, format!("Stream to {}: back live", self.cfg.name));
                    }
                }
                // Stable for a minute: forget earlier failures.
                if live && connected_at.elapsed() > Duration::from_secs(60) {
                    *attempts = 0;
                }
                let n = *attempts;
                self.update(|d| {
                    d.kbps = kbps;
                    d.congested = congested;
                    d.attempts = n;
                    if live {
                        d.state = DestState::Live;
                        d.message = None;
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The LIVE clock starts when a destination is really on air, not
    /// when Go Live was pressed (numbers audit #5).
    #[test]
    fn live_clock_waits_for_the_connection() {
        let connecting = StreamStatus { since: Some(Instant::now() - std::time::Duration::from_secs(7)), ..Default::default() };
        assert_eq!(connecting.live_clock_from(), None);
        assert_eq!(connecting.live_seconds(), 0.0);
        let on_air = StreamStatus { on_air_since: Some(Instant::now() - std::time::Duration::from_secs(2)), ..connecting };
        assert!((on_air.live_seconds() - 2.0).abs() < 0.5);
    }

    /// "Retrying in 0 s" never shows, and the first retry says its try
    /// number too (numbers audit #19).
    #[test]
    fn retry_countdown_and_try_number() {
        let d = |secs_ms: u64, attempts| DestStatus {
            name: "YouTube".into(),
            service: Service::ALL[0],
            state: DestState::Retrying { at: Instant::now() + std::time::Duration::from_millis(secs_ms) },
            kbps: 0.0,
            message: None,
            attempts,
            congested: false,
        };
        assert_eq!(describe_dest(&d(1_500, 1)), "Retrying in 2 s (try 2)");
        assert_eq!(describe_dest(&d(300, 3)), "Retrying in 1 s (try 4)");
        assert_eq!(describe(&DestState::Retrying { at: Instant::now() }), "Retrying now…");
        assert_eq!(describe_dest(&DestStatus { state: DestState::Live, ..d(0, 2) }), "Live");
    }

    fn v(t: Ns) -> Pkt {
        Pkt::Video(VideoPacket {
            pts: t,
            dts: t,
            keyframe: true,
            data: Arc::new(vec![]),
            config: Arc::new(crate::media::mp4::tests::fake_video_config()),
        })
    }
    fn a(t: Ns) -> Pkt {
        Pkt::Audio(t, Arc::new(vec![]))
    }

    #[test]
    fn bitrate_eases_off_and_comes_back() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut a = Adapt::new(6000, t0);
        assert_eq!(a.step(0.0, at(1)), None, "all clear: as set");
        assert_eq!(a.step(0.5, at(3)), Some(4500), "backing up: down a quarter");
        assert_eq!(a.step(0.5, at(4)), None, "not again straight away");
        assert_eq!(a.step(0.5, at(5)), Some(3375));
        for s in 7..30 {
            a.step(0.9, at(s));
        }
        assert_eq!(a.current, 1500, "never below a quarter");
        // Clear again: nothing for 15 s, then back up a step at a time.
        assert_eq!(a.step(0.0, at(40)), None);
        assert_eq!(a.step(0.0, at(43)), None);
        assert_eq!(a.step(0.0, at(44)), Some(2100), "15 s after the last backlog");
        assert_eq!(a.step(0.0, at(50)), None, "steps are spaced out");
        assert_eq!(a.step(0.1, at(57)), None, "a little backlog holds it");
        let mut s = 57;
        while a.current < 6000 {
            s += 1;
            a.step(0.0, at(s));
            assert!(s < 400);
        }
        assert_eq!(a.current, 6000, "back to what was set, no further");
        a.disabled = true;
        assert_eq!(a.step(0.9, at(s + 10)), None);
    }

    #[test]
    fn interleaves_in_time_order() {
        let mut q = Interleaver::default();
        q.push(v(0));
        q.push(v(33));
        assert!(q.pop(false).is_none(), "waits for audio");
        q.push(a(0));
        q.push(a(21));
        q.push(a(42));
        let mut times = Vec::new();
        while let Some(p) = q.pop(false) {
            times.push((matches!(p, Pkt::Video(_)), p.time()));
        }
        assert_eq!(times, vec![(true, 0), (false, 0), (false, 21), (true, 33)]);
        // a(42) waits for video to pass 42 ...
        q.push(v(66));
        assert_eq!(q.pop(false).map(|p| p.time()), Some(42));
        // ... and v(66) for audio.
        assert!(q.pop(false).is_none());
        assert_eq!(q.pop(true).map(|p| p.time()), Some(66));
    }
}
