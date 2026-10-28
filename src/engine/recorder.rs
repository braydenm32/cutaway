//! Recording a take: one file per source, the program, and WAVs.
//!
//! Every file shares one start time T0, aligned to the program's frame grid:
//! program frames start exactly at 0, each source is re-timed onto a
//! constant frame rate from T0, and audio is cut at the exact sample.
//!
//! ```text
//! source slot ─► retimer ─► encoder ─┐
//! program tap ─────────────► encoder ─┼─► MP4 writer (one per file)
//! program mix ─► AAC ─────────────────┘   (audio shared by every file)
//! input raw ───► WAV writer
//! ```

use crate::util::LockExt;
use super::encode::{self, AudioEncoder, Purpose, VideoSettings};
use super::{LogLevel, Status};
use crate::media::mp4::{Brand, Mp4Writer};
use crate::media::wav::{Bext, SampleFormat, WavWriter};
use crate::media::{AudioBlock, AudioConfig, Ns, VideoFrame, VideoPacket, AAC_FRAME, NS_PER_SEC};
use anyhow::{anyhow, Result};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Default, Clone)]
pub struct TakeReport {
    pub folder: PathBuf,
    pub files: Vec<(PathBuf, u64)>,
    pub problems: Vec<String>,
    pub duration: Duration,
    /// Markers (seconds into the take, name), for chapters.
    pub markers: Vec<(f64, String)>,
    /// Wall-clock time of the take's first frame (dates in titles and notes).
    pub recorded_at: Option<chrono::DateTime<chrono::Local>>,
    /// The podcast copy is still being made (uploads wait for it).
    pub podcast_pending: bool,
    /// The podcast copy, once made.
    pub podcast: Option<PathBuf>,
}

/// A marker dropped during a take (time from the start of the files).
#[derive(Clone, Debug)]
pub struct Marker {
    pub at: Ns,
    pub name: String,
    /// The scene on air at the time.
    pub scene: String,
    /// Left by a saved clip: on the editing timelines, not a chapter.
    pub clip: bool,
}

pub type Markers = Arc<Mutex<Vec<Marker>>>;

/// What every file of a take shares.
#[derive(Clone)]
pub struct FileOpts {
    /// The take's folder (files may sit in subfolders of it).
    pub take_folder: PathBuf,
    /// Folder for a second copy of every file (a backup drive).
    pub backup_folder: Option<PathBuf>,
    /// Time of day at the first frame, in seconds since midnight.
    pub tc_seconds: Option<f64>,
    /// Date and time of the first frame, for broadcast WAV ("YYYY-MM-DD", "HH:MM:SS").
    pub tc_date: (String, String),
    pub markers: Markers,
}

impl FileOpts {
    /// Where the backup copy of `path` goes (same place inside the take).
    pub fn backup_for(&self, path: &std::path::Path) -> Option<PathBuf> {
        let rel = path.strip_prefix(&self.take_folder).ok().map(|r| r.to_path_buf()).or_else(|| path.file_name().map(PathBuf::from))?;
        Some(self.backup_folder.as_ref()?.join(rel))
    }
}

/// A small H.264 copy made alongside a video file.
pub struct ProxyJob {
    pub settings: VideoSettings,
    /// Shrink by this whole factor.
    pub factor: u32,
    pub mux: Sender<MuxMsg>,
}

/// Whole factor that brings `width` down to at most `target`.
pub fn proxy_factor(width: u32, target: u32) -> u32 {
    width.div_ceil(target.max(160)).max(1)
}

/// Timecode start frame for a file at `fps`.
pub fn tc_frame(seconds: f64, fps: u32) -> u32 {
    (seconds * fps as f64).round() as u32
}

pub enum MuxMsg {
    Video(VideoPacket),
    /// The video ended before the take did (its source left the show): the
    /// file ends there, audio included.
    VideoEnd(Ns),
    Audio(Ns, Arc<Vec<u8>>),
    /// The audio's real length (its last frame is padded).
    AudioEnd(Ns),
    /// An extra audio track (one input on its own), by number.
    Track(usize, Ns, Arc<Vec<u8>>),
    TrackEnd(usize, Ns),
}

/// Writes one MP4/MOV; created when the first video packet arrives (its
/// decoder configuration is needed for the header).
#[allow(clippy::too_many_arguments)]
pub fn spawn_mp4(
    path: PathBuf,
    brand: Brand,
    fps: u32,
    audio: Option<AudioConfig>,
    tracks: Vec<(String, AudioConfig)>,
    rx: Receiver<MuxMsg>,
    status: Arc<Mutex<Status>>,
    label: String,
    opts: FileOpts,
    clips: Option<Arc<super::clips::ClipBuffer>>,
) -> JoinHandle<Result<(PathBuf, u64)>> {
    std::thread::Builder::new()
        .name(format!("write {label}"))
        .spawn(move || {
            let backup = opts.backup_for(&path);
            let report_backup = |w: &mut Mp4Writer| {
                if let Some(e) = w.take_backup_error() {
                    status.lock_ok().push(LogLevel::Warning, format!("{label}: {e}. The main recording continues."));
                }
            };
            let mut writer: Option<(Mp4Writer, usize, Option<usize>)> = None;
            let mut early_audio: Vec<(Ns, Arc<Vec<u8>>)> = Vec::new();
            let mut early_tracks: Vec<(usize, Ns, Arc<Vec<u8>>)> = Vec::new();
            let mut track_ids: Vec<usize> = Vec::new();
            let mut track_ends: Vec<(usize, Ns)> = Vec::new();
            let mut failed: Option<anyhow::Error> = None;
            let mut audio_end = None;
            let mut video_end: Option<Ns> = None;
            for msg in rx {
                if let Some(c) = &clips {
                    c.push(&msg);
                }
                if failed.is_some() {
                    continue; // keep draining so producers never block
                }
                let r = (|| -> Result<()> {
                    match msg {
                        MuxMsg::Video(p) => {
                            if writer.is_none() {
                                let mut w = Mp4Writer::create_with_backup(&path, backup.as_deref(), brand)?;
                                report_backup(&mut w);
                                let v = w.add_video(&p.config, fps);
                                let a = audio.as_ref().map(|a| w.add_audio(a));
                                if let (Some(a), false) = (a, tracks.is_empty()) {
                                    // The mix plays; each input is an alternative track editors see.
                                    w.set_track_info(a, "Mix", true, 1);
                                    for (name, cfg) in &tracks {
                                        let t = w.add_audio(cfg);
                                        w.set_track_info(t, name, false, 1);
                                        track_ids.push(t);
                                    }
                                }
                                if let Some(secs) = opts.tc_seconds {
                                    w.set_timecode(fps, tc_frame(secs, fps));
                                }
                                writer = Some((w, v, a));
                            }
                            let (w, v, a) = writer.as_mut().unwrap();
                            if let Some(a) = a {
                                for (t, d) in early_audio.drain(..) {
                                    w.write(*a, t, &d, true)?;
                                }
                            }
                            for (k, t, d) in early_tracks.drain(..) {
                                if let Some(&id) = track_ids.get(k) {
                                    w.write(id, t, &d, true)?;
                                }
                            }
                            w.write(*v, p.dts, &p.data, p.keyframe)?;
                            if p.keyframe {
                                report_backup(w);
                            }
                        }
                        MuxMsg::Audio(t, _) if video_end.is_some_and(|end| t >= end) => {}
                        MuxMsg::Audio(t, d) => match writer.as_mut() {
                            Some((w, _, Some(a))) => w.write(*a, t, &d, true)?,
                            Some(_) => {}
                            None => early_audio.push((t, d)),
                        },
                        MuxMsg::AudioEnd(ns) => audio_end = Some(ns),
                        MuxMsg::VideoEnd(ns) => video_end = Some(ns),
                        MuxMsg::Track(k, t, d) => match writer.as_mut() {
                            Some((w, ..)) => {
                                if let Some(&id) = track_ids.get(k) {
                                    w.write(id, t, &d, true)?;
                                }
                            }
                            None => early_tracks.push((k, t, d)),
                        },
                        MuxMsg::TrackEnd(k, ns) => track_ends.push((k, ns)),
                    }
                    Ok(())
                })();
                if let Err(e) = r {
                    let msg = format!("{label}: {e:#}");
                    let mut st = status.lock_ok();
                    st.push(LogLevel::Error, format!("Recording {msg}"));
                    st.rec_failed = Some(msg);
                    failed = Some(e);
                }
            }
            if let Some(e) = failed {
                if let Some((w, ..)) = writer {
                    let _ = w.finish();
                }
                return Err(e);
            }
            match writer {
                Some((mut w, _, a)) => {
                    let audio_end = match (audio_end, video_end) {
                        (Some(a), Some(v)) => Some(a.min(v)),
                        (a, v) => a.or(v),
                    };
                    if let (Some(a), Some(ns)) = (a, audio_end) {
                        w.set_duration(a, ns);
                    }
                    for (k, ns) in track_ends {
                        if let Some(&id) = track_ids.get(k) {
                            w.set_duration(id, ns);
                        }
                    }
                    let chapters = opts.markers.lock_ok().iter().filter(|m| !m.clip).map(|m| (m.at, m.name.clone())).collect();
                    w.set_chapters(chapters);
                    report_backup(&mut w);
                    Ok((path, w.finish()?))
                }
                None => Err(anyhow!("{label}: no video arrived, so no file was written")),
            }
        })
        .expect("thread")
}

/// Puts a source's frames onto a constant frame rate starting at T0:
/// repeats a frame when the source skips, drops extras when it's fast.
pub struct Retimer {
    t0: Ns,
    fps: u32,
    delay: Ns,
    next: i64,
    last: Option<VideoFrame>,
    /// Slots filled by repeating a picture (the source skipped).
    pub repeated: u64,
    /// Pictures that arrived for an already-filled slot.
    pub dropped: u64,
}

impl Retimer {
    pub fn new(t0: Ns, fps: u32, delay: Ns) -> Self {
        Retimer { t0, fps: fps.max(1), delay, next: 0, last: None, repeated: 0, dropped: 0 }
    }

    pub fn slot_time(&self, k: i64) -> Ns {
        k * NS_PER_SEC / self.fps as i64
    }

    /// Frames to encode, with their output times.
    pub fn push(&mut self, frame: VideoFrame) -> Vec<(Ns, VideoFrame)> {
        let rel = frame.pts + self.delay - self.t0;
        let k = (rel as i128 * self.fps as i128 + NS_PER_SEC as i128 / 2).div_euclid(NS_PER_SEC as i128) as i64;
        let mut out = Vec::new();
        if k < self.next {
            // Belongs to a slot that's already filled (or is from before
            // the start): it's just the current picture now.
            if k >= 0 {
                self.dropped += 1;
            }
            self.last = Some(frame);
            return out;
        }
        // Slots the source skipped repeat the previous picture (or this one,
        // at the very start).
        while self.next < k {
            let f = self.last.clone().unwrap_or_else(|| frame.clone());
            out.push((self.slot_time(self.next), f));
            self.next += 1;
            self.repeated += 1;
        }
        out.push((self.slot_time(self.next), frame.clone()));
        self.next += 1;
        self.last = Some(frame);
        out
    }

    /// Where the frames so far end (relative to T0).
    pub fn end(&self) -> Ns {
        self.slot_time(self.next)
    }

    /// Repeat the last frame up to `end` (relative to T0).
    pub fn finish(&mut self, end: Ns) -> Vec<(Ns, VideoFrame)> {
        let mut out = Vec::new();
        if let Some(f) = &self.last {
            while self.slot_time(self.next) < end {
                out.push((self.slot_time(self.next), f.clone()));
                self.next += 1;
            }
        }
        out
    }
}

/// Live figures for one recorded video file (shown in the health panel).
#[derive(Default)]
pub struct VideoStats {
    pub encoded: AtomicU64,
    pub repeated: AtomicU64,
    pub dropped: AtomicU64,
    /// Pictures of the wrong size (the device changed mode) replaced by the
    /// previous one.
    pub resized: AtomicU64,
    /// Frames waiting to be encoded.
    pub backlog: AtomicU64,
    pub failed: AtomicBool,
}

/// Encodes frames for one file. With a retimer the input is raw captured
/// frames; without, frames are already on the grid (the program).
#[allow(clippy::too_many_arguments)]
pub fn spawn_video(
    label: String,
    rx: Receiver<VideoFrame>,
    mut settings: VideoSettings,
    mut retimer: Option<Retimer>,
    t0: Ns,
    t1: Arc<AtomicI64>,
    mux: Sender<MuxMsg>,
    status: Arc<Mutex<Status>>,
    stats: Arc<VideoStats>,
    mut proxy: Option<ProxyJob>,
) -> JoinHandle<Result<()>> {
    std::thread::Builder::new()
        .name(format!("encode {label}"))
        .spawn(move || {
            let mut enc: Option<Box<dyn encode::VideoEncoder>> = None;
            let mut proxy_enc: Option<Box<dyn encode::VideoEncoder>> = None;
            let mut last_good: Option<VideoFrame> = None;
            let mut warned_size = false;
            let mut run = |f: &VideoFrame, pts: Ns, enc: &mut Option<Box<dyn encode::VideoEncoder>>| -> Result<()> {
                if enc.is_none() {
                    settings.width = f.width;
                    settings.height = f.height;
                    let m = mux.clone();
                    *enc = Some(encode::create_video(&settings, Arc::new(move |p| {
                        let _ = m.send(MuxMsg::Video(p));
                    }))?);
                }
                // A device that came back in a different mode: keep the file
                // going with the last good picture rather than failing it.
                let f = if f.width != settings.width || f.height != settings.height {
                    stats.resized.fetch_add(1, Ordering::Relaxed);
                    if !warned_size {
                        warned_size = true;
                        status.lock_ok().push(
                            LogLevel::Warning,
                            format!("{label}: the source changed to {}×{}; its file repeats the last picture until the next take", f.width, f.height),
                        );
                    }
                    match &last_good {
                        Some(g) => g.clone(),
                        None => return Ok(()),
                    }
                } else {
                    last_good = Some(f.clone());
                    f.clone()
                };
                enc.as_mut().unwrap().encode(&f, pts, false)?;
                stats.encoded.fetch_add(1, Ordering::Relaxed);
                // The proxy never stops the real file: a problem just ends it.
                if let Some(p) = proxy.as_mut() {
                    let small = f.downscaled(p.factor);
                    let r = (|| -> Result<()> {
                        if proxy_enc.is_none() {
                            p.settings.width = small.width;
                            p.settings.height = small.height;
                            let m = p.mux.clone();
                            proxy_enc = Some(encode::create_video(&p.settings, Arc::new(move |pk| {
                                let _ = m.send(MuxMsg::Video(pk));
                            }))?);
                        }
                        proxy_enc.as_mut().unwrap().encode(&small, pts, false)
                    })();
                    if let Err(e) = r {
                        status.lock_ok().push(LogLevel::Warning, format!("{label} proxy stopped: {e:#}"));
                        proxy = None;
                        proxy_enc = None;
                    }
                }
                Ok(())
            };
            // Frames are encoded once their time has come: a delayed source
            // delivers frames ahead of the program, and a frame past the
            // end of the take (decided when Stop is pressed) must not get in.
            let mut held: VecDeque<(Ns, VideoFrame)> = VecDeque::new();
            let mut result = Ok(());
            let mut open = true;
            while open || !held.is_empty() {
                match rx.recv_timeout(Duration::from_millis(10)) {
                    Ok(frame) => match retimer.as_mut() {
                        Some(r) => held.extend(r.push(frame)),
                        None => held.push_back((frame.pts - t0, frame)),
                    },
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => open = false,
                }
                let now = crate::media::clock::now();
                let end = t1.load(Ordering::Relaxed);
                while let Some((pts, _)) = held.front() {
                    if open && t0 + pts > now {
                        break;
                    }
                    let (pts, f) = held.pop_front().unwrap();
                    if t0 + pts >= end || result.is_err() {
                        continue;
                    }
                    if let Err(e) = run(&f, pts, &mut enc) {
                        status.lock_ok().push(LogLevel::Error, format!("{label}: {e:#}"));
                        stats.failed.store(true, Ordering::Relaxed);
                        result = Err(e);
                    }
                }
                if let Some(r) = &retimer {
                    stats.repeated.store(r.repeated, Ordering::Relaxed);
                    stats.dropped.store(r.dropped, Ordering::Relaxed);
                }
                stats.backlog.store((held.len() + rx.len()) as u64, Ordering::Relaxed);
            }
            // Frames stopped coming before the take's end was decided: the
            // source left the show, and its file ends here.
            let left = t1.load(Ordering::Relaxed) > crate::media::clock::now();
            if result.is_ok() {
                if let Some(r) = retimer.as_mut() {
                    let end = if left { crate::media::clock::now() } else { t1.load(Ordering::Relaxed) } - t0;
                    for (pts, f) in r.finish(end) {
                        run(&f, pts, &mut enc)?;
                    }
                }
            }
            if left {
                if let Some(r) = &retimer {
                    let _ = mux.send(MuxMsg::VideoEnd(r.end()));
                    if let Some(p) = &proxy {
                        let _ = p.mux.send(MuxMsg::VideoEnd(r.end()));
                    }
                }
            }
            if let Some(mut e) = enc {
                e.finish()?;
            }
            if let Some(mut e) = proxy_enc {
                let _ = e.finish();
            }
            result
        })
        .expect("thread")
}

/// Sample range of `block` that falls in [t0, t1).
pub fn trim(block: &AudioBlock, t0: Ns, t1: Ns) -> std::ops::Range<usize> {
    let frames = block.frames();
    let to_frames = |ns: Ns| ((ns as i128 * block.rate as i128 + NS_PER_SEC as i128 / 2) / NS_PER_SEC as i128) as i64;
    let start = to_frames(t0 - block.pts).clamp(0, frames as i64) as usize;
    let end = to_frames(t1 - block.pts).clamp(0, frames as i64) as usize;
    start..end.max(start)
}

/// Encodes a mix once and hands the frames to every file: the program
/// mix (`track` None), or one input for its own track (stereo blocks in;
/// `mono` keeps the left channel).
pub fn spawn_audio(rx: Receiver<AudioBlock>, t0: Ns, t1: Arc<AtomicI64>, mut aac: Box<dyn AudioEncoder>, outs: Vec<Sender<MuxMsg>>, track: Option<usize>, mono: bool) -> JoinHandle<Result<()>> {
    std::thread::Builder::new()
        .name("encode audio".into())
        .spawn(move || {
            let cfg = aac.config();
            let rate = cfg.rate as i64;
            let mut cursor = AudioCursor::new(t0, cfg.rate, if mono { 2 } else { cfg.channels as usize });
            let mut n = 0i64;
            let mut send = |frames: Vec<Vec<u8>>| {
                for f in frames {
                    let t = n * AAC_FRAME as i64 * NS_PER_SEC / rate;
                    let d = Arc::new(f);
                    for o in &outs {
                        let _ = o.send(match track {
                            None => MuxMsg::Audio(t, d.clone()),
                            Some(k) => MuxMsg::Track(k, t, d.clone()),
                        });
                    }
                    n += 1;
                }
            };
            for b in rx.iter() {
                let samples = cursor.take(&b, t1.load(Ordering::Relaxed));
                if !samples.is_empty() {
                    let samples = if mono { samples.chunks_exact(2).map(|f| f[0]).collect() } else { samples };
                    send(aac.push(&samples)?);
                }
            }
            send(aac.finish()?);
            let end = cursor.position_ns();
            for o in &outs {
                let _ = o.send(match track {
                    None => MuxMsg::AudioEnd(end),
                    Some(k) => MuxMsg::TrackEnd(k, end),
                });
            }
            Ok(())
        })
        .expect("thread")
}

/// Turns program-mix blocks into one gapless run of samples from `t0`:
/// trims to [t0, t1), fills gaps with silence and drops overlaps, so the
/// audio's position always matches its timestamps.
pub struct AudioCursor {
    t0: Ns,
    rate: u32,
    channels: usize,
    pushed: u64,
}

impl AudioCursor {
    pub fn new(t0: Ns, rate: u32, channels: usize) -> Self {
        AudioCursor { t0, rate, channels: channels.max(1), pushed: 0 }
    }

    /// Length of the audio handed out so far.
    pub fn position_ns(&self) -> Ns {
        (self.pushed as i128 * NS_PER_SEC as i128 / self.rate.max(1) as i128) as Ns
    }

    pub fn take(&mut self, b: &AudioBlock, t1: Ns) -> Vec<f32> {
        let r = trim(b, self.t0, t1);
        if r.is_empty() || b.channels as usize != self.channels {
            return Vec::new();
        }
        let ch = self.channels;
        let block_start = ((b.pts - self.t0) as i128 * self.rate as i128 + NS_PER_SEC as i128 / 2).div_euclid(NS_PER_SEC as i128) as i64;
        let at = block_start + r.start as i64;
        let tolerance = (self.rate / 50) as i64; // 20 ms
        let mut out = Vec::with_capacity((r.len() + 16) * ch);
        let mut skip = 0usize;
        let pushed = self.pushed as i64;
        if at > pushed + tolerance {
            out.resize((at - pushed) as usize * ch, 0.0);
        } else if at + tolerance < pushed {
            skip = ((pushed - at) as usize).min(r.len());
        }
        out.extend_from_slice(&b.samples[(r.start + skip) * ch..r.end * ch]);
        self.pushed += (out.len() / ch) as u64;
        out
    }
}

/// Writes one input's raw audio (one multichannel file, or one per channel).
#[allow(clippy::too_many_arguments)]
pub fn spawn_wav(
    rx: Receiver<AudioBlock>,
    base: PathBuf,
    bits: u32,
    split: bool,
    t0: Ns,
    t1: Arc<AtomicI64>,
    status: Arc<Mutex<Status>>,
    opts: FileOpts,
) -> JoinHandle<Result<Vec<(PathBuf, u64)>>> {
    std::thread::Builder::new()
        .name("write wav".into())
        .spawn(move || {
            let label = base.file_name().unwrap_or_default().to_string_lossy().to_string();
            let mut w = WavJob { base, format: SampleFormat::from_bits(bits), split, t0, writers: Vec::new(), scratch: Vec::new(), expected: None, fmt: None, opts };
            // Blocks are written once their time has come (a delayed input
            // runs ahead of the clock), so nothing past the end gets in.
            let mut held: VecDeque<AudioBlock> = VecDeque::new();
            let mut open = true;
            let mut result = Ok(());
            while open || !held.is_empty() {
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(b) => held.push_back(b),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => open = false,
                }
                let now = crate::media::clock::now();
                while let Some(b) = held.front() {
                    let end_of_block = b.pts + b.frames() as i64 * NS_PER_SEC / b.rate.max(1) as i64;
                    if open && end_of_block > now {
                        break;
                    }
                    let b = held.pop_front().unwrap();
                    if result.is_ok() {
                        result = w.write(&b, t1.load(Ordering::Relaxed));
                        for (_, wr) in w.writers.iter_mut() {
                            if let Some(e) = wr.take_backup_error() {
                                status.lock_ok().push(LogLevel::Warning, format!("{label}: {e}. The main recording continues."));
                            }
                        }
                        if let Err(e) = &result {
                            let msg = format!("{label}: {e:#}");
                            let mut st = status.lock_ok();
                            st.push(LogLevel::Error, format!("Recording {msg}"));
                            st.rec_failed = Some(msg);
                        }
                    }
                }
            }
            // Every file gets its proper header, even if one can't.
            let mut out = Vec::new();
            let mut failed = None;
            for (p, w) in w.writers {
                match w.finish() {
                    Ok(size) => out.push((p, size)),
                    Err(e) => failed = failed.or(Some(e)),
                }
            }
            result?;
            match failed {
                Some(e) => Err(e),
                None => Ok(out),
            }
        })
        .expect("thread")
}

struct WavJob {
    base: PathBuf,
    format: SampleFormat,
    split: bool,
    t0: Ns,
    writers: Vec<(PathBuf, WavWriter)>,
    scratch: Vec<f32>,
    /// Host time where the next sample belongs.
    expected: Option<Ns>,
    /// The file's rate and channels (from its first block).
    fmt: Option<(u32, u16)>,
    opts: FileOpts,
}

impl WavJob {
    fn write(&mut self, b: &AudioBlock, t1: Ns) -> Result<()> {
        // A device that comes back in another format (a USB mic plugged
        // in again at 44.1 kHz mono): made to fit the file as it started.
        let conformed;
        let b = match self.fmt {
            Some((rate, ch)) if (b.rate, b.channels) != (rate, ch) => {
                conformed = conform(b, rate, ch);
                &conformed
            }
            _ => b,
        };
        self.fmt.get_or_insert((b.rate, b.channels));
        let r = trim(b, self.t0, t1);
        if r.is_empty() {
            return Ok(());
        }
        let ch = b.channels as usize;
        let first_at = b.pts + r.start as i64 * NS_PER_SEC / b.rate as i64;
        // Silence for audio that never arrived: before the first block (an
        // input that started late) or a dropout. Small differences are
        // just clock jitter.
        let gap = first_at - self.expected.unwrap_or(self.t0);
        let silence = if gap > 20_000_000 { (gap as i128 * b.rate as i128 / NS_PER_SEC as i128) as usize } else { 0 };
        self.expected = Some(first_at + r.len() as i64 * NS_PER_SEC / b.rate as i64);
        if self.writers.is_empty() {
            let bext = self.opts.tc_seconds.map(|secs| Bext {
                description: format!("Cutaway {}", self.base.file_name().unwrap_or_default().to_string_lossy()),
                date: self.opts.tc_date.0.clone(),
                time: self.opts.tc_date.1.clone(),
                time_reference: (secs * b.rate as f64).round() as u64,
            });
            let mut paths = Vec::new();
            if self.split && ch > 1 {
                for c in 0..ch {
                    paths.push((self.base.with_file_name(format!("{}_ch{}.wav", self.base.file_name().unwrap().to_string_lossy(), c + 1)), 1));
                }
            } else {
                paths.push((self.base.with_extension("wav"), b.channels));
            }
            for (p, channels) in paths {
                let backup = self.opts.backup_for(&p);
                let w = WavWriter::create_with(&p, backup.as_deref(), self.format, channels, b.rate, bext.as_ref())?;
                self.writers.push((p, w));
            }
        }
        if silence > 0 {
            let n = silence.min(b.rate as usize * 3600);
            for (_, w) in self.writers.iter_mut() {
                w.write(&vec![0.0f32; n * w.channels() as usize])?;
            }
        }
        let samples = &b.samples[r.start * ch..r.end * ch];
        if self.writers.len() == 1 {
            self.writers[0].1.write(samples)?;
        } else {
            for (c, (_, w)) in self.writers.iter_mut().enumerate() {
                self.scratch.clear();
                self.scratch.extend(samples.iter().skip(c).step_by(ch));
                w.write(&self.scratch)?;
            }
        }
        Ok(())
    }
}

/// `b` at another sample rate and channel count (linear in-between
/// samples: only for the rare device that changes mid-take).
fn conform(b: &AudioBlock, rate: u32, channels: u16) -> AudioBlock {
    let (ic, oc) = (b.channels.max(1) as usize, channels.max(1) as usize);
    let frames = b.frames();
    let out_frames = if b.rate == rate || b.rate == 0 { frames } else { (frames as u64 * rate as u64 / b.rate as u64) as usize };
    let mut samples = Vec::with_capacity(out_frames * oc);
    let at = |f: usize, c: usize| -> f32 {
        let f = f.min(frames.saturating_sub(1));
        match (ic, c) {
            (1, _) => b.samples.get(f).copied().unwrap_or(0.0),
            (_, c) if c < ic => b.samples.get(f * ic + c).copied().unwrap_or(0.0),
            _ => 0.0,
        }
    };
    for o in 0..out_frames {
        let pos = if out_frames == frames { o as f64 } else { o as f64 * b.rate as f64 / rate as f64 };
        let (i, frac) = (pos.floor() as usize, (pos - pos.floor()) as f32);
        for c in 0..oc {
            samples.push(at(i, c) * (1.0 - frac) + at(i + 1, c) * frac);
        }
    }
    AudioBlock { pts: b.pts, rate, channels, samples }
}

/// Everything running for one take.
pub struct Take {
    pub folder: PathBuf,
    pub t0: Ns,
    pub t1: Arc<AtomicI64>,
    pub started: std::time::Instant,
    /// Wall-clock time of the first frame.
    pub wall: chrono::DateTime<chrono::Local>,
    pub first_scene: String,
    pub first_angle: Option<usize>,
    pub video_jobs: Vec<(String, JoinHandle<Result<()>>)>,
    pub audio_job: Option<JoinHandle<Result<()>>>,
    /// One encoder per input track.
    pub track_jobs: Vec<(String, JoinHandle<Result<()>>)>,
    pub writers: Vec<(String, JoinHandle<Result<(PathBuf, u64)>>)>,
    pub wavs: Vec<(String, JoinHandle<Result<Vec<(PathBuf, u64)>>>)>,
    pub mix_tap_ids: Vec<u64>,
    pub markers: Markers,
    pub opts: FileOpts,
    /// Every video file, for the editor timelines.
    pub export_files: Vec<super::export::ExportFile>,
    /// Time of day at the first frame (seconds since midnight).
    pub tc_seconds: f64,
    /// The sources and inputs being recorded, as they were when the take
    /// started (they may be removed from the show before it ends).
    pub iso_slots: Vec<(String, Arc<super::sources::SourceSlot>)>,
    pub raw_inputs: Vec<Arc<super::audio::InputShared>>,
}

impl Take {
    /// Wait for every job and collect the files (taps must be removed
    /// first), saying what it's waiting for and how far along it is (0 to 1)
    /// as it goes.
    pub fn collect_with(self, mut step: impl FnMut(String, f32)) -> TakeReport {
        let mut report = TakeReport { folder: self.folder.clone(), duration: self.started.elapsed(), recorded_at: Some(self.wall), ..Default::default() };
        let total = (self.video_jobs.len() + self.audio_job.is_some() as usize + self.track_jobs.len() + self.writers.len() + self.wavs.len()).max(1);
        let mut k = 0;
        let mut next = |what: String| {
            step(what, k as f32 / total as f32);
            k += 1;
        };
        for (label, h) in self.video_jobs {
            next(format!("Finishing the {label} video"));
            match h.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => report.problems.push(format!("{label}: {e:#}")),
                Err(_) => report.problems.push(format!("{label}: encoder crashed")),
            }
        }
        if let Some(h) = self.audio_job {
            next("Finishing the sound".into());
            match h.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => report.problems.push(format!("audio: {e:#}")),
                Err(_) => report.problems.push("audio: the sound encoder crashed".into()),
            }
        }
        for (label, h) in self.track_jobs {
            next(format!("Finishing {label}'s track"));
            match h.join() {
                Ok(Ok(())) => {}
                Ok(Err(e)) => report.problems.push(format!("{label} track: {e:#}")),
                Err(_) => report.problems.push(format!("{label} track: encoder crashed")),
            }
        }
        for (label, h) in self.writers {
            next(format!("Saving {label}"));
            match h.join() {
                Ok(Ok(f)) => report.files.push(f),
                Ok(Err(e)) => report.problems.push(format!("{e:#}")),
                Err(_) => report.problems.push(format!("{label}: writer crashed")),
            }
        }
        for (label, h) in self.wavs {
            next(format!("Saving {label}'s audio"));
            match h.join() {
                Ok(Ok(files)) => report.files.extend(files),
                Ok(Err(e)) => report.problems.push(format!("{label}: {e:#}")),
                Err(_) => report.problems.push(format!("{label}: writer crashed")),
            }
        }
        report
    }
}

pub fn video_settings(codec: crate::media::VideoCodec, fps: u32, bitrate_kbps: u32, backend: &str) -> VideoSettings {
    VideoSettings { codec, width: 0, height: 0, fps, bitrate_kbps, purpose: Purpose::Record, backend: backend.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conforms_a_block_to_the_file() {
        let b = AudioBlock { pts: 0, rate: 24_000, channels: 1, samples: vec![0.0, 1.0, 0.0, 1.0] };
        let c = conform(&b, 48_000, 2);
        assert_eq!((c.rate, c.channels, c.frames()), (48_000, 2, 8));
        assert_eq!(&c.samples[..6], &[0.0, 0.0, 0.5, 0.5, 1.0, 1.0]);
        let s = AudioBlock { pts: 0, rate: 48_000, channels: 2, samples: vec![0.1, 0.2, 0.3, 0.4] };
        assert_eq!(conform(&s, 48_000, 1).samples, vec![0.1, 0.3]);
    }

    fn frame(pts: Ns) -> VideoFrame {
        VideoFrame::new_nv12(2, 2, pts, vec![0; 4], vec![0; 2], 0)
    }

    #[test]
    fn retimer_starts_with_the_earlier_picture_if_slot_0_is_missed() {
        let t0 = 1_000_000_000;
        let mut r = Retimer::new(t0, 30, 0);
        assert!(r.push(frame(t0 - 30_000_000)).is_empty());
        // The source skipped slot 0: the earlier picture fills it.
        let out = r.push(frame(t0 + 34_000_000));
        assert_eq!(out.iter().map(|o| (o.0, o.1.pts)).collect::<Vec<_>>(), vec![(0, t0 - 30_000_000), (33_333_333, t0 + 34_000_000)]);
    }

    #[test]
    fn retimer_fills_and_drops() {
        let t0 = 1_000_000_000;
        let mut r = Retimer::new(t0, 30, 0);
        // A frame from before the start only waits in the wings ...
        assert!(r.push(frame(t0 - 30_000_000)).is_empty());
        // ... and the one for slot 0 takes it.
        let out = r.push(frame(t0 - 10_000_000));
        assert_eq!(out.iter().map(|o| o.0).collect::<Vec<_>>(), vec![0]);
        // Next frame skipped two slots: slots 1 and 2 repeat, then slot 3.
        let out = r.push(frame(t0 + 100_000_000));
        assert_eq!(out.len(), 3);
        assert_eq!(out[2].0, 100_000_000);
        // A frame for an already-filled slot is dropped.
        assert!(r.push(frame(t0 + 101_000_000)).is_empty());
        // Finish up to 200 ms: slots 4, 5 repeat (133, 166 ms).
        let out = r.finish(200_000_000);
        assert_eq!(out.iter().map(|o| o.0).collect::<Vec<_>>(), vec![133_333_333, 166_666_666]);
    }

    #[test]
    fn cursor_fills_gaps() {
        let blk = |pts: Ns| AudioBlock { pts, rate: 48000, channels: 2, samples: vec![1.0; 960] };
        let mut c = AudioCursor::new(1_000_000_000, 48000, 2);
        assert_eq!(c.take(&blk(995_000_000), i64::MAX).len(), 240 * 2); // trimmed to t0
        assert_eq!(c.take(&blk(1_005_000_000), i64::MAX).len(), 480 * 2); // contiguous
        let gap = c.take(&blk(1_045_000_000), i64::MAX); // 30 ms gap
        assert_eq!(gap.len(), (1440 + 480) * 2);
        assert!(gap[..2880].iter().all(|s| *s == 0.0));
        assert_eq!(c.take(&blk(1_020_000_000), i64::MAX).len(), 0); // already covered: dropped
    }

    #[test]
    fn trimming() {
        let b = AudioBlock { pts: 1_000_000_000, rate: 48000, channels: 2, samples: vec![0.0; 960] };
        assert_eq!(trim(&b, 1_005_000_000, i64::MAX), 240..480);
        assert_eq!(trim(&b, 0, 1_002_500_000), 0..120);
        assert!(trim(&b, 2_000_000_000, i64::MAX).is_empty());
    }
}
