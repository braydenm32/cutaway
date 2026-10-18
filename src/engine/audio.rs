//! Audio: aligning inputs to the host clock and mixing the program.
//!
//! Every input (an interface, a loopback device, a test tone) runs on its
//! own clock, which drifts against the host clock by a few parts per
//! million. Over an hour that is tens of milliseconds - enough to wreck lip
//! sync - so each input goes through an [`Aligner`]: a high-quality
//! resampler whose ratio is steered so that the audio stays locked to its
//! capture timestamps.

use super::dsp::{Leveller, Limiter, LoudnessMeter, VoiceChain};
use super::{Meter, Status};
use crate::config::VoiceProcessing;
use crate::media::{clock, AudioBlock, Ns, NS_PER_SEC};
use crossbeam_channel::{Receiver, Sender};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use rubato::{Adjustable, Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType, WindowFunction};
use std::collections::VecDeque;

pub const MIX_RATE: u32 = 48_000;
/// Mixer block: 10 ms.
pub const BLOCK: usize = 480;
pub const BLOCK_NS: Ns = 10_000_000;

/// Resamples one stereo input onto the host timeline.
pub struct Aligner {
    in_rate: u32,
    resampler: Async<f32>,
    /// Interleaved stereo frames not yet consumed.
    buf: VecDeque<f32>,
    /// Global index of the first frame in `buf`.
    head: u64,
    /// (global frame index, capture time) for each received block.
    anchors: VecDeque<(u64, Ns)>,
    aligned: bool,
    err_filtered: f64,
    integral: f64,
    pub last_error_ns: f64,
    pub ratio: f64,
    /// Times the input had to be re-lined-up after audio arrived late.
    pub realigns: u64,
    was_aligned: bool,
    fade_in: bool,
    scratch_in: Vec<f32>,
    scratch_out: Vec<f32>,
}

impl Aligner {
    pub fn new(in_rate: u32) -> Self {
        let params = SincInterpolationParameters::new(64, WindowFunction::BlackmanHarris2)
            .oversampling_factor(128)
            .interpolation(SincInterpolationType::Cubic);
        let ratio = MIX_RATE as f64 / in_rate as f64;
        let resampler = Async::<f32>::new_sinc(ratio, 1.1, &params, BLOCK, 2, FixedAsync::Output).expect("resampler");
        Aligner {
            in_rate,
            resampler,
            buf: VecDeque::new(),
            head: 0,
            anchors: VecDeque::new(),
            aligned: false,
            err_filtered: 0.0,
            integral: 0.0,
            last_error_ns: 0.0,
            ratio: 1.0,
            realigns: 0,
            was_aligned: false,
            fade_in: false,
            scratch_in: Vec::new(),
            scratch_out: vec![0.0; BLOCK * 2 * 2],
        }
    }

    fn end(&self) -> u64 {
        self.head + (self.buf.len() / 2) as u64
    }

    /// Add stereo frames captured at `pts`.
    pub fn push(&mut self, pts: Ns, stereo: &[f32]) {
        let idx = self.end();
        self.anchors.push_back((idx, pts));
        self.buf.extend(stereo.iter().copied());
        // Keep at most ~10 s.
        let max = (self.in_rate as usize) * 2 * 10;
        if self.buf.len() > max {
            let drop = (self.buf.len() - max) / 2;
            self.drop_frames(drop as u64);
            self.aligned = false;
        }
    }

    fn drop_frames(&mut self, n: u64) {
        let n = n.min((self.buf.len() / 2) as u64);
        self.buf.drain(..(n as usize * 2));
        self.head += n;
        while self.anchors.len() > 1 && self.anchors[1].0 <= self.head {
            self.anchors.pop_front();
        }
    }

    /// Capture time of global frame `idx`.
    fn time_at(&self, idx: u64) -> Option<Ns> {
        let a = self.anchors.iter().rev().find(|a| a.0 <= idx).or(self.anchors.front())?;
        let frames = idx as i64 - a.0 as i64;
        Some(a.1 + frames * NS_PER_SEC / self.in_rate as i64)
    }

    fn delay_ns(&self) -> Ns {
        self.resampler.output_delay() as i64 * NS_PER_SEC / MIX_RATE as i64
    }

    /// Fill `out` (BLOCK stereo frames) with the audio for the block that
    /// starts at `t`. Returns false (and silence) when there's no audio.
    pub fn read(&mut self, t: Ns, out: &mut [f32]) -> bool {
        out.fill(0.0);
        if self.buf.is_empty() {
            self.aligned = false;
            return false;
        }
        let delay = self.delay_ns();
        if !self.aligned {
            let Some(head_t) = self.time_at(self.head) else { return false };
            let target = t + delay;
            if head_t > target + BLOCK_NS {
                return false; // this input starts later
            }
            let offset = ((target - head_t) as i128 * self.in_rate as i128 / NS_PER_SEC as i128) as i64;
            if offset > 0 {
                self.drop_frames(offset as u64);
            } else if offset < 0 {
                let n = (-offset) as usize;
                for _ in 0..n * 2 {
                    self.buf.push_front(0.0);
                }
                self.head -= n as u64;
            }
            self.resampler.reset();
            let _ = self.resampler.set_resample_ratio_relative(1.0, false);
            self.ratio = 1.0;
            self.err_filtered = 0.0;
            self.integral = 0.0;
            self.aligned = true;
            if self.was_aligned {
                self.realigns += 1;
            }
            self.was_aligned = true;
            self.fade_in = true;
        }
        let needed = self.resampler.input_frames_next();
        if self.buf.len() / 2 < needed {
            // Not enough audio yet (late delivery): realign next time.
            self.aligned = false;
            return false;
        }
        self.scratch_in.clear();
        self.scratch_in.extend(self.buf.drain(..needed * 2));
        self.head += needed as u64;
        while self.anchors.len() > 1 && self.anchors[1].0 <= self.head {
            self.anchors.pop_front();
        }
        let input = InterleavedSlice::new(&self.scratch_in, 2, needed).expect("input");
        let cap = self.scratch_out.len() / 2;
        let mut output = InterleavedSlice::new_mut(&mut self.scratch_out, 2, cap).expect("output");
        let (_, produced) = match self.resampler.process_into_buffer(&input, &mut output, None) {
            Ok(r) => r,
            Err(_) => {
                self.aligned = false;
                return false;
            }
        };
        let n = produced.min(BLOCK);
        out[..n * 2].copy_from_slice(&self.scratch_out[..n * 2]);
        if self.fade_in {
            // Ramp up over 5 ms after starting or re-lining-up, so a gap
            // doesn't become a click.
            self.fade_in = false;
            let ramp = (MIX_RATE as usize / 200).min(n);
            for (i, f) in out.chunks_exact_mut(2).take(ramp).enumerate() {
                let g = i as f32 / ramp as f32;
                f[0] *= g;
                f[1] *= g;
            }
        }

        // Steer: the next unconsumed frame should be the one that lands at
        // the end of this block (plus the resampler's own delay).
        if let Some(head_t) = self.time_at(self.head) {
            let err = (head_t - delay - (t + BLOCK_NS)) as f64;
            self.last_error_ns = err;
            if err.abs() > 40e6 {
                self.aligned = false;
            } else {
                self.err_filtered += 0.05 * (err - self.err_filtered);
                let e = self.err_filtered / 1e9;
                self.integral = (self.integral + e * 0.01).clamp(-0.05, 0.05);
                let rel = (1.0 + 0.3 * e + 0.05 * self.integral).clamp(0.99, 1.01);
                self.ratio = rel;
                let _ = self.resampler.set_resample_ratio_relative(rel, true);
            }
        }
        true
    }
}

/// Peak and RMS (dB) per channel, accumulated over a few blocks.
#[derive(Default, Clone)]
pub struct MeterAcc {
    peak: Vec<f32>,
    sum_sq: Vec<f64>,
    n: usize,
}

impl MeterAcc {
    pub fn add(&mut self, samples: &[f32], channels: usize) {
        if self.peak.len() != channels {
            self.peak = vec![0.0; channels];
            self.sum_sq = vec![0.0; channels];
            self.n = 0;
        }
        for frame in samples.chunks(channels) {
            for (c, s) in frame.iter().enumerate() {
                self.peak[c] = self.peak[c].max(s.abs());
                self.sum_sq[c] += (*s as f64) * (*s as f64);
            }
        }
        self.n += samples.len() / channels.max(1);
    }

    /// (peak dB, rms dB) per channel; resets.
    pub fn take(&mut self) -> (Vec<f32>, Vec<f32>) {
        let db = |v: f64| if v <= 1e-9 { -100.0 } else { (20.0 * v.log10()) as f32 };
        let peak = self.peak.iter().map(|p| db(*p as f64)).collect();
        let rms = self.sum_sq.iter().map(|s| db((s / self.n.max(1) as f64).sqrt())).collect();
        self.peak.iter_mut().for_each(|p| *p = 0.0);
        self.sum_sq.iter_mut().for_each(|s| *s = 0.0);
        self.n = 0;
        (peak, rms)
    }
}

/// Pick two channels (1-based) of an interleaved block as stereo.
pub fn select_stereo(block: &AudioBlock, left: u32, right: u32) -> Vec<f32> {
    let ch = block.channels.max(1) as usize;
    let l = (left.max(1) as usize - 1).min(ch - 1);
    let r = (right.max(1) as usize - 1).min(ch - 1);
    let mut out = Vec::with_capacity(block.frames() * 2);
    for f in block.samples.chunks(ch) {
        out.push(f[l]);
        out.push(f[r]);
    }
    out
}

/// Where an input's raw audio goes while recording.
pub struct RawTap {
    pub tx: Sender<AudioBlock>,
    /// Send audio from this host time on (earlier audio is replayed from
    /// the mixer's history, so delayed inputs start on time).
    pub from: Ns,
    pub id: u64,
}

/// Live controls and taps for one input, shared with the engine.
pub struct InputShared {
    gain: AtomicU32,
    /// On air (unmuted), as shown in the UI.
    on: AtomicBool,
    /// Scheduled on/off changes: (mix time, on, fade length).
    changes: Mutex<VecDeque<(Ns, bool, Ns)>>,
    to_recording: AtomicBool,
    to_stream: AtomicBool,
    processing: Mutex<VoiceProcessing>,
    processing_version: AtomicU64,
    /// Raw blocks (all channels, native rate) for the WAV recorder.
    raw_tap: Mutex<Option<RawTap>>,
    tap_ids: AtomicU64,
    /// Host time the last audio arrived.
    last_seen: AtomicI64,
    resyncs: AtomicU64,
    /// Per-channel level (linear, fast attack, slow release) of the raw input.
    levels: Mutex<Vec<f32>>,
    /// Compressor gain reduction (dB) for the UI.
    reduction: AtomicU32,
}

impl InputShared {
    pub fn new(cfg: &crate::config::AudioConfig) -> Arc<Self> {
        let s = Arc::new(InputShared {
            gain: AtomicU32::new(0),
            on: AtomicBool::new(cfg.in_program),
            changes: Mutex::new(VecDeque::new()),
            to_recording: AtomicBool::new(cfg.to_recording),
            to_stream: AtomicBool::new(cfg.to_stream),
            processing: Mutex::new(cfg.processing.clone()),
            processing_version: AtomicU64::new(1),
            raw_tap: Mutex::new(None),
            tap_ids: Default::default(),
            last_seen: Default::default(),
            resyncs: Default::default(),
            levels: Mutex::new(Vec::new()),
            reduction: AtomicU32::new(0),
        });
        s.set_gain_db(cfg.gain_db);
        s
    }
    pub fn start_raw_tap(&self, tx: Sender<AudioBlock>, from: Ns) {
        let id = self.tap_ids.fetch_add(1, Ordering::Relaxed) + 1;
        *self.raw_tap.lock().unwrap() = Some(RawTap { tx, from, id });
    }
    pub fn last_seen(&self) -> Ns {
        self.last_seen.load(Ordering::Relaxed)
    }
    pub fn stop_raw_tap(&self) {
        *self.raw_tap.lock().unwrap() = None;
    }
    pub fn set_gain_db(&self, db: f64) {
        self.gain.store((10f64.powf(db / 20.0) as f32).to_bits(), Ordering::Relaxed);
    }
    /// Turn on or off now (with a short fade so it doesn't click).
    pub fn set_in_program(&self, on: bool) {
        self.set_on_at(on, 0, 20_000_000);
    }
    /// Turn on or off at mix time `at`, fading over `fade` (scene audio:
    /// timed to match the picture change).
    pub fn set_on_at(&self, on: bool, at: Ns, fade: Ns) {
        self.on.store(on, Ordering::Relaxed);
        self.changes.lock().unwrap().push_back((at, on, fade.max(5_000_000)));
    }
    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }
    pub fn set_routing(&self, to_recording: bool, to_stream: bool) {
        self.to_recording.store(to_recording, Ordering::Relaxed);
        self.to_stream.store(to_stream, Ordering::Relaxed);
    }
    pub fn set_processing(&self, p: &VoiceProcessing) {
        let mut cur = self.processing.lock().unwrap();
        if *cur != *p {
            *cur = p.clone();
            self.processing_version.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn resyncs(&self) -> u64 {
        self.resyncs.load(Ordering::Relaxed)
    }
    /// Level of each channel of the raw input, in dB.
    pub fn levels_db(&self) -> Vec<f32> {
        self.levels.lock().unwrap().iter().map(|l| if *l <= 1e-6 { -120.0 } else { 20.0 * l.log10() }).collect()
    }
    /// Compressor gain reduction in dB (0 when not compressing).
    pub fn reduction_db(&self) -> f32 {
        f32::from_bits(self.reduction.load(Ordering::Relaxed))
    }
    fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }
    fn update_levels(&self, b: &AudioBlock) {
        let ch = b.channels.max(1) as usize;
        let frames = b.frames().max(1);
        let mut lv = self.levels.lock().unwrap();
        if lv.len() != ch {
            *lv = vec![0.0; ch];
        }
        // Release of about 300 ms per block length.
        let decay = (-(frames as f32 / b.rate.max(1) as f32) / 0.3).exp();
        for c in 0..ch {
            let sum: f32 = b.samples.iter().skip(c).step_by(ch).map(|s| s * s).sum();
            let rms = (sum / frames as f32).sqrt();
            lv[c] = rms.max(lv[c] * decay);
        }
    }
}

pub struct MixInput {
    pub rx: Receiver<AudioBlock>,
    pub left: u32,
    pub right: u32,
    pub delay: Ns,
    pub shared: Arc<InputShared>,
    /// Gaps are normal (sound from a media player or network source).
    pub gaps_expected: bool,
}

/// Which mix a consumer wants.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MixKind {
    /// Heard in the recorded files.
    Recording,
    /// Sent to the live stream (levelled if that's on).
    Stream,
    /// One input on its own (after processing, before its on/off), for
    /// solo monitoring.
    Input(usize),
    /// One input for its own track in the files: after processing, and
    /// either before the fader or as heard in the mix.
    Track { input: usize, post_fader: bool },
}

/// Consumers of the mixes (recording, streaming, monitoring), by id.
pub type MixTaps = Arc<Mutex<Vec<(u64, MixKind, Sender<AudioBlock>)>>>;

/// Settings for the master section, shared with the engine.
pub struct MixShared {
    limiter: AtomicBool,
    ceiling_db: AtomicU32,
    levelling: AtomicBool,
    target_lufs: AtomicU32,
    reset_recording: AtomicBool,
    reset_stream: AtomicBool,
}

impl MixShared {
    pub fn new(cfg: &crate::config::MixConfig) -> Arc<Self> {
        let m = Arc::new(MixShared {
            limiter: AtomicBool::new(true),
            ceiling_db: AtomicU32::new(0),
            levelling: AtomicBool::new(false),
            target_lufs: AtomicU32::new(0),
            reset_recording: AtomicBool::new(false),
            reset_stream: AtomicBool::new(false),
        });
        m.apply(cfg);
        m
    }
    pub fn apply(&self, cfg: &crate::config::MixConfig) {
        self.limiter.store(cfg.limiter, Ordering::Relaxed);
        self.ceiling_db.store(cfg.limiter_ceiling_db.clamp(-12.0, 0.0).to_bits(), Ordering::Relaxed);
        self.levelling.store(cfg.stream_levelling, Ordering::Relaxed);
        self.target_lufs.store(cfg.stream_target_lufs.clamp(-30.0, -8.0).to_bits(), Ordering::Relaxed);
    }
    /// Start measuring the integrated loudness of the recording afresh.
    pub fn reset_recording_loudness(&self) {
        self.reset_recording.store(true, Ordering::Relaxed);
    }
    pub fn reset_stream_loudness(&self) {
        self.reset_stream.store(true, Ordering::Relaxed);
    }
}

/// Loudness figures for the UI (LUFS; below -100 = silence).
#[derive(Clone, Debug, Default)]
pub struct Loudness {
    pub momentary: f32,
    pub short_term: f32,
    pub integrated: f32,
    /// Limiter gain reduction (dB).
    pub limiting: f32,
}

#[derive(Clone, Debug, Default)]
pub struct MixLoudness {
    pub recording: Loudness,
    pub stream: Loudness,
    /// What the leveller is doing to the stream (dB), when on.
    pub stream_gain_db: Option<f32>,
}

/// How far behind real time the mix runs, so every input has delivered.
pub fn mix_latency(max_delay: Ns) -> Ns {
    60_000_000 + max_delay
}

/// Master section of one mix.
struct Master {
    limiter: Limiter,
    meter: LoudnessMeter,
    peaks: MeterAcc,
}

impl Master {
    fn new() -> Self {
        Master { limiter: Limiter::new(-1.0), meter: LoudnessMeter::new(), peaks: MeterAcc::default() }
    }
    fn finish(&mut self, buf: &mut [f32], shared: &MixShared) {
        if shared.limiter.load(Ordering::Relaxed) {
            self.limiter.set_ceiling(f32::from_bits(shared.ceiling_db.load(Ordering::Relaxed)) as f64);
            self.limiter.process(buf);
        } else {
            self.limiter.reduction_db = 0.0;
        }
        self.meter.push(buf);
        self.peaks.add(buf, 2);
    }
    fn loudness(&self) -> Loudness {
        Loudness {
            momentary: self.meter.momentary() as f32,
            short_term: self.meter.short_term() as f32,
            integrated: self.meter.integrated() as f32,
            limiting: self.limiter.reduction_db as f32,
        }
    }
}

/// Per-input state inside the mixer.
struct Strip {
    aligner: Option<Aligner>,
    chain: Option<VoiceChain>,
    chain_version: u64,
    /// On/off fader position (0..1) and where it's heading.
    level: f32,
    target: f32,
    step: f32,
    realigns: u64,
    warned_at: Option<std::time::Instant>,
    meter: MeterAcc,
    history: VecDeque<AudioBlock>,
    fed: Option<u64>,
}

/// The mixer loop: runs until `stop`.
#[allow(clippy::too_many_arguments)]
pub fn run_mixer(inputs: Vec<MixInput>, names: Vec<String>, taps: MixTaps, master: Arc<MixShared>, status: Arc<Mutex<Status>>, stop: Arc<AtomicBool>, latency: Ns) {
    let mut strips: Vec<Strip> = inputs
        .iter()
        .map(|inp| {
            let on = if inp.shared.is_on() { 1.0 } else { 0.0 };
            inp.shared.changes.lock().unwrap().clear();
            Strip {
                aligner: None,
                chain: None,
                chain_version: 0,
                level: on,
                target: on,
                step: 0.0,
                realigns: 0,
                warned_at: None,
                meter: MeterAcc::default(),
                history: VecDeque::new(),
                fed: None,
            }
        })
        .collect();
    let mut rec = Master::new();
    let mut stream = Master::new();
    let mut leveller = Leveller::default();
    let mut t = (clock::now() / BLOCK_NS) * BLOCK_NS;
    let mut buf = vec![0.0f32; BLOCK * 2];
    let mut n = 0u64;
    while !stop.load(Ordering::Relaxed) {
        clock::sleep_until(t + BLOCK_NS + latency);
        for (i, inp) in inputs.iter().enumerate() {
            let strip = &mut strips[i];
            let mut fresh = Vec::new();
            while let Ok(mut b) = inp.rx.try_recv() {
                b.pts += inp.delay;
                if strip.aligner.as_ref().is_none_or(|a| a.in_rate != b.rate) {
                    strip.aligner = Some(Aligner::new(b.rate));
                }
                strip.aligner.as_mut().unwrap().push(b.pts, &select_stereo(&b, inp.left, inp.right));
                inp.shared.update_levels(&b);
                fresh.push(b);
            }
            if !fresh.is_empty() {
                inp.shared.last_seen.store(clock::now(), Ordering::Relaxed);
            }
            let block_end = |b: &AudioBlock| b.pts + b.frames() as i64 * NS_PER_SEC / b.rate.max(1) as i64;
            {
                let tap = inp.shared.raw_tap.lock().unwrap();
                match tap.as_ref() {
                    Some(tap) if strip.fed != Some(tap.id) => {
                        // New tap: replay what's still in history from `from`.
                        for b in strip.history.iter().chain(fresh.iter()) {
                            if block_end(b) > tap.from {
                                let _ = tap.tx.try_send(b.clone());
                            }
                        }
                        strip.fed = Some(tap.id);
                    }
                    Some(tap) => {
                        for b in &fresh {
                            let _ = tap.tx.try_send(b.clone());
                        }
                    }
                    None => strip.fed = None,
                }
            }
            strip.history.extend(fresh);
            let keep = inp.delay + latency + NS_PER_SEC / 2;
            while strip.history.len() > 1 && strip.history.back().map(|b| b.pts).unwrap_or(0) - strip.history[1].pts > keep {
                strip.history.pop_front();
            }
            // Processing settings changed?
            let v = inp.shared.processing_version.load(Ordering::Relaxed);
            if v != strip.chain_version {
                strip.chain_version = v;
                strip.chain = VoiceChain::new(&inp.shared.processing.lock().unwrap());
            }
            // On/off changes due by now.
            let mut changes = inp.shared.changes.lock().unwrap();
            while let Some(&(at, on, fade)) = changes.front() {
                if at > t {
                    break;
                }
                changes.pop_front();
                strip.target = if on { 1.0 } else { 0.0 };
                let blocks = (fade / BLOCK_NS).max(1) as f32;
                strip.step = 1.0 / (blocks * BLOCK as f32);
            }
        }
        let wanted: Vec<MixKind> = taps.lock().unwrap().iter().map(|t| t.1).collect();
        let mut rec_mix = vec![0.0f32; BLOCK * 2];
        let mut stream_mix = vec![0.0f32; BLOCK * 2];
        let mut solos: Vec<(usize, Vec<f32>)> = Vec::new();
        let mut tracks: Vec<(usize, bool, Vec<f32>)> = Vec::new();
        for (i, inp) in inputs.iter().enumerate() {
            let strip = &mut strips[i];
            let Some(a) = strip.aligner.as_mut() else { continue };
            let ok = a.read(t, &mut buf);
            if a.realigns != strip.realigns {
                strip.realigns = a.realigns;
                inp.shared.resyncs.store(a.realigns, Ordering::Relaxed);
                if !inp.gaps_expected && strip.warned_at.is_none_or(|w| w.elapsed().as_secs() >= 30) {
                    strip.warned_at = Some(std::time::Instant::now());
                    status.lock().unwrap().push(
                        crate::engine::LogLevel::Warning,
                        format!("{}: audio arrived late and had to be re-synced (possible click). Is the computer overloaded?", names.get(i).map(String::as_str).unwrap_or("Audio")),
                    );
                }
            }
            if !ok {
                continue;
            }
            if let Some(c) = strip.chain.as_mut() {
                c.process(&mut buf);
                inp.shared.reduction.store(c.reduction_db().to_bits(), Ordering::Relaxed);
            }
            strip.meter.add(&buf, 2);
            let g = inp.shared.gain();
            if wanted.contains(&MixKind::Input(i)) {
                solos.push((i, buf.iter().map(|s| s * g).collect()));
            }
            if wanted.contains(&MixKind::Track { input: i, post_fader: false }) {
                tracks.push((i, false, buf.clone()));
            }
            let post = wanted.contains(&MixKind::Track { input: i, post_fader: true });
            let mut post_buf = if post { vec![0.0f32; buf.len()] } else { Vec::new() };
            let to_rec = inp.shared.to_recording.load(Ordering::Relaxed);
            let to_stream = inp.shared.to_stream.load(Ordering::Relaxed);
            if strip.level == 0.0 && strip.target == 0.0 {
                if post {
                    tracks.push((i, true, post_buf));
                }
                continue;
            }
            for (k, f) in buf.chunks_exact(2).enumerate() {
                if strip.level != strip.target {
                    strip.level = if strip.target > strip.level { (strip.level + strip.step).min(strip.target) } else { (strip.level - strip.step).max(strip.target) };
                }
                let gl = g * strip.level;
                let (l, r) = (f[0] * gl, f[1] * gl);
                if post {
                    post_buf[2 * k] = l;
                    post_buf[2 * k + 1] = r;
                }
                if to_rec {
                    rec_mix[2 * k] += l;
                    rec_mix[2 * k + 1] += r;
                }
                if to_stream {
                    stream_mix[2 * k] += l;
                    stream_mix[2 * k + 1] += r;
                }
            }
            if post {
                tracks.push((i, true, post_buf));
            }
        }
        // Integrated loudness restarts with each take / stream.
        if master.reset_recording.swap(false, Ordering::Relaxed) {
            rec.meter.reset_integrated();
        }
        if master.reset_stream.swap(false, Ordering::Relaxed) {
            stream.meter.reset_integrated();
        }
        rec.finish(&mut rec_mix, &master);
        let levelling = master.levelling.load(Ordering::Relaxed);
        if levelling {
            leveller.process(&mut stream_mix, f32::from_bits(master.target_lufs.load(Ordering::Relaxed)) as f64);
        }
        stream.finish(&mut stream_mix, &master);
        let rec_block = AudioBlock { pts: t, rate: MIX_RATE, channels: 2, samples: rec_mix };
        let stream_block = AudioBlock { pts: t, rate: MIX_RATE, channels: 2, samples: stream_mix };
        for (_, kind, tap) in taps.lock().unwrap().iter() {
            let block = match kind {
                MixKind::Recording => rec_block.clone(),
                MixKind::Stream => stream_block.clone(),
                MixKind::Input(i) => match solos.iter().find(|s| s.0 == *i) {
                    Some((_, s)) => AudioBlock { pts: t, rate: MIX_RATE, channels: 2, samples: s.clone() },
                    None => AudioBlock { pts: t, rate: MIX_RATE, channels: 2, samples: vec![0.0; BLOCK * 2] },
                },
                MixKind::Track { input, post_fader } => match tracks.iter().find(|s| s.0 == *input && s.1 == *post_fader) {
                    Some((_, _, s)) => AudioBlock { pts: t, rate: MIX_RATE, channels: 2, samples: s.clone() },
                    None => AudioBlock { pts: t, rate: MIX_RATE, channels: 2, samples: vec![0.0; BLOCK * 2] },
                },
            };
            let _ = tap.try_send(block);
        }
        n += 1;
        if n.is_multiple_of(5) {
            let mut st = status.lock().unwrap();
            for (i, s) in strips.iter_mut().enumerate() {
                if s.aligner.is_some() {
                    let (peak_db, rms_db) = s.meter.take();
                    st.meters.insert(format!("a{i}"), Meter { peak_db, rms_db });
                }
            }
            let (peak_db, rms_db) = rec.peaks.take();
            st.meters.insert("mix".into(), Meter { peak_db, rms_db });
            let (peak_db, rms_db) = stream.peaks.take();
            st.meters.insert("stream".into(), Meter { peak_db, rms_db });
            st.loudness = MixLoudness { recording: rec.loudness(), stream: stream.loudness(), stream_gain_db: levelling.then_some(leveller.gain_db as f32) };
        }
        t += BLOCK_NS;
        let now = clock::now();
        if now > t + latency + 500_000_000 {
            // Fell far behind (machine asleep?): jump to now.
            t = (now - latency) / BLOCK_NS * BLOCK_NS;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1 kHz tone burst at input time `beep`, sampled at an input clock
    /// running `ppm` fast. Returns blocks with (slightly jittery) host
    /// timestamps, as a real device would deliver.
    fn simulate(rate: u32, ppm: f64, seconds: f64, beep: Ns) -> Vec<(Ns, Vec<f32>)> {
        let true_rate = rate as f64 * (1.0 + ppm * 1e-6);
        let per_block = 512usize;
        let mut out = Vec::new();
        let mut frame = 0u64;
        let mut jitter = 12345u64;
        while (frame as f64) / true_rate < seconds {
            let t0 = frame as f64 / true_rate;
            jitter = jitter.wrapping_mul(6364136223846793005).wrapping_add(1);
            let j = ((jitter >> 33) % 400) as i64 * 1000 - 200_000; // ±0.2 ms
            let pts = 1_000_000_000 + (t0 * 1e9) as i64 + j;
            let mut s = Vec::with_capacity(per_block * 2);
            for i in 0..per_block {
                let t = 1_000_000_000 + (((frame + i as u64) as f64 / true_rate) * 1e9) as i64;
                let v = if t >= beep && t < beep + 5_000_000 { 0.9 } else { 0.0 };
                s.push(v);
                s.push(v);
            }
            out.push((pts, s));
            frame += per_block as u64;
        }
        out
    }

    fn run(rate: u32, ppm: f64, seconds: f64) -> (f64, Ns) {
        let beep = 1_000_000_000 + ((seconds - 2.0) * 1e9) as i64;
        let blocks = simulate(rate, ppm, seconds, beep);
        let mut a = Aligner::new(rate);
        let mut bi = 0;
        let mut t = 1_000_000_000i64;
        let mut out = vec![0.0f32; BLOCK * 2];
        let mut worst_late = 0.0f64;
        let mut beep_at = None;
        let end = 1_000_000_000 + ((seconds - 0.5) * 1e9) as i64;
        while t < end {
            // Deliver everything captured up to t + 50 ms (mixer latency).
            while bi < blocks.len() && blocks[bi].0 <= t + 50_000_000 {
                a.push(blocks[bi].0, &blocks[bi].1);
                bi += 1;
            }
            a.read(t, &mut out);
            if t > 1_000_000_000 + 10 * NS_PER_SEC {
                worst_late = worst_late.max(a.last_error_ns.abs());
            }
            if beep_at.is_none() {
                if let Some(i) = out.chunks(2).position(|f| f[0] > 0.45) {
                    beep_at = Some(t + i as i64 * NS_PER_SEC / MIX_RATE as i64);
                }
            }
            t += BLOCK_NS;
        }
        (worst_late, beep_at.expect("beep not found") - beep)
    }

    #[test]
    fn locks_to_drifting_clock() {
        // 80 ppm fast for 10 minutes = 48 ms of drift if uncorrected.
        let (err, beep_offset) = run(48_000, 80.0, 600.0);
        eprintln!("48k +80ppm: worst error {:.3} ms, beep offset {:.3} ms", err / 1e6, beep_offset as f64 / 1e6);
        assert!(err < 1.0e6, "alignment error {err} ns");
        assert!(beep_offset.abs() < 1_500_000, "beep {beep_offset} ns off");
    }

    #[test]
    fn converts_44k1() {
        let (err, beep_offset) = run(44_100, -30.0, 120.0);
        eprintln!("44.1k -30ppm: worst error {:.3} ms, beep offset {:.3} ms", err / 1e6, beep_offset as f64 / 1e6);
        assert!(err < 1.0e6, "alignment error {err} ns");
        assert!(beep_offset.abs() < 1_500_000, "beep {beep_offset} ns off");
    }

    #[test]
    fn meters() {
        let mut m = MeterAcc::default();
        m.add(&[0.5, -1.0, 0.5, 0.0], 2);
        let (p, r) = m.take();
        assert!((p[0] - -6.02).abs() < 0.1 && p[1].abs() < 0.01);
        assert!((r[0] - -6.02).abs() < 0.1);
    }
}
