//! The media engine: captures every source, composites the program on the
//! GPU, mixes audio, and feeds the recorder and the live stream.
//!
//! ```text
//!  camera ─► SourceSlot ─┬─► compositor (GPU) ─┬─► program monitor + multiview
//!                        │        ▲            ├─► [program file]
//!                        │   switcher layout   └─► [stream encoder] ─► RTMP / SRT
//!                        └─► [ISO file]
//!  audio in ─► mixer (drift-corrected) ─┬─► meters
//!         │                             ├─► [AAC for every file]
//!         │                             └─► [stream AAC]
//!         └─► [WAV file]
//! ```
//! Everything is timestamped on one host clock. The compositor renders a
//! little behind real time (`VIDEO_LATENCY`) so every source's frame for an
//! instant has arrived; the mixer does the same for audio. Recordings and
//! streams start and end on the program's frame grid, so all files line up.
//! Branches in [brackets] are taps that exist only while recording or live.

pub mod audio;
pub mod autoswitch;
pub mod captions;
pub mod compositor;
pub mod clips;
pub mod cutout;
pub mod decode;
pub mod dsp;
pub mod encode;
pub mod export;
pub mod feed;
pub mod framing;
pub mod ptz;
pub mod vertical;
pub mod replay;
pub mod graphics;
pub mod guests;
pub mod health;
pub mod inputs;
pub mod lipsync;
pub mod ndi;
pub mod netsrc;
pub mod opus;
pub mod pads;
pub mod player;
pub mod podcast;
pub mod precheck;
pub mod recorder;
pub mod sessions;
pub mod sources;
pub mod stinger;
pub mod streamer;
pub mod switcher;
pub mod take;
pub mod text;
pub mod timers;
pub mod watchdog;

use crate::util::LockExt;
use crate::config::{parse_color, Config, TransitionMode};
use crate::media::{clock, AudioBlock, Ns, VideoFrame, NS_PER_SEC};
use anyhow::{bail, Context, Result};
use audio::{InputShared, MixInput, MixShared, MixTaps};
use compositor::{Compositor, CompositorSettings, Wants};
use crossbeam_channel::{bounded, Sender};
use sources::SourceSlot;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use streamer::{StreamSession, StreamStatus};
use switcher::Switcher;

pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// How far behind real time the program is rendered, so that every
/// source's frame for that instant has been delivered.
pub const VIDEO_LATENCY: Ns = 50_000_000;

// ---------------------------------------------------------------------------
// Shared state the UI reads

pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Latest preview picture (program monitor or a multiview tile).
pub struct FrameSlot {
    frame: Mutex<Option<Arc<Frame>>>,
    generation: AtomicU64,
    last: Mutex<Option<Instant>>,
    born: Instant,
}

impl FrameSlot {
    pub fn new() -> Arc<Self> {
        Arc::new(FrameSlot { frame: Mutex::new(None), generation: AtomicU64::new(0), last: Mutex::new(None), born: Instant::now() })
    }
    pub(crate) fn put(&self, f: Frame) {
        *self.frame.lock_ok() = Some(Arc::new(f));
        *self.last.lock_ok() = Some(Instant::now());
        self.generation.fetch_add(1, Ordering::Release);
    }
    /// (generation, frame); generation changes whenever a new frame arrives.
    pub fn latest(&self) -> Option<(u64, Arc<Frame>)> {
        let f = self.frame.lock_ok().clone()?;
        Some((self.generation.load(Ordering::Acquire), f))
    }
    /// Forget the picture.
    pub fn clear(&self) {
        *self.frame.lock_ok() = None;
        self.generation.fetch_add(1, Ordering::Release);
    }
    /// How long since the last frame (None if there's never been one).
    pub fn since_last(&self) -> Option<Duration> {
        self.last.lock_ok().map(|t| t.elapsed())
    }
    /// How long it's been waiting for pictures.
    pub fn age(&self) -> Duration {
        self.born.elapsed()
    }
    /// True when frames have stopped arriving.
    pub fn stale(&self, after: Duration) -> bool {
        self.last.lock_ok().map(|t| t.elapsed() > after).unwrap_or(true)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct LogLine {
    pub time: String,
    pub level: LogLevel,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct Meter {
    pub peak_db: Vec<f32>,
    pub rms_db: Vec<f32>,
}

#[derive(Debug, Clone, Default)]
pub enum RecState {
    #[default]
    Idle,
    Recording {
        since: Instant,
        folder: PathBuf,
    },
    Finalizing {
        folder: PathBuf,
        /// What it's doing now, and how far along it is (0 to 1).
        step: String,
        done: f32,
    },
}

#[derive(Default)]
pub struct Status {
    /// Keys: "a0", "a1", ... for inputs, "mix" for the program mix.
    pub meters: HashMap<String, Meter>,
    pub log: VecDeque<LogLine>,
    pub rec: RecState,
    pub last_take: Option<recorder::TakeReport>,
    /// Names of sources/inputs that aren't delivering.
    pub offline: HashSet<String>,
    /// Set when writing a file fails (e.g. disk full); the take should be stopped.
    pub rec_failed: Option<String>,
    /// Unrecoverable engine error.
    pub fatal: Option<String>,
    pub stream: StreamStatus,
    /// Sources, files and inputs keeping up (updated every second).
    pub health: health::Health,
    /// Recording drive and backup drive.
    pub disk: Option<health::DiskStatus>,
    pub backup_disk: Option<health::DiskStatus>,
    /// Markers in the take being recorded.
    pub markers: usize,
    pub loudness: audio::MixLoudness,
    pub auto_switch: autoswitch::AutoStatus,
    /// Work after a take (making the podcast copy): what, and how far (0..1).
    pub background: Option<(String, f32)>,
}

impl Status {
    pub fn push(&mut self, level: LogLevel, text: impl Into<String>) {
        let text = text.into();
        if self.log.back().is_some_and(|l| l.text == text) {
            return;
        }
        match level {
            LogLevel::Error => eprintln!("error: {text}"),
            LogLevel::Warning => eprintln!("warning: {text}"),
            LogLevel::Info => eprintln!("{text}"),
        }
        self.log.push_back(LogLine { time: chrono::Local::now().format("%H:%M:%S").to_string(), level, text });
        while self.log.len() > 200 {
            self.log.pop_front();
        }
    }
}

pub struct SourceHandle {
    /// Identity (`SourceConfig::uid`).
    pub uid: u64,
    pub name: String,
    /// Multiview tile.
    pub preview: Arc<FrameSlot>,
    pub size: (u32, u32),
    pub fps: u32,
    pub record: bool,
    pub slot: Arc<SourceSlot>,
    pub input: Arc<inputs::VideoInput>,
}

impl SourceHandle {
    pub fn description(&self) -> String {
        self.input.description()
    }
}

pub struct AudioHandle {
    /// Identity (`AudioConfig::uid`).
    pub uid: u64,
    pub name: String,
    pub record: bool,
    pub split: bool,
    pub shared: Arc<InputShared>,
    pub input: Arc<inputs::AudioInputDev>,
}

impl AudioHandle {
    pub fn description(&self) -> String {
        self.input.description()
    }
}

#[derive(Debug, Clone)]
pub struct EngineInfo {
    pub compositor: String,
    pub canvas: (u32, u32),
    pub fps: u32,
}

/// Running NDI outputs.
pub struct NdiOutputs {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    video_tap: Option<u64>,
    mix_tap: Option<u64>,
    watchers: Vec<(Arc<SourceSlot>, u64)>,
    connections: Arc<AtomicU32>,
}

/// Consumers of composited video.
#[derive(Default)]
pub struct VideoTaps {
    pub program: Option<ProgramTap>,
    pub stream: Option<StreamTap>,
    /// The vertical version: recorded, and streamed.
    pub tall: Option<ProgramTap>,
    pub tall_stream: Option<StreamTap>,
    /// Other consumers of every program frame (the virtual camera).
    pub listeners: Vec<(u64, Sender<VideoFrame>)>,
}

pub struct ProgramTap {
    pub tx: Sender<VideoFrame>,
    /// First frame time wanted.
    pub from: Ns,
}

pub struct StreamTap {
    pub tx: Sender<VideoFrame>,
    pub from: Ns,
    pub size: (u32, u32),
    pub fps: u32,
}

/// Time of frame `k` on a frame grid.
pub fn grid_time(k: i64, fps: u32) -> Ns {
    (k as i128 * NS_PER_SEC as i128 / fps.max(1) as i128) as Ns
}

/// Index of the first grid point at or after `t`.
pub fn grid_index(t: Ns, fps: u32) -> i64 {
    let mut k = (t as i128 * fps.max(1) as i128 / NS_PER_SEC as i128) as i64;
    while grid_time(k, fps) < t {
        k += 1;
    }
    k
}

/// Next program frame time comfortably in the future (start/end of a take
/// or stream).
pub fn next_boundary(fps: u32) -> Ns {
    grid_time(grid_index(clock::now() + 20_000_000, fps), fps)
}

/// Crop/colour/key per source and the frame style, as the compositor uses them.
pub struct LookState {
    pub generation: u64,
    pub looks: Vec<crate::config::SourceLook>,
    pub style: crate::config::FrameStyle,
}

/// The vertical version's settings, for the compositor.
pub struct TallState {
    pub generation: u64,
    pub cfg: crate::config::VerticalConfig,
    /// The sources' names (to find layouts and framing by name).
    pub names: Vec<String>,
}

type CaptionsSlot = Arc<Mutex<Option<(u64, Arc<captions::Captions>)>>>;

/// The sources and inputs as they are now, for the background work that
/// watches them (the watchdog, the health figures). It changes when they're
/// added, removed or moved.
#[derive(Default)]
pub struct Lineup {
    pub generation: u64,
    pub video: Vec<Arc<inputs::VideoInput>>,
    pub audio: Vec<Arc<inputs::AudioInputDev>>,
}

pub type SharedLineup = Arc<Mutex<Lineup>>;

/// A new line-up of sources for the compositor thread, which takes it up
/// between frames (with the switcher, so what's on air follows).
struct CompLineup {
    generation: u64,
    /// For each source now: the number it had (None: new).
    from: Vec<Option<usize>>,
    sizes: Vec<(u32, u32)>,
    slots: Vec<Arc<SourceSlot>>,
    thumbs: Vec<Arc<FrameSlot>>,
    masks: Vec<Arc<cutout::MaskSlot>>,
    looks: Vec<crate::config::SourceLook>,
    style: crate::config::FrameStyle,
    aspects: Vec<f32>,
    uids: Vec<u64>,
}

pub struct Engine {
    cfg: Arc<Mutex<Config>>,
    pub info: EngineInfo,
    pub sources: Vec<SourceHandle>,
    pub audio: Vec<AudioHandle>,
    pub program_preview: Arc<FrameSlot>,
    /// The vertical version's monitor picture (while it's on).
    pub tall_preview: Arc<FrameSlot>,
    tall: Arc<Mutex<TallState>>,
    /// The preview bus (next scene), when preview/program switching is on.
    pub preview_bus: Arc<FrameSlot>,
    pub graphics: Arc<graphics::Graphics>,
    looks: Arc<Mutex<LookState>>,
    /// Width of the program monitor picture.
    pub preview_width: Arc<AtomicU32>,
    pub switcher: Arc<Switcher>,
    pub status: Arc<Mutex<Status>>,
    video_taps: Arc<Mutex<VideoTaps>>,
    mix_taps: MixTaps,
    tap_ids: AtomicU64,
    stream: Mutex<Option<StreamSession>>,
    stream_started: Mutex<Option<Ns>>,
    take: Mutex<Option<recorder::Take>>,
    /// Background jobs finishing files/streams; joined on shutdown.
    finalizers: Mutex<Vec<JoinHandle<()>>>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
    waker: Waker,
    audio_latency: Ns,
    pub mix: Arc<MixShared>,
    take_monitor: Arc<Mutex<Option<health::TakeMonitor>>>,
    pub auto: Arc<autoswitch::AutoShared>,
    /// Headphone output: (device, source, tap id, running output).
    monitor: Mutex<Option<(String, String, u64, Box<dyn sources::Running>)>>,
    /// Extra outputs (presenters' headphones, mix-minus returns).
    outputs: Mutex<Vec<RunningOutput>>,
    /// Sound pads (played through the "pads" input strip).
    pub pads: Arc<pads::PadBank>,
    /// The last stretch of the recording, for instant clips.
    clips: Arc<clips::ClipBuffer>,
    /// The last stretch of each source's own file, for replays from any
    /// camera: (source name, its buffer). Empty when not recording.
    angles: Mutex<Vec<(String, Arc<clips::ClipBuffer>)>>,
    /// The stinger transition's video.
    pub stinger: Arc<stinger::StingerPlayer>,
    /// Instant replays.
    pub replay: Arc<replay::ReplayPlayer>,
    replay_label: Arc<AtomicBool>,
    /// NDI outputs, and what they were started for.
    ndi_out: Mutex<Option<(crate::config::NdiOutConfig, NdiOutputs)>>,
    monitor_volume: Arc<AtomicU32>,
    /// The virtual camera, while on: (tap id, sender).
    vcam: Mutex<Option<(u64, Box<dyn sources::Running>)>>,
    /// Live captions (with the mix tap feeding them) and their settings.
    captions: CaptionsSlot,
    captions_cfg: Mutex<Option<crate::config::CaptionsConfig>>,
    /// Background removal: each source's masks, and what makes them.
    masks: Vec<Arc<cutout::MaskSlot>>,
    cutouts: Mutex<Vec<CutoutState>>,
    /// Sound from sources and players, by name (for inputs added later).
    hub: Arc<feed::AudioHub>,
    /// The line-up, for the watchdog and health figures.
    lineup: SharedLineup,
    /// The inputs by name, for automatic switching.
    auto_inputs: Arc<Mutex<Vec<(String, Arc<InputShared>)>>>,
    /// A line-up waiting for the compositor, the last it took up, and a count.
    comp_lineup: Arc<Mutex<Option<CompLineup>>>,
    comp_done: Arc<AtomicU64>,
    comp_lineups: AtomicU64,
}

/// An extra audio output while it runs.
struct RunningOutput {
    cfg: crate::config::AudioOutput,
    tap: u64,
    volume: Arc<AtomicU32>,
    running: Box<dyn sources::Running>,
}

/// An output's feed: the mix it takes, without the inputs it leaves out
/// (by name), with talkback if it has it.
fn output_kind_for(o: &crate::config::AudioOutput, inputs: &[String]) -> audio::MixKind {
    let mut minus = 0u64;
    for name in &o.minus {
        if let Some(i) = inputs.iter().position(|a| a.eq_ignore_ascii_case(name.trim())) {
            if i < 64 {
                minus |= 1 << i;
            }
        }
    }
    audio::MixKind::Feed { stream: o.mix.trim().eq_ignore_ascii_case("stream"), minus, talkback: o.talkback }
}

fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db.clamp(-60.0, 12.0) / 20.0)
}

/// The talkback mic's input number, if one is set and exists.
fn talkback_index(cfg: &Config) -> Option<usize> {
    let name = cfg.mix.talkback_input.trim();
    (!name.is_empty()).then(|| cfg.audio.iter().position(|a| a.name.trim().eq_ignore_ascii_case(name))).flatten()
}

/// Background removal for one source.
#[derive(Default)]
enum CutoutState {
    #[default]
    Off,
    On(u8, #[allow(dead_code)] cutout::Cutout), // runs while held
    /// Couldn't start with this quality (not retried until it changes).
    Failed(u8),
}

impl Engine {
    /// Open every source, start the compositor and mixer. An unexpected
    /// failure anywhere becomes an error message rather than a crash.
    pub fn start(cfg: &Config, waker: Waker) -> Result<Engine> {
        crate::util::guard("Starting the engine", || Self::start_inner(cfg, waker))
    }

    fn start_inner(cfg: &Config, waker: Waker) -> Result<Engine> {
        cfg.validate()?;
        let mut cfg = cfg.with_framed_rates();
        // (The app gives them identities; started on its own, it has none yet.)
        cfg.assign_uids();
        let cfg = &cfg;
        let mut device_cache = inputs::DeviceCache::default();
        let status = Arc::new(Mutex::new(Status::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (cw, ch, fps) = (cfg.program.width, cfg.program.height, cfg.program.fps);

        // ---- compositor ------------------------------------------------------
        let bg = parse_color(&cfg.program.background_color).unwrap_or(0xff10131a);
        let background_image = match &cfg.program.background_image {
            Some(p) if !p.as_os_str().is_empty() => match image::open(p) {
                Ok(img) => Some(img.to_rgba8()),
                Err(e) => {
                    status.lock_ok().push(LogLevel::Warning, format!("Background image {}: {e}", p.display()));
                    None
                }
            },
            _ => None,
        };
        let comp_settings = CompositorSettings {
            canvas: (cw, ch),
            background_rgb: [(bg >> 16) as u8, (bg >> 8) as u8, bg as u8],
            background_image,
            preview_width: cfg.program.preview_width.clamp(160, cw),
            thumb_width: cfg.program.multiview_width.clamp(64, 1920),
        };
        let mut comp = crate::util::guard("The GPU compositor", || Compositor::new(&comp_settings)).context("starting the GPU compositor")?;
        let compositor_name = comp.adapter_name.clone();

        // ---- video sources -----------------------------------------------------
        // A device that can't be opened now is retried in the background
        // (see watchdog), so plugging it in later just works.
        let hub = feed::AudioHub::new();
        let pads = pads::PadBank::new(&cfg.pads);
        *hub.pads.lock_ok() = Some(pads.clone());
        let mut sources = Vec::new();
        let mut aspects = Vec::new();
        // Every source's frames first: an auto-framed source is cut from
        // another (and keeps that camera's delay).
        let delay_of = |sc: &crate::config::SourceConfig| {
            let parent = sc.framed_from().and_then(|from| cfg.sources.iter().find(|p| p.name.trim().eq_ignore_ascii_case(from)));
            parent.unwrap_or(sc).delay_ms
        };
        let slots: Vec<Arc<SourceSlot>> = cfg.sources.iter().enumerate().map(|(i, sc)| SourceSlot::new(i, delay_of(sc), VIDEO_LATENCY)).collect();
        let peers: Arc<Vec<(String, Arc<SourceSlot>)>> = Arc::new(cfg.sources.iter().zip(&slots).map(|(sc, s)| (sc.name.clone(), s.clone())).collect());
        let inputs: Vec<Arc<inputs::VideoInput>> = cfg.sources.iter().enumerate().map(|(i, sc)| inputs::VideoInput::new(i, sc.clone(), slots[i].clone(), hub.clone(), peers.clone())).collect();
        let devs: Vec<Vec<crate::devices::DeviceInfo>> = cfg.sources.iter().map(|sc| device_cache.video(&sc.device).to_vec()).collect();
        // Opened side by side: cameras can take a second or more each to
        // start, and the window waits for this.
        let opened: Vec<Result<String>> = std::thread::scope(|scope| {
            let jobs: Vec<_> = inputs.iter().zip(&devs).map(|(input, devs)| scope.spawn(move || input.open(devs))).collect();
            jobs.into_iter().map(|j| j.join().unwrap_or_else(|_| Err(anyhow::anyhow!("opening it failed unexpectedly")))).collect()
        });
        for (((i, sc), input), opened) in cfg.sources.iter().enumerate().zip(inputs).zip(opened) {
            let slot = slots[i].clone();
            match opened {
                Err(e) => {
                    let mut st = status.lock_ok();
                    st.push(LogLevel::Error, format!("{}: {e:#}. It will connect automatically when available.", sc.name));
                    st.offline.insert(sc.name.clone());
                }
                Ok(_) => {
                    if let Some(p) = input.framing().and_then(|f| f.problem.lock_ok().clone()) {
                        status.lock_ok().push(LogLevel::Warning, format!("{}: showing the whole picture ({p})", sc.name));
                    }
                }
            }
            comp.add_source(sc.width, sc.height);
            aspects.push(sc.look.cropped_aspect(sc.width as f32 / sc.height as f32));
            sources.push(SourceHandle {
                uid: sc.uid,
                name: sc.name.clone(),
                preview: FrameSlot::new(),
                size: (sc.width, sc.height),
                fps: sc.fps,
                record: sc.record,
                slot,
                input,
            });
        }

        // ---- audio -------------------------------------------------------------
        let mut audio = Vec::new();
        let mut mix_inputs = Vec::new();
        for (i, ac) in cfg.audio.iter().enumerate() {
            let (tx, rx) = bounded(1000);
            let shared = InputShared::new(ac);
            let input = inputs::AudioInputDev::new(i, ac.clone(), tx, shared.clone(), hub.clone());
            let devs = device_cache.audio(&ac.device).to_vec();
            match input.open(&devs) {
                Ok((_, Some(note))) => status.lock_ok().push(LogLevel::Warning, format!("{}: {note}", ac.name)),
                Ok(_) => {}
                Err(e) => {
                    let mut st = status.lock_ok();
                    st.push(LogLevel::Error, format!("{}: {e:#}. It will connect automatically when available.", ac.name));
                    st.offline.insert(ac.name.clone());
                }
            }
            mix_inputs.push(MixInput {
                rx,
                left: ac.program_left,
                right: ac.program_right,
                delay: ac.delay_ms as i64 * 1_000_000,
                shared: shared.clone(),
                gaps_expected: input.gaps_expected(),
            });
            audio.push(AudioHandle { uid: ac.uid, name: ac.name.clone(), record: ac.record, split: ac.split_channels, shared, input });
        }
        let max_audio_delay = cfg.audio.iter().map(|a| a.delay_ms).max().unwrap_or(0) as i64 * 1_000_000;
        let audio_latency = audio::mix_latency(max_audio_delay);
        let mix_taps: MixTaps = Arc::new(Mutex::new(Vec::new()));
        let mix = MixShared::new(&cfg.mix);
        mix.set_talkback_input(talkback_index(cfg));

        let mut threads = Vec::new();
        {
            let (taps, status, stop, mix) = (mix_taps.clone(), status.clone(), stop.clone(), mix.clone());
            let names: Vec<String> = audio.iter().map(|a| a.name.clone()).collect();
            threads.push(
                std::thread::Builder::new()
                    .name("mixer".into())
                    .spawn(move || audio::run_mixer(mix_inputs, names, taps, mix, status, stop, audio_latency))?,
            );
        }

        // ---- switcher + compositor thread --------------------------------------
        let scenes = cfg.effective_scenes();
        let mut initial = scenes.first().cloned().unwrap_or_else(|| crate::scene::grid(sources.len()));
        initial.retain_sources(sources.len());
        let switcher = Arc::new(Switcher::new((cw, ch), aspects, initial));
        switcher.set_uids(sources.iter().map(|s| s.uid).collect());
        switcher.set_style(cfg.transition.direction, &cfg.transition.dip_color);
        let program_preview = FrameSlot::new();
        let tall_preview = FrameSlot::new();
        let tall = Arc::new(Mutex::new(TallState { generation: 1, cfg: cfg.vertical.clone(), names: cfg.sources.iter().map(|s| s.name.clone()).collect() }));
        let preview_bus = FrameSlot::new();
        let graphics = graphics::Graphics::new(&cfg.graphics, (cw, ch), timers::Timers::new());
        let looks = Arc::new(Mutex::new(LookState { generation: 1, looks: cfg.sources.iter().map(|s| s.look.clone()).collect(), style: cfg.style.clone() }));
        let preview_width = Arc::new(AtomicU32::new(cfg.program.preview_width));
        let video_taps = Arc::new(Mutex::new(VideoTaps::default()));
        let compositor_skips = Arc::new(AtomicU64::new(0));
        let captions_slot: CaptionsSlot = Arc::new(Mutex::new(None));
        let masks: Vec<Arc<cutout::MaskSlot>> = sources.iter().map(|_| cutout::MaskSlot::new()).collect();
        let stinger = Arc::new(stinger::StingerPlayer::new((cw, ch)));
        stinger.configure(&cfg.transition.stinger);
        let replay = Arc::new(replay::ReplayPlayer::new((cw, ch)));
        let replay_label = Arc::new(AtomicBool::new(cfg.replay.label));
        let comp_lineup: Arc<Mutex<Option<CompLineup>>> = Arc::new(Mutex::new(None));
        let comp_done = Arc::new(AtomicU64::new(0));
        {
            let ctx = CompositorLoop {
                lineup: comp_lineup.clone(),
                lineup_done: comp_done.clone(),
                stinger: stinger.clone(),
                replay: replay.clone(),
                replay_label: replay_label.clone(),
                comp,
                slots: sources.iter().map(|s| s.slot.clone()).collect(),
                thumbs: sources.iter().map(|s| s.preview.clone()).collect(),
                preview: program_preview.clone(),
                preview_bus: preview_bus.clone(),
                tall: tall.clone(),
                tall_generation: 0,
                tall_on: None,
                tall_preview: tall_preview.clone(),
                graphics: graphics.clone(),
                looks: looks.clone(),
                looks_generation: 0,
                preview_width: preview_width.clone(),
                switcher: switcher.clone(),
                taps: video_taps.clone(),
                status: status.clone(),
                stop: stop.clone(),
                waker: waker.clone(),
                fps,
                skips: compositor_skips.clone(),
                captions: captions_slot.clone(),
                masks: masks.clone(),
            };
            threads.push(std::thread::Builder::new().name("compositor".into()).spawn(move || ctx.run())?);
        }

        // ---- remote guests hear the mix and see the program --------------------------
        guests::hub().attach_engine(guests::EngineLink {
            mix_taps: mix_taps.clone(),
            video_taps: video_taps.clone(),
            inputs: cfg.audio.iter().enumerate().map(|(i, a)| (a.device.clone(), i)).collect(),
            status: status.clone(),
            waker: waker.clone(),
        });

        // ---- watchdog: reconnects devices, keeps health figures current -----------
        let shared_cfg = Arc::new(Mutex::new(cfg.clone()));
        let take_monitor = Arc::new(Mutex::new(None));
        let lineup: SharedLineup = Arc::new(Mutex::new(Lineup { generation: 1, video: sources.iter().map(|s| s.input.clone()).collect(), audio: audio.iter().map(|a| a.input.clone()).collect() }));
        {
            let mut monitor = health::HealthMonitor {
                cfg: shared_cfg.clone(),
                status: status.clone(),
                take: take_monitor.clone(),
                lineup: lineup.clone(),
                compositor_skips: compositor_skips.clone(),
                last_frames: Vec::new(),
                seen: 0,
            };
            let w = watchdog::Watchdog {
                lineup: lineup.clone(),
                status: status.clone(),
                stop: stop.clone(),
                waker: waker.clone(),
                every_second: vec![Box::new(move || monitor.tick())],
            };
            threads.push(std::thread::Builder::new().name("watchdog".into()).spawn(move || w.run())?);
        }

        // ---- automatic switching ----------------------------------------------------
        let auto = autoswitch::AutoShared::new(cfg.auto_switch.enabled);
        let auto_inputs: Arc<Mutex<Vec<(String, Arc<InputShared>)>>> = Arc::new(Mutex::new(audio.iter().map(|a| (a.name.clone(), a.shared.clone())).collect()));
        {
            let (sw, c2, inputs2) = (switcher.clone(), shared_cfg.clone(), auto_inputs.clone());
            let take = Box::new(move |scene: &crate::scene::Scene| {
                let t = c2.lock_ok().transition.clone();
                let mut s = scene.clone();
                s.retain_sources(sw.source_count());
                // A stinger every time someone speaks would be a lot: mix.
                let mode = if t.mode == TransitionMode::Stinger { TransitionMode::Mix } else { t.mode };
                apply_scene_audio(&inputs2.lock_ok(), &s, scene_audio_timing(mode, t.duration_ms));
                sw.take(s, mode, t.duration_ms);
            });
            let a = autoswitch::AutoSwitcher {
                shared: auto.clone(),
                cfg: shared_cfg.clone(),
                inputs: auto_inputs.clone(),
                switcher: switcher.clone(),
                take,
                status: status.clone(),
                stop: stop.clone(),
                waker: waker.clone(),
            };
            threads.push(std::thread::Builder::new().name("auto switch".into()).spawn(move || a.run())?);
        }

        let codec = cfg.output.codec.video_codec();
        let encoder = describe_encoder(codec, &cfg.output.encoder);
        {
            let mut st = status.lock_ok();
            st.push(LogLevel::Info, format!("Program {cw}×{ch} @ {fps} fps on {compositor_name}"));
            st.push(LogLevel::Info, format!("Recording with {encoder}"));
        }
        Ok(Engine {
            cfg: shared_cfg,
            info: EngineInfo { compositor: compositor_name, canvas: (cw, ch), fps },
            sources,
            audio,
            program_preview,
            tall_preview,
            tall,
            preview_bus,
            graphics,
            looks,
            preview_width,
            switcher,
            status,
            video_taps,
            mix_taps,
            tap_ids: AtomicU64::new(1),
            stream: Mutex::new(None),
            stream_started: Mutex::new(None),
            take: Mutex::new(None),
            finalizers: Mutex::new(Vec::new()),
            stop,
            threads,
            waker,
            audio_latency,
            mix,
            take_monitor,
            auto,
            monitor: Mutex::new(None),
            outputs: Mutex::new(Vec::new()),
            pads,
            clips: clips::ClipBuffer::new(),
            angles: Mutex::new(Vec::new()),
            stinger,
            replay,
            replay_label,
            ndi_out: Mutex::new(None),
            monitor_volume: Arc::new(AtomicU32::new(1.0f32.to_bits())),
            vcam: Mutex::new(None),
            captions: captions_slot,
            captions_cfg: Mutex::new(None),
            cutouts: Mutex::new(masks.iter().map(|_| CutoutState::Off).collect()),
            masks,
            hub,
            lineup,
            auto_inputs,
            comp_lineup,
            comp_done,
            comp_lineups: AtomicU64::new(0),
        })
        .map(|e| {
            e.apply_monitor(&cfg.mix);
            e.apply_outputs(&cfg.mix);
            e.apply_captions(&cfg.captions);
            e.apply_ndi_out(&cfg.ndi_out);
            e.apply_cutouts(&cfg.sources);
            e
        })
    }

    // ---- background removal ----------------------------------------------------------------

    /// Start or stop background removal to match each source's settings.
    fn apply_cutouts(&self, sources: &[crate::config::SourceConfig]) {
        let mut states = self.cutouts.lock_ok();
        for (i, sc) in sources.iter().enumerate() {
            let (Some(state), Some(handle)) = (states.get_mut(i), self.sources.get(i)) else { continue };
            let want = sc.look.remove_background.then(|| sc.look.cutout_quality.level());
            let have = match state {
                CutoutState::Off => None,
                CutoutState::On(q, _) | CutoutState::Failed(q) => Some(*q),
            };
            if want == have {
                continue;
            }
            *state = CutoutState::Off; // stops the old one first
            let Some(q) = want else { continue };
            let (status, waker, name) = (self.status.clone(), self.waker.clone(), sc.name.clone());
            let failed = move |e: String| {
                status.lock_ok().push(LogLevel::Warning, format!("{name}: background removal stopped working ({e})"));
                waker();
            };
            *state = match cutout::start(&sc.name, handle.slot.clone(), self.masks[i].clone(), q, failed) {
                Ok(c) => CutoutState::On(q, c),
                Err(e) => {
                    self.status.lock_ok().push(LogLevel::Warning, format!("{}: background removal isn't available ({e:#})", sc.name));
                    CutoutState::Failed(q)
                }
            };
        }
    }

    /// Background removal state of source `i`: masks per second while on.
    pub fn cutout_rate(&self, i: usize) -> Option<f32> {
        let m = self.masks.get(i)?;
        m.on.load(Ordering::Relaxed).then(|| m.rate_x10.load(Ordering::Relaxed) as f32 / 10.0)
    }

    // ---- live captions -------------------------------------------------------------------

    /// Start, stop or restyle live captions to match the settings.
    pub fn apply_captions(&self, cfg: &crate::config::CaptionsConfig) {
        let mut current = self.captions_cfg.lock_ok();
        let same_listening = current.as_ref().is_some_and(|c| c.enabled == cfg.enabled && c.source == cfg.source && c.language == cfg.language && c.model == cfg.model);
        if same_listening {
            if let Some((_, c)) = &*self.captions.lock_ok() {
                c.set_style(cfg);
            }
            *current = Some(cfg.clone());
            return;
        }
        *current = Some(cfg.clone());
        drop(current);
        // Stop what's running.
        if let Some((id, c)) = self.captions.lock_ok().take() {
            self.mix_taps.lock_ok().retain(|t| t.0 != id);
            drop(c);
        }
        if !cfg.enabled {
            return;
        }
        if let Some(p) = crate::platform::speech::problem(&cfg.language, cfg.model) {
            self.status.lock_ok().push(LogLevel::Warning, format!("Live captions: {p}"));
            return;
        }
        let kind = match cfg.source.trim() {
            "" | "recording" => audio::MixKind::Recording,
            "stream" => audio::MixKind::Stream,
            name => match self.audio.iter().position(|a| a.name.eq_ignore_ascii_case(name)) {
                Some(i) => audio::MixKind::Track { input: i, post_fader: false },
                None => {
                    self.status.lock_ok().push(LogLevel::Warning, format!("Live captions: there's no audio input called \"{name}\"; listening to the recording mix"));
                    audio::MixKind::Recording
                }
            },
        };
        let (tx, rx) = bounded(500);
        let id = self.tap_ids.fetch_add(1, Ordering::Relaxed);
        self.mix_taps.lock_ok().push((id, kind, tx));
        let (language, model) = (cfg.language.clone(), cfg.model);
        let c = captions::Captions::start(cfg, self.info.canvas, rx, Box::new(move || crate::platform::speech::start(&language, model)), self.status.clone(), self.waker.clone());
        *self.captions.lock_ok() = Some((id, Arc::new(c)));
    }

    pub fn captions(&self) -> Option<Arc<captions::Captions>> {
        self.captions.lock_ok().as_ref().map(|c| c.1.clone())
    }

    // ---- NDI output -----------------------------------------------------------------------

    /// NDI was installed while Cutaway was running: start the NDI sources
    /// that couldn't open without it, and the NDI output.
    pub fn ndi_arrived(&self) {
        let mut started = Vec::new();
        for s in &self.sources {
            if s.input.cfg.device.trim().to_lowercase().starts_with("ndi:") && !s.input.is_open() {
                match s.input.open(&[]) {
                    Ok(_) => started.push(s.name.clone()),
                    Err(e) => self.status.lock_ok().push(LogLevel::Warning, format!("{}: {e:#}", s.name)),
                }
            }
        }
        let mut st = self.status.lock_ok();
        st.push(LogLevel::Info, if started.is_empty() { "NDI is installed".to_string() } else { format!("NDI is installed: connecting {}", started.join(", ")) });
        drop(st);
        let cfg = self.config();
        self.apply_ndi_out(&cfg.ndi_out);
    }

    /// Start, change or stop the NDI outputs to match the settings.
    pub fn apply_ndi_out(&self, cfg: &crate::config::NdiOutConfig) {
        let mut cur = self.ndi_out.lock_ok();
        if cur.as_ref().map(|c| &c.0) == Some(cfg) {
            return;
        }
        if let Some((_, old)) = cur.take() {
            self.stop_ndi_outputs(old);
        }
        if !cfg.program && !cfg.sources {
            return;
        }
        if let Err(e) = ndi::api_ready() {
            self.status.lock_ok().push(LogLevel::Warning, format!("NDI output: {e:#}"));
            return;
        }
        let mut outs = NdiOutputs { stop: Arc::new(AtomicBool::new(false)), threads: Vec::new(), video_tap: None, mix_tap: None, watchers: Vec::new(), connections: Arc::new(AtomicU32::new(0)) };
        let name = if cfg.name.trim().is_empty() { "Cutaway".to_string() } else { cfg.name.trim().to_string() };
        let fps = self.info.fps;
        if cfg.program {
            match ndi::NdiSender::new(&name) {
                Ok(mut sender) => {
                    let full = Some(sender.source().0).filter(|n| !n.is_empty()).unwrap_or_else(|| name.clone());
                    let (vtx, vrx) = bounded::<VideoFrame>(4);
                    let (atx, arx) = bounded::<AudioBlock>(200);
                    let vid = self.tap_ids.fetch_add(1, Ordering::Relaxed);
                    let aid = self.tap_ids.fetch_add(1, Ordering::Relaxed);
                    self.video_taps.lock_ok().listeners.push((vid, vtx));
                    self.mix_taps.lock_ok().push((aid, audio::MixKind::Recording, atx));
                    outs.video_tap = Some(vid);
                    outs.mix_tap = Some(aid);
                    let (stop, conns) = (outs.stop.clone(), outs.connections.clone());
                    if let Ok(t) = std::thread::Builder::new().name("ndi out".into()).spawn(move || {
                        let mut last_check = Instant::now();
                        while !stop.load(Ordering::Relaxed) {
                            crossbeam_channel::select! {
                                recv(vrx) -> f => match f { Ok(f) => sender.video(&f, fps), Err(_) => break },
                                recv(arx) -> b => match b { Ok(b) => sender.audio(&b), Err(_) => break },
                                default(Duration::from_millis(200)) => {}
                            }
                            if last_check.elapsed() > Duration::from_secs(1) {
                                last_check = Instant::now();
                                conns.store(sender.connections().max(0) as u32, Ordering::Relaxed);
                            }
                        }
                    }) {
                        outs.threads.push(t);
                    }
                    self.status.lock_ok().push(LogLevel::Info, format!("NDI output: the program is on the network as \"{full}\""));
                }
                Err(e) => self.status.lock_ok().push(LogLevel::Warning, format!("NDI output: {e:#}")),
            }
        }
        if cfg.sources {
            for s in &self.sources {
                let Ok(mut sender) = ndi::NdiSender::new(&format!("{name} - {}", s.name)) else { continue };
                let (tx, rx) = bounded::<VideoFrame>(3);
                let wid = (1u64 << 50) + self.tap_ids.fetch_add(1, Ordering::Relaxed);
                s.slot.add_watcher(wid, tx);
                outs.watchers.push((s.slot.clone(), wid));
                let (stop, sfps) = (outs.stop.clone(), s.fps);
                if let Ok(t) = std::thread::Builder::new().name(format!("ndi out {}", s.name)).spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        match rx.recv_timeout(Duration::from_millis(200)) {
                            Ok(f) => sender.video(&f, sfps),
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                            Err(_) => break,
                        }
                    }
                }) {
                    outs.threads.push(t);
                }
            }
        }
        *cur = Some((cfg.clone(), outs));
        (self.waker)();
    }

    fn stop_ndi_outputs(&self, mut outs: NdiOutputs) {
        outs.stop.store(true, Ordering::Relaxed);
        if let Some(id) = outs.video_tap {
            self.video_taps.lock_ok().listeners.retain(|l| l.0 != id);
        }
        if let Some(id) = outs.mix_tap {
            self.mix_taps.lock_ok().retain(|(i, ..)| *i != id);
        }
        for (slot, id) in &outs.watchers {
            slot.remove_watcher(*id);
        }
        for t in outs.threads.drain(..) {
            let _ = t.join();
        }
    }

    /// NDI receivers watching the program (None when it isn't being sent).
    pub fn ndi_watchers(&self) -> Option<u32> {
        let cur = self.ndi_out.lock_ok();
        let (cfg, outs) = cur.as_ref()?;
        cfg.program.then(|| outs.connections.load(Ordering::Relaxed))
    }

    // ---- virtual camera ----------------------------------------------------------------

    pub fn virtual_camera_on(&self) -> bool {
        self.vcam.lock_ok().is_some()
    }

    /// Send the program to the "Cutaway" camera in video apps.
    pub fn set_virtual_camera(&self, on: bool) -> Result<()> {
        let mut v = self.vcam.lock_ok();
        if let Some((id, running)) = v.take() {
            self.video_taps.lock_ok().listeners.retain(|l| l.0 != id);
            drop(running);
        }
        if on {
            let (tx, rx) = bounded(3);
            let running = crate::platform::vcam::start(rx)?;
            let id = self.tap_ids.fetch_add(1, Ordering::Relaxed);
            self.video_taps.lock_ok().listeners.push((id, tx));
            *v = Some((id, running));
            self.status.lock_ok().push(LogLevel::Info, "Virtual camera on: choose \"Cutaway\" as the camera in your video app");
        }
        (self.waker)();
        Ok(())
    }

    // ---- headphone monitoring ---------------------------------------------------------

    /// Start, change or stop the monitor output to match the settings.
    pub fn apply_monitor(&self, mix: &crate::config::MixConfig) {
        self.monitor_volume.store((10f32.powf(mix.monitor_volume_db.clamp(-60.0, 12.0) / 20.0)).to_bits(), Ordering::Relaxed);
        let mut m = self.monitor.lock_ok();
        let device = mix.monitor_device.trim().to_string();
        let source = mix.monitor_source.trim().to_string();
        if m.as_ref().is_some_and(|(d, s, ..)| *d == device && *s == source) {
            return;
        }
        if let Some((_, _, id, running)) = m.take() {
            self.mix_taps.lock_ok().retain(|(i, ..)| *i != id);
            drop(running);
        }
        if device.is_empty() {
            return;
        }
        let kind = if source.eq_ignore_ascii_case("stream") {
            audio::MixKind::Stream
        } else if let Some(i) = self.audio.iter().position(|a| a.name.eq_ignore_ascii_case(&source)) {
            audio::MixKind::Input(i)
        } else {
            audio::MixKind::Recording
        };
        let (tx, rx) = bounded(64);
        match crate::platform::open_audio_output(&device, (1, 2), rx, self.monitor_volume.clone()) {
            Ok(running) => {
                let id = self.tap_ids.fetch_add(1, Ordering::Relaxed);
                self.mix_taps.lock_ok().push((id, kind, tx));
                *m = Some((device, source, id, running));
            }
            Err(e) => self.status.lock_ok().push(LogLevel::Warning, format!("Headphones: {e:#}")),
        }
    }

    // ---- outputs: presenters' headphones, mix-minus returns ------------------------------

    /// The mix kind an output needs.
    fn output_kind(&self, o: &crate::config::AudioOutput) -> audio::MixKind {
        let names: Vec<String> = self.audio.iter().map(|a| a.name.clone()).collect();
        output_kind_for(o, &names)
    }

    /// The same, for inputs listed in settings.
    fn output_kind_in(&self, o: &crate::config::AudioOutput, inputs: &[crate::config::AudioConfig]) -> audio::MixKind {
        let names: Vec<String> = inputs.iter().map(|a| a.name.clone()).collect();
        output_kind_for(o, &names)
    }

    /// Start, change or stop the extra outputs to match the settings.
    pub fn apply_outputs(&self, mix: &crate::config::MixConfig) {
        let mut outs = self.outputs.lock_ok();
        // Volume changes apply in place; anything else reopens that output.
        let mut keep: Vec<RunningOutput> = Vec::new();
        for o in outs.drain(..) {
            match mix.outputs.iter().find(|c| c.device == o.cfg.device && c.left == o.cfg.left && c.right == o.cfg.right && c.mix == o.cfg.mix && c.minus == o.cfg.minus && c.talkback == o.cfg.talkback) {
                Some(c) if !keep.iter().any(|k| k.cfg == *c) => {
                    o.volume.store(db_to_gain(c.volume_db).to_bits(), Ordering::Relaxed);
                    keep.push(RunningOutput { cfg: c.clone(), ..o });
                }
                _ => {
                    self.mix_taps.lock_ok().retain(|t| t.0 != o.tap);
                    drop(o.running);
                }
            }
        }
        for c in &mix.outputs {
            if keep.iter().any(|k| k.cfg == *c) {
                continue;
            }
            let volume = Arc::new(AtomicU32::new(db_to_gain(c.volume_db).to_bits()));
            let (tx, rx) = bounded(64);
            match crate::platform::open_audio_output(&c.device, (c.left, c.right), rx, volume.clone()) {
                Ok(running) => {
                    let tap = self.tap_ids.fetch_add(1, Ordering::Relaxed);
                    self.mix_taps.lock_ok().push((tap, self.output_kind(c), tx));
                    keep.push(RunningOutput { cfg: c.clone(), tap, volume, running });
                }
                Err(e) => self.status.lock_ok().push(LogLevel::Warning, format!("Output \"{}\": {e:#}", c.name)),
            }
        }
        *outs = keep;
    }

    /// Talkback pressed or let go.
    pub fn set_talkback(&self, on: bool) {
        self.mix.set_talkback(on);
    }

    pub fn talkback(&self) -> bool {
        self.mix.talkback()
    }

    // ---- settings ------------------------------------------------------------------

    /// Current settings (as last applied).
    pub fn config(&self) -> Config {
        self.cfg.lock_ok().clone()
    }

    /// Whether `update_config` would take these settings now.
    pub fn check_config(&self, new: &Config) -> Result<()> {
        let old = self.config();
        if old.output.codec != new.output.codec || old.output.encoder != new.output.encoder {
            if self.is_recording() {
                bail!("stop recording before changing the codec or encoder");
            }
            encode::resolve_backend(new.output.codec.video_codec(), &new.output.encoder)?;
        }
        if old.streaming.encoder != new.streaming.encoder {
            encode::resolve_backend(crate::media::VideoCodec::H264, &new.streaming.encoder)?;
        }
        Ok(())
    }

    /// The scenes as saved (automatic switching finds them by name).
    pub fn set_scenes(&self, scenes: &[crate::scene::Scene]) {
        self.cfg.lock_ok().scenes = scenes.to_vec();
    }

    /// Apply settings that don't need the engine rebuilt (recording format,
    /// streaming, naming, ...). Callers check `Config::needs_restart` first.
    /// Takes effect from the next take / next time you go live.
    pub fn update_config(&self, new: &Config) -> Result<()> {
        let old = self.config();
        if old.output.codec != new.output.codec || old.output.encoder != new.output.encoder {
            if self.is_recording() {
                bail!("stop recording before changing the codec or encoder");
            }
            encode::resolve_backend(new.output.codec.video_codec(), &new.output.encoder)?;
        }
        if old.streaming.encoder != new.streaming.encoder {
            encode::resolve_backend(crate::media::VideoCodec::H264, &new.streaming.encoder)?;
        }
        for (i, a) in new.audio.iter().enumerate() {
            self.set_gain(i, a.gain_db);
            if old.audio.get(i).is_none_or(|o| o.in_program != a.in_program) {
                self.set_in_program(i, a.in_program);
            }
            if let Some(h) = self.audio.get(i) {
                h.shared.set_routing(a.to_recording, a.to_stream);
                h.shared.set_processing(&a.processing);
                h.shared.set_duck(a.duck);
                h.shared.set_automix(a.automix);
            }
        }
        self.mix.apply(&new.mix);
        self.mix.set_talkback_input(talkback_index(new));
        self.apply_monitor(&new.mix);
        self.apply_outputs(&new.mix);
        if old.pads != new.pads {
            self.pads.set_pads(&new.pads);
        }
        self.replay_label.store(new.replay.label, Ordering::Relaxed);
        if old.vertical != new.vertical {
            let mut t = self.tall.lock_ok();
            t.cfg = new.vertical.clone();
            t.generation += 1;
        }
        if old.transition != new.transition {
            self.switcher.set_style(new.transition.direction, &new.transition.dip_color);
            self.stinger.configure(&new.transition.stinger);
        }
        let new_looks: Vec<crate::config::SourceLook> = new.sources.iter().map(|s| s.look.clone()).collect();
        let old_looks: Vec<crate::config::SourceLook> = old.sources.iter().map(|s| s.look.clone()).collect();
        if new_looks != old_looks || old.style != new.style {
            let aspects = new.sources.iter().map(|s| s.look.cropped_aspect(s.width as f32 / s.height as f32)).collect();
            self.switcher.set_aspects(aspects);
            let mut l = self.looks.lock_ok();
            l.generation += 1;
            l.looks = new_looks;
            l.style = new.style.clone();
        }
        if old.graphics != new.graphics {
            self.graphics.update(&new.graphics);
        }
        self.apply_captions(&new.captions);
        self.apply_cutouts(&new.sources);
        self.apply_ndi_out(&new.ndi_out);
        // Who auto-framed sources follow, and how.
        for (i, s) in new.sources.iter().enumerate() {
            if let (Some(h), Some(o)) = (self.sources.get(i), old.sources.get(i)) {
                if o.framing != s.framing && o.device == s.device {
                    if let Some(f) = h.input.framing() {
                        *f.cfg.lock_ok() = s.framing.clone();
                    }
                }
            }
        }
        // PTZ controls follow their settings.
        for (i, s) in new.sources.iter().enumerate() {
            if let (Some(h), Some(o)) = (self.sources.get(i), old.sources.get(i)) {
                if o.ptz != s.ptz && o.device == s.device {
                    h.input.update_ptz(s, &[]);
                }
            }
        }
        // New playlists go straight to the players.
        for (i, s) in new.sources.iter().enumerate() {
            if old.sources.get(i).is_some_and(|o| o.media != s.media) {
                if let Some(p) = self.player(i) {
                    p.send(player::PlayerCmd::Playlist(s.media.clone()));
                }
            }
        }
        *self.cfg.lock_ok() = new.clone();
        Ok(())
    }

    // ---- sources and inputs while running ------------------------------------------

    /// Sources and audio inputs added, removed, moved or changed (whatever
    /// `Config::lineup_changed` sees), while everything else carries on:
    /// the program, the recording and the stream included. Sources that
    /// stay keep running untouched; changed ones reopen on their own; new
    /// ones open in the background. Returns notes for the operator.
    pub fn apply_lineup(&mut self, new: &Config) -> Vec<String> {
        use crate::config::{audio_key, source_key};
        let new = new.with_framed_rates();
        let old = self.config();
        let recording = self.is_recording();
        let mut notes = Vec::new();

        // ---- video sources -----------------------------------------------------
        // Each source now: the one it was (by identity, if nothing that
        // needs reopening changed), or made afresh.
        let mut keep: Vec<Option<usize>> = new
            .sources
            .iter()
            .map(|sc| {
                let j = self.sources.iter().position(|h| h.uid == sc.uid)?;
                old.sources.get(j).filter(|o| source_key(o) == source_key(sc)).map(|_| j)
            })
            .collect();
        // A close-up or virtual camera follows its wide source when that's remade.
        loop {
            let mut again = false;
            for (i, sc) in new.sources.iter().enumerate() {
                let Some(parent) = sc.framed_from().or_else(|| sc.virtual_ptz_of()) else { continue };
                if keep[i].is_some() {
                    let p = new.sources.iter().position(|p| p.name.trim().eq_ignore_ascii_case(parent.trim()));
                    if p.is_none_or(|p| keep[p].is_none()) {
                        keep[i] = None;
                        again = true;
                    }
                }
            }
            if !again {
                break;
            }
        }
        let delay_of = |sc: &crate::config::SourceConfig| {
            let parent = sc.framed_from().and_then(|from| new.sources.iter().find(|p| p.name.trim().eq_ignore_ascii_case(from)));
            parent.unwrap_or(sc).delay_ms
        };
        let slots: Vec<Arc<SourceSlot>> = new.sources.iter().enumerate().map(|(i, sc)| match keep[i] {
            Some(j) => self.sources[j].slot.clone(),
            None => SourceSlot::new(i, delay_of(sc), VIDEO_LATENCY),
        }).collect();
        let peers: Arc<Vec<(String, Arc<SourceSlot>)>> = Arc::new(new.sources.iter().zip(&slots).map(|(sc, s)| (sc.name.clone(), s.clone())).collect());
        let mut old_handles: Vec<Option<SourceHandle>> = std::mem::take(&mut self.sources).into_iter().map(Some).collect();
        let mut old_masks: Vec<Option<Arc<cutout::MaskSlot>>> = std::mem::take(&mut self.masks).into_iter().map(Some).collect();
        let mut old_cutouts: Vec<Option<CutoutState>> = std::mem::take(&mut *self.cutouts.lock_ok()).into_iter().map(Some).collect();
        let mut sources = Vec::new();
        let mut masks = Vec::new();
        let mut cutouts = Vec::new();
        let mut to_open = Vec::new();
        for (i, sc) in new.sources.iter().enumerate() {
            match keep[i].and_then(|j| old_handles[j].take().map(|h| (j, h))) {
                Some((j, mut h)) => {
                    h.record = sc.record;
                    sources.push(h);
                    masks.push(old_masks[j].take().unwrap_or_else(cutout::MaskSlot::new));
                    cutouts.push(old_cutouts[j].take().unwrap_or_default());
                }
                None => {
                    keep[i] = None;
                    let input = inputs::VideoInput::new(i, sc.clone(), slots[i].clone(), self.hub.clone(), peers.clone());
                    to_open.push(input.clone());
                    sources.push(SourceHandle { uid: sc.uid, name: sc.name.clone(), preview: FrameSlot::new(), size: (sc.width, sc.height), fps: sc.fps, record: sc.record, slot: slots[i].clone(), input });
                    masks.push(cutout::MaskSlot::new());
                    cutouts.push(CutoutState::Off);
                }
            }
        }
        let gone: Vec<SourceHandle> = old_handles.into_iter().flatten().collect();
        let old_cutouts: Vec<CutoutState> = old_cutouts.into_iter().flatten().collect();
        for h in &gone {
            let still = new.sources.iter().any(|s| s.uid == h.uid);
            if recording {
                // Its file ends now rather than holding its last picture to
                // the end of the take.
                h.slot.stop_iso_tap();
                if let Some(take) = self.take.lock_ok().as_mut() {
                    let length = clock::now() - take.t0;
                    for f in take.export_files.iter_mut().filter(|f| f.source == Some(h.uid as usize)) {
                        f.length = Some(length);
                    }
                }
            }
            if recording && h.record {
                notes.push(if still {
                    format!("{}: changed, so its own file in this take ends here (it's recorded again from the next take)", h.name)
                } else {
                    format!("{}: removed; its own file in this take ends here", h.name)
                });
            }
            self.status.lock_ok().offline.remove(&h.name);
        }
        if recording {
            for (i, sc) in new.sources.iter().enumerate() {
                if keep[i].is_none() && sc.record && !old.sources.iter().any(|o| o.uid == sc.uid) {
                    notes.push(format!("{}: recorded to its own file from the next take (it's in the program now)", sc.name));
                }
            }
        }
        self.sources = sources;
        self.masks = masks;
        *self.cutouts.lock_ok() = cutouts;

        // The compositor and switcher follow between two frames.
        let generation = self.comp_lineups.fetch_add(1, Ordering::Relaxed) + 1;
        let aspects: Vec<f32> = new.sources.iter().map(|s| s.look.cropped_aspect(s.width as f32 / s.height as f32)).collect();
        *self.comp_lineup.lock_ok() = Some(CompLineup {
            generation,
            from: keep.clone(),
            sizes: new.sources.iter().map(|s| (s.width, s.height)).collect(),
            slots: self.sources.iter().map(|s| s.slot.clone()).collect(),
            thumbs: self.sources.iter().map(|s| s.preview.clone()).collect(),
            masks: self.masks.clone(),
            looks: new.sources.iter().map(|s| s.look.clone()).collect(),
            style: new.style.clone(),
            aspects,
            uids: new.sources.iter().map(|s| s.uid).collect(),
        });
        let until = Instant::now() + Duration::from_secs(2);
        while self.comp_done.load(Ordering::Acquire) < generation && Instant::now() < until && !self.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(2));
        }
        {
            // Looks by the new numbers (the compositor already has them).
            let mut l = self.looks.lock_ok();
            l.looks = new.sources.iter().map(|s| s.look.clone()).collect();
            l.style = new.style.clone();
            l.generation += 1;
        }
        {
            let mut t = self.tall.lock_ok();
            t.names = new.sources.iter().map(|s| s.name.clone()).collect();
            t.generation += 1;
        }

        // ---- audio inputs ------------------------------------------------------
        let mut akeep: Vec<Option<usize>> = new
            .audio
            .iter()
            .map(|ac| {
                let j = self.audio.iter().position(|h| h.uid == ac.uid)?;
                old.audio.get(j).filter(|o| audio_key(o) == audio_key(ac)).map(|_| j)
            })
            .collect();
        let mut old_audio: Vec<Option<AudioHandle>> = std::mem::take(&mut self.audio).into_iter().map(Some).collect();
        let mut audio = Vec::new();
        let mut mix_slots = Vec::new();
        let mut audio_to_open = Vec::new();
        for (i, ac) in new.audio.iter().enumerate() {
            match akeep[i].and_then(|j| old_audio[j].take().map(|h| (j, h))) {
                Some((j, mut h)) => {
                    h.record = ac.record;
                    h.split = ac.split_channels;
                    mix_slots.push(audio::MixSlot::Keep(j, h.shared.clone()));
                    audio.push(h);
                }
                None => {
                    akeep[i] = None;
                    let (tx, rx) = bounded(1000);
                    let shared = InputShared::new(ac);
                    let input = inputs::AudioInputDev::new(i, ac.clone(), tx, shared.clone(), self.hub.clone());
                    mix_slots.push(audio::MixSlot::New(MixInput {
                        rx,
                        left: ac.program_left,
                        right: ac.program_right,
                        delay: ac.delay_ms as i64 * 1_000_000,
                        shared: shared.clone(),
                        gaps_expected: input.gaps_expected(),
                    }));
                    audio_to_open.push(input.clone());
                    audio.push(AudioHandle { uid: ac.uid, name: ac.name.clone(), record: ac.record, split: ac.split_channels, shared, input });
                }
            }
        }
        let audio_gone: Vec<AudioHandle> = old_audio.into_iter().flatten().collect();
        for h in &audio_gone {
            let still = new.audio.iter().any(|a| a.uid == h.uid);
            if recording {
                h.shared.stop_raw_tap();
            }
            if recording && h.record {
                notes.push(if still {
                    format!("{}: changed, so its WAV in this take ends here (it's recorded again from the next take)", h.name)
                } else {
                    format!("{}: removed; its WAV in this take ends here", h.name)
                });
            }
            self.status.lock_ok().offline.remove(&h.name);
        }
        if recording {
            for (i, ac) in new.audio.iter().enumerate() {
                if akeep[i].is_none() && ac.record && !old.audio.iter().any(|o| o.uid == ac.uid) {
                    notes.push(format!("{}: in the mix now; its own WAV starts with the next take", ac.name));
                }
            }
        }
        self.audio = audio;
        self.mix.set_lineup(mix_slots, new.audio.iter().map(|a| a.name.clone()).collect());
        self.mix.set_talkback_input(talkback_index(&new));
        *self.auto_inputs.lock_ok() = self.audio.iter().map(|a| (a.name.clone(), a.shared.clone())).collect();
        guests::hub().set_engine_inputs(new.audio.iter().enumerate().map(|(i, a)| (a.device.clone(), i)).collect());
        // Outputs that leave out an input by name (one that's new, too).
        {
            let outs = self.outputs.lock_ok();
            let kinds: Vec<(u64, audio::MixKind)> = outs.iter().map(|o| (o.tap, self.output_kind_in(&o.cfg, &new.audio))).collect();
            drop(outs);
            for t in self.mix_taps.lock_ok().iter_mut() {
                if let Some((_, k)) = kinds.iter().find(|(id, _)| *id == t.0) {
                    t.1 = *k;
                }
            }
        }

        // ---- the rest follows ---------------------------------------------------
        {
            let mut l = self.lineup.lock_ok();
            l.generation += 1;
            l.video = self.sources.iter().map(|s| s.input.clone()).collect();
            l.audio = self.audio.iter().map(|a| a.input.clone()).collect();
        }
        // Open what's new, and close what went, without the window waiting.
        for input in to_open {
            let (status, waker) = (self.status.clone(), self.waker.clone());
            let _ = std::thread::Builder::new().name("opening".into()).spawn(move || {
                let devs = inputs::DeviceCache::default().video(&input.cfg.device).to_vec();
                match input.open(&devs) {
                    Ok(desc) => status.lock_ok().push(LogLevel::Info, format!("{}: {desc}", input.cfg.name)),
                    Err(e) => {
                        let mut st = status.lock_ok();
                        st.push(LogLevel::Error, format!("{}: {e:#}. It will connect automatically when available.", input.cfg.name));
                        st.offline.insert(input.cfg.name.clone());
                    }
                }
                waker();
            });
        }
        for input in audio_to_open {
            let (status, waker) = (self.status.clone(), self.waker.clone());
            let _ = std::thread::Builder::new().name("opening".into()).spawn(move || {
                let devs = inputs::DeviceCache::default().audio(&input.cfg.device).to_vec();
                match input.open(&devs) {
                    Ok((_, Some(note))) => status.lock_ok().push(LogLevel::Warning, format!("{}: {note}", input.cfg.name)),
                    Ok(_) => {}
                    Err(e) => {
                        let mut st = status.lock_ok();
                        st.push(LogLevel::Error, format!("{}: {e:#}. It will connect automatically when available.", input.cfg.name));
                        st.offline.insert(input.cfg.name.clone());
                    }
                }
                waker();
            });
        }
        if !gone.is_empty() || !audio_gone.is_empty() || !old_cutouts.is_empty() {
            let _ = std::thread::Builder::new().name("closing".into()).spawn(move || {
                drop(old_cutouts);
                for h in gone {
                    h.input.close();
                }
                for a in audio_gone {
                    a.input.close();
                }
            });
        }
        {
            // The engine's own copy in the new order, each kept one as it was
            // (so `update_config` then applies what changed live: looks,
            // framing, levels, processing...), the new ones as they are.
            let mut c = self.cfg.lock_ok();
            c.sources = new.sources.iter().enumerate().map(|(i, sc)| keep[i].and_then(|j| old.sources.get(j).cloned()).unwrap_or_else(|| sc.clone())).collect();
            c.audio = new.audio.iter().enumerate().map(|(i, ac)| akeep[i].and_then(|j| old.audio.get(j).cloned()).unwrap_or_else(|| ac.clone())).collect();
            c.scenes = new.scenes.clone();
        }
        // Background removal for new sources; NDI's per-source outputs, the
        // headphones and captions where they named an input.
        self.apply_cutouts(&new.sources);
        if new.ndi_out.sources {
            if let Some((_, outs)) = self.ndi_out.lock_ok().take() {
                self.stop_ndi_outputs(outs);
            }
            self.apply_ndi_out(&new.ndi_out);
        }
        let solo = new.mix.monitor_source.trim();
        if !solo.is_empty() && !solo.eq_ignore_ascii_case("stream") {
            if let Some((_, _, id, running)) = self.monitor.lock_ok().take() {
                self.mix_taps.lock_ok().retain(|(i, ..)| *i != id);
                drop(running);
            }
            self.apply_monitor(&new.mix);
        }
        let heard = new.captions.source.trim().to_lowercase();
        if new.captions.enabled && !heard.is_empty() && heard != "recording" && heard != "stream" {
            *self.captions_cfg.lock_ok() = None;
            self.apply_captions(&new.captions);
        }
        (self.waker)();
        notes
    }

    pub fn encoder_description(&self) -> String {
        let cfg = self.cfg.lock_ok();
        describe_encoder(cfg.output.codec.video_codec(), &cfg.output.encoder)
    }

    // ---- streaming ------------------------------------------------------------------

    pub fn is_streaming(&self) -> bool {
        self.stream.lock_ok().is_some()
    }

    /// Go live to every enabled destination.
    pub fn start_stream(&self) -> Result<()> {
        if self.is_streaming() {
            bail!("already live");
        }
        if self.status.lock_ok().stream.stopping {
            bail!("the previous stream is still ending");
        }
        let cfg = self.config();
        let id = self.tap_ids.fetch_add(1, Ordering::Relaxed);
        let session = StreamSession::start(
            &cfg.streaming,
            streamer::Context {
                video_taps: self.video_taps.clone(),
                mix_taps: self.mix_taps.clone(),
                tap_id: id,
                program_fps: self.info.fps,
                vertical: {
                    let v = &self.tall.lock_ok().cfg;
                    v.enabled.then(|| v.size())
                },
                status: self.status.clone(),
                waker: self.waker.clone(),
            },
        )?;
        self.mix.reset_stream_loudness();
        *self.stream_started.lock_ok() = Some(clock::now());
        let names: Vec<String> = cfg.streaming.destinations.iter().filter(|d| d.enabled).map(|d| d.name.clone()).collect();
        self.status.lock_ok().push(LogLevel::Info, format!("Going live to {} ({})", names.join(", "), session.encoder));
        *self.stream.lock_ok() = Some(session);
        if cfg.streaming.record_when_live && !self.is_recording() {
            if let Err(e) = self.start_recording() {
                self.status.lock_ok().push(LogLevel::Error, format!("Couldn't start recording: {e:#}"));
            }
        }
        (self.waker)();
        Ok(())
    }

    /// End the stream cleanly (in the background; takes a moment).
    pub fn stop_stream(&self) {
        let Some(session) = self.stream.lock_ok().take() else { return };
        self.status.lock_ok().stream.stopping = true;
        (self.waker)();
        let (status, waker) = (self.status.clone(), self.waker.clone());
        let handle = std::thread::spawn(move || {
            let since = status.lock_ok().stream.since;
            session.stop();
            let mut st = status.lock_ok();
            let length = since.map(|s| fmt_duration(s.elapsed())).unwrap_or_default();
            st.stream = StreamStatus::default();
            st.push(LogLevel::Info, format!("Stream ended ({length})"));
            drop(st);
            waker();
        });
        self.finalizers.lock_ok().push(handle);
    }

    // ---- live controls -------------------------------------------------------------

    /// Switch by hand (pauses automatic switching for a moment).
    pub fn take_scene(&self, scene: &crate::scene::Scene, mode: TransitionMode, duration_ms: u32) {
        let mut s = scene.clone();
        s.retain_sources(self.sources.len());
        self.auto.manual_switch();
        self.restart_players_going_on_air(&s);
        let inputs: Vec<(String, Arc<InputShared>)> = self.audio.iter().map(|a| (a.name.clone(), a.shared.clone())).collect();
        let mut mode = mode;
        if mode == TransitionMode::Stinger {
            match self.stinger.trigger() {
                Some(st) => {
                    if let Some(sound) = st.sound {
                        self.pads.play_extra(sound, st.volume_db);
                    }
                    // Scene sound changes at the cut, as the picture does.
                    apply_scene_audio(&inputs, &s, (clock::now() + st.cut, 150_000_000));
                    self.switcher.take_stinger(s, st.duration, st.cut, VIDEO_LATENCY);
                    (self.waker)();
                    return;
                }
                None => {
                    let info = self.stinger.info();
                    let why = info.problem.unwrap_or_else(|| if info.loading { "the stinger is still loading".into() } else { "no stinger video is set (Settings → Switching)".into() });
                    self.status.lock_ok().push(LogLevel::Warning, format!("Mixed instead of the stinger: {why}"));
                    mode = TransitionMode::Mix;
                }
            }
        }
        apply_scene_audio(&inputs, &s, scene_audio_timing(mode, duration_ms));
        self.switcher.take(s, mode, duration_ms);
        (self.waker)();
    }

    /// Fade the program to black (Some(true)), back up (Some(false)), or
    /// the other way (None). Returns whether it's going to black.
    pub fn fade_to_black(&self, on: Option<bool>) -> bool {
        let on = on.unwrap_or(!self.switcher.is_black());
        let cfg = self.config();
        let ms = cfg.transition.ftb_ms.min(10_000);
        self.switcher.fade_to_black(on, ms);
        // The sound fades with the picture (same timing as scene audio).
        let at = clock::now() - VIDEO_LATENCY;
        if cfg.transition.ftb_audio || !on {
            self.mix.fade_program(if on { 0.0 } else { 1.0 }, at, ms.max(20) as i64 * 1_000_000);
        }
        (self.waker)();
        on
    }

    pub fn is_black(&self) -> bool {
        self.switcher.is_black()
    }

    /// Play the last `seconds` (None: as set) of the program again, on air.
    /// Returns how long it'll be on.
    pub fn start_replay(&self, seconds: Option<f32>, slow: bool, angle: &crate::control::ReplayAngle) -> Result<Duration> {
        use crate::control::ReplayAngle;
        let cfg = self.config();
        let secs = seconds.unwrap_or(cfg.replay.seconds).clamp(1.0, 60.0);
        // The newest pictures are still on their way to the recording.
        let lanes = match angle {
            ReplayAngle::Program => vec![self.clips.recent(secs)?],
            ReplayAngle::Source(t) => {
                let angles = self.angles.lock_ok();
                let names: Vec<String> = angles.iter().map(|a| a.0.clone()).collect();
                // (By number: the source's place in the show, not among those recorded.)
                let i = match t {
                    crate::control::Target::Index(i) => self.sources.get(*i).and_then(|s| names.iter().position(|n| *n == s.name)),
                    t => t.find(&names),
                };
                let Some(i) = i else {
                    self.clips.recent(secs)?; // (says "start recording first" when it isn't)
                    bail!("{t} isn't being recorded on its own, so it can't be replayed (its \"Record to its own file\" is off, or the files are ProRes)");
                };
                vec![angles[i].1.recent(secs)?]
            }
            ReplayAngle::Quad => {
                self.clips.recent(secs)?;
                let angles = self.angles.lock_ok();
                let lanes: Vec<_> = angles.iter().take(4).filter_map(|a| a.1.recent(secs).ok()).collect();
                if lanes.len() < 2 {
                    bail!("a replay from every angle needs at least two sources recording their own files");
                }
                lanes
            }
        };
        let speed = if slow { cfg.replay.slow_speed.clamp(0.1, 1.0) } else { 1.0 };
        let d = self.replay.start_lanes(lanes, speed)?;
        (self.waker)();
        Ok(d)
    }

    /// Pan, tilt and zoom controls of a source (a PTZ or virtual camera).
    pub fn ptz(&self, source: usize) -> Option<Arc<ptz::PtzCamera>> {
        self.sources.get(source)?.input.ptz()
    }

    /// Media players set to "restart when on air" start from the top when
    /// a scene brings them on.
    fn restart_players_going_on_air(&self, scene: &crate::scene::Scene) {
        let live = self.switcher.live().sources();
        let cfg = self.config();
        for i in scene.sources() {
            if !live.contains(&i) && cfg.sources.get(i).is_some_and(|s| s.media.restart_on_air) {
                if let Some(p) = self.player(i) {
                    p.send(player::PlayerCmd::Restart);
                }
            }
        }
    }

    /// Controls of a media player source.
    pub fn player(&self, source: usize) -> Option<Arc<player::PlayerControl>> {
        self.sources.get(source)?.input.player.lock_ok().clone()
    }

    pub fn set_auto_switch(&self, on: bool) {
        self.auto.enabled.store(on, Ordering::Relaxed);
        if on && self.config().auto_switch.rules.is_empty() {
            self.status.lock_ok().push(LogLevel::Warning, "Auto switching has no rules yet: add them in Settings → Switching.");
        }
        (self.waker)();
    }

    pub fn auto_switch_enabled(&self) -> bool {
        self.auto.enabled.load(Ordering::Relaxed)
    }

    /// Whether an input is on air (the scene or the operator may change it).
    pub fn is_on(&self, input: usize) -> bool {
        self.audio.get(input).is_some_and(|a| a.shared.is_on())
    }

    /// Host time the current stream started.
    pub fn stream_started_at(&self) -> Option<Ns> {
        if self.is_streaming() {
            *self.stream_started.lock_ok()
        } else {
            None
        }
    }

    pub fn set_gain(&self, input: usize, db: f64) {
        if let Some(a) = self.audio.get(input) {
            a.shared.set_gain_db(db);
        }
    }

    pub fn set_in_program(&self, input: usize, on: bool) {
        if let Some(a) = self.audio.get(input) {
            a.shared.set_in_program(on);
        }
    }

    // ---- recording (see take.rs) ---------------------------------------------------

    pub fn wait_finalized(&self) {
        let jobs: Vec<JoinHandle<()>> = std::mem::take(&mut *self.finalizers.lock_ok());
        for h in jobs {
            let _ = h.join();
        }
    }

    pub(crate) fn describe_setup(&self) -> String {
        let mut s = String::new();
        s += &format!("Program: {}x{} @ {} fps, composited on {}\n", self.info.canvas.0, self.info.canvas.1, self.info.fps, self.info.compositor);
        s += &format!("Video encoding: {}\n", self.encoder_description());
        if !self.audio.is_empty() {
            s += &format!("Audio encoding: AAC {} kbps, 48 kHz stereo\n", self.config().output.audio_bitrate_kbps);
        }
        s += "\nSources:\n";
        for (i, src) in self.sources.iter().enumerate() {
            s += &format!(
                "  {}. {} - {} ({}x{} @ {}){}\n",
                i + 1,
                src.name,
                src.description(),
                src.size.0,
                src.size.1,
                src.fps,
                if src.record { "" } else { " [not recorded]" }
            );
        }
        if !self.audio.is_empty() {
            s += "\nAudio inputs:\n";
            for (i, a) in self.audio.iter().enumerate() {
                s += &format!("  {}. {} - {}\n", i + 1, a.name, a.description());
            }
        }
        s
    }

    /// Stop everything. Finishes an active recording and stream first.
    pub fn shutdown(&mut self) {
        if self.stop.load(Ordering::Relaxed) {
            return;
        }
        self.stop_stream();
        if self.is_recording() {
            self.stop_recording();
        }
        self.wait_finalized();
        guests::hub().detach_engine();
        if let Some((_, outs)) = self.ndi_out.lock_ok().take() {
            self.stop_ndi_outputs(outs);
        }
        self.monitor.lock_ok().take();
        self.outputs.lock_ok().clear();
        if let Some((_, r)) = self.vcam.lock_ok().take() {
            drop(r);
        }
        self.stop.store(true, Ordering::Relaxed);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
        for s in &self.sources {
            s.input.close();
        }
        for a in &self.audio {
            a.input.close();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// When a switch's sound change lands, and how long it fades.
fn scene_audio_timing(mode: TransitionMode, duration_ms: u32) -> (Ns, Ns) {
    let fade = if mode != TransitionMode::Cut { duration_ms.max(20) as i64 * 1_000_000 } else { 20_000_000 };
    (clock::now() - VIDEO_LATENCY, fade)
}

/// Scenes can turn audio inputs on or off; the change is timed to land
/// with the picture change and fades along with a transition.
fn apply_scene_audio(inputs: &[(String, Arc<InputShared>)], scene: &crate::scene::Scene, (at, fade): (Ns, Ns)) {
    if scene.audio_on.is_empty() && scene.audio_off.is_empty() {
        return;
    }
    for (names, on) in [(&scene.audio_on, true), (&scene.audio_off, false)] {
        for n in names {
            if let Some((_, a)) = inputs.iter().find(|(name, _)| name.eq_ignore_ascii_case(n.trim())) {
                if a.is_on() != on {
                    a.set_on_at(on, at, fade);
                }
            }
        }
    }
}

fn describe_encoder(codec: crate::media::VideoCodec, wanted: &str) -> String {
    match encode::resolve_backend(codec, wanted) {
        Ok(b) => format!("{} · {}", encode::codec_name(codec), encode::describe_backend(b)),
        Err(e) => format!("{e}"),
    }
}

// ---------------------------------------------------------------------------
// The compositor thread

struct CompositorLoop {
    /// A new line-up of sources to take up, and the last one taken up.
    lineup: Arc<Mutex<Option<CompLineup>>>,
    lineup_done: Arc<AtomicU64>,
    comp: Compositor,
    slots: Vec<Arc<SourceSlot>>,
    thumbs: Vec<Arc<FrameSlot>>,
    preview: Arc<FrameSlot>,
    preview_bus: Arc<FrameSlot>,
    tall: Arc<Mutex<TallState>>,
    tall_generation: u64,
    /// While the vertical version is on: its settings and framing.
    tall_on: Option<(crate::config::VerticalConfig, vertical::Framing)>,
    tall_preview: Arc<FrameSlot>,
    graphics: Arc<graphics::Graphics>,
    looks: Arc<Mutex<LookState>>,
    looks_generation: u64,
    preview_width: Arc<AtomicU32>,
    switcher: Arc<Switcher>,
    taps: Arc<Mutex<VideoTaps>>,
    status: Arc<Mutex<Status>>,
    stop: Arc<AtomicBool>,
    waker: Waker,
    fps: u32,
    skips: Arc<AtomicU64>,
    captions: CaptionsSlot,
    masks: Vec<Arc<cutout::MaskSlot>>,
    stinger: Arc<stinger::StingerPlayer>,
    replay: Arc<replay::ReplayPlayer>,
    replay_label: Arc<AtomicBool>,
}

impl CompositorLoop {
    fn run(mut self) {
        let fps = self.fps;
        let frame_ns = NS_PER_SEC / fps as i64;
        let preview_every = fps.div_ceil(30).max(1) as i64;
        let thumb_every = fps.div_ceil(15).max(1) as i64;
        let mut k = grid_index(clock::now() - VIDEO_LATENCY, fps);
        let mut stream_grid: Option<(Ns, i64)> = None; // (from, next stream frame index)
        let mut tall_grid: Option<(Ns, i64)> = None;
        let mut warned_tall = false;
        let mut warned_program = false;
        let mut warned_stream = false;
        let mut last_behind: Option<Instant> = None;
        let mut serial = 0u64;
        while !self.stop.load(Ordering::Relaxed) {
            let t = grid_time(k, fps);
            clock::sleep_until(t + VIDEO_LATENCY);
            // Sources added, removed or moved: everything follows at once,
            // between frames.
            let lineup = self.lineup.lock_ok().take();
            if let Some(l) = lineup {
                self.comp.set_lineup(&l.from, &l.sizes);
                for (i, why) in self.comp.set_looks(&l.looks, &l.style) {
                    self.status.lock_ok().push(LogLevel::Warning, format!("Source {}: the LUT wasn't used: {why}", i + 1));
                }
                self.switcher.remap_sources(&l.from, l.aspects, l.uids);
                self.slots = l.slots;
                self.thumbs = l.thumbs;
                self.masks = l.masks;
                self.lineup_done.store(l.generation, Ordering::Release);
            }
            {
                let l = self.looks.lock_ok();
                if l.generation != self.looks_generation {
                    self.looks_generation = l.generation;
                    for (i, why) in self.comp.set_looks(&l.looks, &l.style) {
                        self.status.lock_ok().push(LogLevel::Warning, format!("Source {}: the LUT wasn't used: {why}", i + 1));
                    }
                }
            }
            let pw = self.preview_width.load(Ordering::Relaxed);
            if pw != self.comp.preview_width() {
                self.comp.set_preview_width(pw);
            }
            {
                let t = self.tall.lock_ok();
                if t.generation != self.tall_generation {
                    self.tall_generation = t.generation;
                    self.tall_on = t.cfg.enabled.then(|| (t.cfg.clone(), vertical::framing(&t.cfg, &t.names)));
                    self.comp.set_tall(t.cfg.enabled.then(|| t.cfg.size()), t.cfg.graphics);
                    if !t.cfg.enabled {
                        self.tall_preview.clear();
                    }
                }
            }
            // Who wants this frame?
            let (program_tx, stream, listeners, tall_tx, tall_stream) = {
                let taps = self.taps.lock_ok();
                (
                    taps.program.as_ref().filter(|p| t >= p.from).map(|p| p.tx.clone()),
                    taps.stream.as_ref().map(|s| (s.tx.clone(), s.from, s.size, s.fps.min(fps).max(1))),
                    taps.listeners.iter().map(|l| l.1.clone()).collect::<Vec<_>>(),
                    taps.tall.as_ref().filter(|p| t >= p.from).map(|p| p.tx.clone()),
                    taps.tall_stream.as_ref().map(|s| (s.tx.clone(), s.from, s.fps.min(fps).max(1))),
                )
            };
            // The vertical stream, on its own frame grid.
            let mut tall_stream_now = None;
            match tall_stream.filter(|_| self.tall_on.is_some()) {
                Some((tx, from, sfps)) => {
                    if tall_grid.is_none_or(|g| g.0 != from) {
                        tall_grid = Some((from, 0));
                    }
                    let (from, next) = tall_grid.unwrap();
                    if t >= from + grid_time(next, sfps) {
                        tall_stream_now = Some(tx);
                        tall_grid = Some((from, grid_index(t - from + 1, sfps)));
                    }
                }
                None => tall_grid = None,
            }
            let tall_tx = tall_tx.filter(|_| self.tall_on.is_some());
            let mut stream_now = None;
            match stream {
                Some((tx, from, size, sfps)) => {
                    if stream_grid.is_none_or(|g| g.0 != from) {
                        stream_grid = Some((from, 0));
                    }
                    let (from, next) = stream_grid.unwrap();
                    if t >= from + grid_time(next, sfps) {
                        stream_now = Some((tx, size));
                        stream_grid = Some((from, grid_index(t - from + 1, sfps)));
                    }
                }
                None => stream_grid = None,
            }
            let canvas = self.comp.canvas_size();
            let stream_is_program = stream_now.as_ref().is_some_and(|(_, s)| *s == canvas);
            let thumbs: Vec<usize> = if k % thumb_every == 0 {
                (0..self.slots.len()).filter(|&i| !self.slots[i].stale(1.0)).collect()
            } else {
                Vec::new()
            };
            let preview_now = k % preview_every == 0;
            let bus = if preview_now { self.switcher.preview_states() } else { None };
            let wants = Wants {
                program: program_tx.is_some() || stream_is_program || !listeners.is_empty(),
                stream: stream_now.as_ref().filter(|_| !stream_is_program).map(|(_, s)| *s),
                preview: preview_now,
                preview_bus: bus.is_some(),
                thumbs,
                tall: tall_tx.is_some() || tall_stream_now.is_some(),
                tall_preview: preview_now && self.tall_on.is_some(),
            };
            if wants.program || wants.stream.is_some() || wants.preview || !wants.thumbs.is_empty() || wants.tall {
                self.comp.set_black(self.switcher.black_level());
                let layout = self.switcher.layout();
                // The vertical version follows the same scenes.
                let mut tall_sources = Vec::new();
                if let Some((cfg, framing)) = &self.tall_on {
                    let (w, h) = cfg.size();
                    let aspect = w as f32 / h as f32;
                    let names = self.tall.lock_ok().names.clone();
                    let frames_of = |scene: &crate::scene::Scene| {
                        let l = cfg.layout_of(&scene.name);
                        let one = names.iter().position(|n| n.eq_ignore_ascii_case(&l.source));
                        vertical::frames(scene, l.mode, one, aspect, framing)
                    };
                    let (from, to, k) = self.switcher.scenes_now();
                    let to_frames = frames_of(&to);
                    let frames = match from {
                        Some(f) => vertical::compose(Some(&frames_of(&f)), &to_frames, k),
                        None => to_frames,
                    };
                    if wants.tall || wants.tall_preview {
                        tall_sources = frames.iter().filter(|f| f.alpha > 0.001).map(|f| f.source).collect();
                    }
                    self.comp.set_tall_frames(frames);
                }
                // New pictures, for the sources this frame draws: on air (both
                // scenes mid-transition), the preview bus, the multiview tiles
                // and the vertical version. The rest keep their last picture on
                // the GPU, rather than every camera being copied every frame.
                let drawn = sources_drawn(self.slots.len(), &layout, bus.as_deref(), &wants.thumbs, &tall_sources);
                for (i, slot) in self.slots.iter().enumerate() {
                    if !drawn[i] {
                        continue;
                    }
                    if let Some(f) = slot.pick(t) {
                        self.comp.upload(i, &f);
                        // Background removal: the mask made from this picture.
                        match self.masks.get(i).filter(|m| m.on.load(Ordering::Relaxed)).and_then(|m| m.pick(f.pts)) {
                            Some((pts, mask)) => self.comp.upload_mask(i, pts, &mask),
                            None => self.comp.clear_mask(i),
                        }
                    }
                }
                if let switcher::Layout::Stinger { elapsed, .. } = &layout {
                    let f = self.stinger.frame_at(*elapsed);
                    self.comp.set_stinger(f.as_ref(), self.stinger.matte());
                }
                match self.replay.frame() {
                    Some(r) => self.comp.set_replay(Some(&r.frame), r.alpha, self.replay_label.load(Ordering::Relaxed)),
                    None => self.comp.set_replay(None, 0.0, false),
                }
                let mut graphics = self.graphics.layers(t);
                if let Some((_, c)) = &*self.captions.lock_ok() {
                    graphics.extend(c.layer());
                }
                serial += 1;
                match self.comp.render(&layout, bus.as_deref(), &graphics, t, serial, &wants) {
                    Ok(r) => {
                        let mut stream_frame = r.stream;
                        if let Some(p) = r.program {
                            if stream_is_program {
                                stream_frame = Some(p.clone());
                            }
                            for l in &listeners {
                                let _ = l.try_send(p.clone());
                            }
                            if let Some(tx) = program_tx {
                                if tx.try_send(p).is_err() && !warned_program {
                                    warned_program = true;
                                    self.status.lock_ok().push(
                                        LogLevel::Warning,
                                        "Program: the encoder can't keep up, frames are being repeated. Lower the resolution or bitrate.",
                                    );
                                }
                            }
                        }
                        if let (Some(f), Some((tx, _))) = (stream_frame, stream_now) {
                            if tx.try_send(f).is_err() && !warned_stream {
                                warned_stream = true;
                                self.status.lock_ok().push(LogLevel::Warning, "Stream: the encoder can't keep up, frames are being dropped.");
                            }
                        }
                        let fresh = r.preview.is_some() || !r.thumbs.is_empty();
                        if let Some((w, h, rgba)) = r.preview {
                            self.preview.put(Frame { width: w as usize, height: h as usize, rgba });
                        }
                        if let Some((w, h, rgba)) = r.preview_bus {
                            self.preview_bus.put(Frame { width: w as usize, height: h as usize, rgba });
                        }
                        for (i, w, h, rgba) in r.thumbs {
                            self.thumbs[i].put(Frame { width: w as usize, height: h as usize, rgba });
                        }
                        if let Some((w, h, rgba)) = r.tall_preview {
                            self.tall_preview.put(Frame { width: w as usize, height: h as usize, rgba });
                        }
                        if let Some(f) = r.tall {
                            if let Some(tx) = &tall_stream_now {
                                let _ = tx.try_send(f.clone());
                            }
                            if let Some(tx) = &tall_tx {
                                if tx.try_send(f).is_err() && !warned_tall {
                                    warned_tall = true;
                                    self.status.lock_ok().push(LogLevel::Warning, "Vertical version: the encoder can't keep up, frames are being repeated.");
                                }
                            }
                        }
                        if fresh {
                            (self.waker)();
                        }
                    }
                    Err(e) => {
                        let mut st = self.status.lock_ok();
                        st.push(LogLevel::Error, format!("Compositor: {e:#}"));
                        st.fatal = Some(format!("the GPU compositor stopped: {e:#}"));
                        drop(st);
                        (self.waker)();
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
            k += 1;
            let now = clock::now();
            if now > grid_time(k, fps) + VIDEO_LATENCY + 4 * frame_ns {
                let jump = grid_index(now - VIDEO_LATENCY, fps);
                self.skips.fetch_add((jump - k).max(0) as u64, Ordering::Relaxed);
                k = jump;
                if last_behind.is_none_or(|l| l.elapsed() > Duration::from_secs(30)) {
                    last_behind = Some(Instant::now());
                    self.status.lock_ok().push(LogLevel::Warning, "The compositor fell behind and skipped frames (is the computer overloaded?)");
                }
            }
        }
    }
}

/// Which sources a frame draws (by index): any shown in the layout (both
/// scenes of a transition), the preview bus, the multiview tiles being
/// redrawn, and the vertical version's frames.
fn sources_drawn(n: usize, layout: &switcher::Layout, bus: Option<&[crate::scene::PadState]>, thumbs: &[usize], tall: &[usize]) -> Vec<bool> {
    let mut drawn = vec![false; n];
    let mut mark = |states: &[crate::scene::PadState]| {
        for (i, s) in states.iter().enumerate().take(n) {
            if s.alpha > 0.001 {
                drawn[i] = true;
            }
        }
    };
    match layout {
        switcher::Layout::One(states) | switcher::Layout::Stinger { states, .. } => mark(states),
        switcher::Layout::Two { from, to, .. } => {
            mark(from);
            mark(to);
        }
    }
    if let Some(b) = bus {
        mark(b);
    }
    for &i in thumbs.iter().chain(tall) {
        if let Some(d) = drawn.get_mut(i) {
            *d = true;
        }
    }
    drawn
}

pub fn fmt_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sources and inputs added, removed and moved while recording: what's
    /// on air stays on air, the recording carries on, and the files say
    /// what happened.
    #[test]
    fn sources_change_while_recording() {
        use crate::config::{AudioConfig, SourceConfig};
        // (The engine attaches to the guest hub, which the guest tests use.)
        let _turn = guests::test_turn();
        let _gpu = compositor::test_gpu_turn();
        let dir = std::env::temp_dir().join(format!("cutaway-lineup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cfg = Config::default();
        cfg.program.width = 320;
        cfg.program.height = 180;
        cfg.program.fps = 15;
        cfg.output.folder = dir.clone();
        let src = |name: &str, device: &str| SourceConfig { name: name.into(), device: device.into(), width: 320, height: 180, fps: 15, ..Default::default() };
        cfg.sources = vec![src("Dark", "test:ball"), src("Bars", "test:smpte"), src("Snow", "test:snow")];
        cfg.audio = vec![AudioConfig { name: "Tone".into(), device: "test".into(), ..Default::default() }];
        cfg.assign_uids();
        let mut e = match Engine::start(&cfg, Arc::new(|| {})) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("skipping: the engine can't start here ({err:#})");
                return;
            }
        };
        // The bars on air, watched as the program comes out.
        let (tx, rx) = bounded::<VideoFrame>(64);
        e.video_taps.lock_ok().listeners.push((999, tx));
        let bars = cfg.effective_scenes()[1].clone();
        e.take_scene(&bars, TransitionMode::Cut, 0);
        std::thread::sleep(Duration::from_millis(800));
        let luma = |rx: &crossbeam_channel::Receiver<VideoFrame>| {
            let f = rx.try_iter().last().expect("program frames");
            f.pixels.with_planes(|p| (0..p.y.len()).step_by(97).map(|i| p.y[i] as f64).sum::<f64>() / (p.y.len() / 97) as f64)
        };
        let on_air = luma(&rx);
        assert!(on_air > 40.0, "the bars (not the dark one): {on_air}");
        if let Err(err) = e.start_recording() {
            eprintln!("skipping the recording part: {err:#}");
            return;
        }
        std::thread::sleep(Duration::from_millis(1000));

        // Dark goes, Bars and Snow move up, Flat (and a second tone) join.
        let mut new = cfg.clone();
        new.sources.remove(0);
        new.sources.push(src("Flat", "test:gradient"));
        new.audio.push(AudioConfig { name: "Tone 2".into(), device: "test".into(), ..Default::default() });
        new.assign_uids();
        let notes = e.apply_lineup(&new);
        e.update_config(&new).unwrap();
        assert_eq!(e.sources.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Bars", "Snow", "Flat"]);
        assert_eq!(e.audio.len(), 2);
        assert!(notes.iter().any(|n| n.starts_with("Dark: removed")), "{notes:?}");
        assert!(notes.iter().any(|n| n.starts_with("Flat: recorded") && n.contains("next take")), "{notes:?}");
        assert!(notes.iter().any(|n| n.starts_with("Tone 2")), "{notes:?}");
        // Still the bars on air, now number 1.
        assert_eq!(e.switcher.live().layers.iter().map(|l| l.source).collect::<Vec<_>>(), vec![0]);
        let _ = rx.try_iter().count();
        std::thread::sleep(Duration::from_millis(600));
        let still = luma(&rx);
        assert!((still - on_air).abs() < 12.0, "the bars, still: {on_air} then {still}");
        // The new source and input come up.
        std::thread::sleep(Duration::from_millis(1200));
        assert!(e.sources[2].input.alive(), "Flat is running");
        assert!(e.status.lock_ok().meters.contains_key("a1"), "Tone 2 is in the mix");

        // The take: every file there was at the start, finished cleanly.
        e.stop_recording();
        e.wait_finalized();
        let report = e.status.lock_ok().last_take.clone().expect("a take");
        let names: Vec<String> = report.files.iter().map(|f| f.0.file_name().unwrap().to_string_lossy().to_string()).collect();
        for want in ["ISO_01_Dark", "ISO_02_Bars", "ISO_03_Snow", "Program", "Audio_01_Tone"] {
            assert!(names.iter().any(|n| n.starts_with(want)), "{want} in {names:?}");
        }
        assert!(!names.iter().any(|n| n.contains("Flat") || n.contains("Tone_2")), "new ones wait for the next take: {names:?}");
        assert!(report.problems.iter().all(|p| !p.contains("crashed")), "{:?}", report.problems);
        // Dark's file ends where it left; the others run to the end.
        let seconds = |want: &str| {
            let path = &report.files.iter().find(|f| f.0.file_name().unwrap().to_string_lossy().starts_with(want)).unwrap().0;
            // (Every track: the sound stops with the picture.)
            crate::media::demux::Mp4Reader::open(path).unwrap().duration() as f64 / 1e9
        };
        let (dark, bars) = (seconds("ISO_01_Dark"), seconds("ISO_02_Bars"));
        assert!(dark > 0.6 && dark < bars - 1.5, "Dark {dark:.2} s, Bars {bars:.2} s");
        e.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A replay from one camera shows that camera (not the program), and
    /// every angle at once shows them side by side.
    #[test]
    fn replays_from_any_camera() {
        use crate::config::SourceConfig;
        use crate::control::{ReplayAngle, Target};
        let _turn = guests::test_turn();
        let _gpu = compositor::test_gpu_turn();
        let dir = std::env::temp_dir().join(format!("cutaway-angles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut cfg = Config::default();
        (cfg.program.width, cfg.program.height, cfg.program.fps) = (320, 180, 15);
        cfg.output.folder = dir.clone();
        let src = |name: &str, device: &str| SourceConfig { name: name.into(), device: device.into(), width: 320, height: 180, fps: 15, ..Default::default() };
        cfg.sources = vec![src("Dark", "test:ball"), src("Bars", "test:smpte")];
        cfg.assign_uids();
        let mut e = match Engine::start(&cfg, Arc::new(|| {})) {
            Ok(e) => e,
            Err(err) => {
                eprintln!("skipping: the engine can't start here ({err:#})");
                return;
            }
        };
        let dark = cfg.effective_scenes()[0].clone();
        e.take_scene(&dark, TransitionMode::Cut, 0);
        assert!(e.start_replay(None, false, &ReplayAngle::Program).is_err(), "nothing recorded yet");
        if let Err(err) = e.start_recording() {
            eprintln!("skipping: {err:#}");
            return;
        }
        std::thread::sleep(Duration::from_millis(3500));
        let mean = |f: &VideoFrame| f.pixels.with_planes(|p| (0..p.y.len()).step_by(37).map(|i| p.y[i] as f64).sum::<f64>() / (p.y.len() / 37) as f64);
        let watch = |e: &Engine| {
            let mut seen = Vec::new();
            for _ in 0..60 {
                std::thread::sleep(Duration::from_millis(20));
                if let Some(f) = e.replay.frame() {
                    seen.push(f.frame);
                }
            }
            seen.last().cloned().expect("replay pictures")
        };
        // The program (the dark one) and the bars, from the bars' own file.
        e.start_replay(Some(2.0), false, &ReplayAngle::Program).unwrap();
        let program = mean(&watch(&e));
        e.replay.stop();
        std::thread::sleep(Duration::from_millis(500));
        e.start_replay(Some(2.0), false, &ReplayAngle::Source(Target::Name("bars".into()))).unwrap();
        let bars = watch(&e);
        assert!(mean(&bars) > program + 20.0, "the bars, not the program: {} vs {program}", mean(&bars));
        e.replay.stop();
        std::thread::sleep(Duration::from_millis(500));
        // By number too, and every angle at once.
        assert!(e.start_replay(Some(2.0), false, &ReplayAngle::Source(Target::Index(1))).is_ok());
        e.replay.stop();
        std::thread::sleep(Duration::from_millis(500));
        e.start_replay(Some(2.0), false, &ReplayAngle::Quad).unwrap();
        let quad = watch(&e);
        assert_eq!((quad.width, quad.height), (320, 180));
        let at = |x: usize, y: usize| quad.pixels.with_planes(|p| p.y[p.y_stride * y + x]);
        assert!(at(80, 135) <= 17, "two angles: the bottom row is empty");
        assert!(e.start_replay(None, false, &ReplayAngle::Source(Target::Name("Nobody".into()))).is_err());
        e.stop_recording();
        e.wait_finalized();
        e.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn draws_only_what_it_needs() {
        use crate::scene::PadState;
        let on = PadState { alpha: 1.0, ..PadState::HIDDEN };
        let off = PadState::HIDDEN;
        let which = |d: Vec<bool>| d.iter().enumerate().filter(|(_, &x)| x).map(|(i, _)| i).collect::<Vec<_>>();
        // Source 1 on air; nothing else this frame.
        let one = switcher::Layout::One(vec![off, on, off, off]);
        assert_eq!(which(sources_drawn(4, &one, None, &[], &[])), vec![1]);
        // Mid-transition from 0 to 2, 3 in preview, tiles for all, 1 in the vertical version.
        let two = switcher::Layout::Two { from: vec![on, off, off, off], to: vec![off, off, on, off], mode: crate::config::TransitionMode::Wipe, t: 0.5, direction: [1.0, 0.0], dip: [0.0; 3] };
        assert_eq!(which(sources_drawn(4, &two, Some(&[off, off, off, on]), &[], &[])), vec![0, 2, 3]);
        assert_eq!(which(sources_drawn(4, &two, None, &[0, 1, 2, 3], &[])), vec![0, 1, 2, 3]);
        assert_eq!(which(sources_drawn(4, &one, None, &[], &[3])), vec![1, 3]);
        // Indices past the end are ignored.
        assert_eq!(which(sources_drawn(2, &one, None, &[7], &[9])), vec![1]);
    }

    #[test]
    fn grid() {
        assert_eq!(grid_time(3, 30), 100_000_000);
        assert_eq!(grid_index(100_000_000, 30), 3);
        assert_eq!(grid_index(100_000_001, 30), 4);
        let k = grid_index(1_234_567_890_123, 60);
        assert!(grid_time(k, 60) >= 1_234_567_890_123 && grid_time(k - 1, 60) < 1_234_567_890_123);
    }
}
