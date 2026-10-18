//! Stinger transitions: a short video (a logo sweeping across, a burst of
//! colour) plays over the program and the scenes change underneath it at
//! its cut point, hidden by the video.
//!
//! Its see-through parts come from a matte: a black-and-white copy beside
//! or below the picture in the same file (most stinger packs include one,
//! and it works with any H.264 file), or black meaning see-through. The
//! first frames are decoded ahead of time so it starts the moment it's
//! triggered; the rest are decoded as it plays.

use super::decode::{create_video_decoder, DecoderSetup, Reorder, VideoDecoder};
use crate::config::{Matte, StingerConfig};
use crate::media::demux::{Mp4Reader, TrackKind};
use crate::media::{Ns, VideoFrame, NS_PER_SEC};
use anyhow::{anyhow, Context, Result};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Frames decoded before it's triggered.
const PRE: usize = 6;
/// Frames decoded ahead of what's showing while it plays.
const AHEAD: usize = 8;

/// A loaded stinger file.
struct Clip {
    path: PathBuf,
    matte: Matte,
    duration: Ns,
    cut: Ns,
    frame_ns: Ns,
    setup: DecoderSetup,
    /// Compressed pictures in decode order, with their times.
    packets: Vec<(Ns, Vec<u8>)>,
    bframes: bool,
    sound: Option<Arc<Vec<f32>>>,
    volume_db: f32,
}

/// What the settings show about the stinger.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StingerInfo {
    pub loaded: bool,
    pub loading: bool,
    pub problem: Option<String>,
    pub seconds: f64,
    pub cut_seconds: f64,
    /// How its see-through parts were found (never Auto).
    pub matte: Option<Matte>,
    pub size: (u32, u32),
    pub has_sound: bool,
}

/// What a trigger starts.
pub struct Started {
    pub duration: Ns,
    pub cut: Ns,
    pub sound: Option<Arc<Vec<f32>>>,
    pub volume_db: f32,
}

#[derive(Default)]
struct State {
    wanted: Option<StingerConfig>,
    wanted_gen: u64,
    clip: Option<Arc<Clip>>,
    loading: bool,
    problem: Option<String>,
    /// Bumped by every trigger.
    triggers: u64,
    triggered_at: Option<Instant>,
    ahead: VecDeque<VideoFrame>,
    playhead: Ns,
    stop: bool,
}

struct Shared {
    state: Mutex<State>,
    cond: Condvar,
}

pub struct StingerPlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl StingerPlayer {
    pub fn new(canvas: (u32, u32)) -> StingerPlayer {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), cond: Condvar::new() });
        let s2 = shared.clone();
        let thread = std::thread::Builder::new().name("stinger".into()).spawn(move || worker(s2, canvas)).ok();
        StingerPlayer { shared, thread }
    }

    /// New settings: a changed file (or matte) is loaded in the background.
    pub fn configure(&self, cfg: &StingerConfig) {
        let mut st = self.shared.state.lock().unwrap();
        let wanted = cfg.file.as_ref().filter(|f| !f.as_os_str().is_empty()).map(|_| cfg.clone());
        if st.wanted != wanted {
            st.wanted = wanted;
            st.wanted_gen += 1;
            self.shared.cond.notify_all();
        }
    }

    pub fn info(&self) -> StingerInfo {
        let st = self.shared.state.lock().unwrap();
        let mut i = StingerInfo { loading: st.loading, problem: st.problem.clone(), ..Default::default() };
        if let Some(c) = &st.clip {
            i.loaded = true;
            i.seconds = c.duration as f64 / 1e9;
            i.cut_seconds = c.cut as f64 / 1e9;
            i.matte = Some(c.matte);
            i.size = (c.setup.width, c.setup.height);
            i.has_sound = c.sound.is_some();
        }
        i
    }

    /// Wait for a load to finish.
    #[cfg(test)]
    pub fn wait_loaded(&self, timeout: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            {
                let st = self.shared.state.lock().unwrap();
                if !st.loading && st.wanted.is_some() == st.clip.is_some() && !st.ahead.is_empty() {
                    return true;
                }
                if st.problem.is_some() {
                    return false;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// Start it playing. None when there's no stinger ready.
    pub fn trigger(&self) -> Option<Started> {
        let mut st = self.shared.state.lock().unwrap();
        let c = st.clip.clone()?;
        st.triggers += 1;
        st.triggered_at = Some(Instant::now());
        st.playhead = 0;
        self.shared.cond.notify_all();
        Some(Started { duration: c.duration, cut: c.cut, sound: c.sound.clone(), volume_db: c.volume_db })
    }

    /// The picture to show `elapsed` after it started.
    pub fn frame_at(&self, elapsed: Ns) -> Option<VideoFrame> {
        let mut st = self.shared.state.lock().unwrap();
        st.playhead = elapsed;
        while st.ahead.len() >= 2 && st.ahead[1].pts <= elapsed {
            st.ahead.pop_front();
        }
        let half = st.clip.as_ref().map(|c| c.frame_ns / 2).unwrap_or(0);
        let f = st.ahead.front().filter(|f| f.pts <= elapsed + half).cloned();
        self.shared.cond.notify_all();
        f
    }

    /// How its see-through parts are stored (Solid when nothing's loaded).
    pub fn matte(&self) -> Matte {
        self.shared.state.lock().unwrap().clip.as_ref().map(|c| c.matte).unwrap_or(Matte::Solid)
    }
}

impl Drop for StingerPlayer {
    fn drop(&mut self) {
        self.shared.state.lock().unwrap().stop = true;
        self.shared.cond.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Which matte a file of `size` has, on a canvas of `canvas`.
pub fn detect_matte(size: (u32, u32), canvas: (u32, u32)) -> Matte {
    let r = (size.0 as f64 / size.1.max(1) as f64) / (canvas.0 as f64 / canvas.1.max(1) as f64);
    if r > 1.6 {
        Matte::Right
    } else if r < 0.65 {
        Matte::Below
    } else {
        Matte::Solid
    }
}

fn load(cfg: &StingerConfig, canvas: (u32, u32)) -> Result<Clip> {
    let path = cfg.file.clone().ok_or_else(|| anyhow!("no file"))?;
    let mut r = Mp4Reader::open(&path).with_context(|| format!("{}", path.display()))?;
    let vi = r.video().ok_or_else(|| anyhow!("{} has no video in it", name(&path)))?;
    let TrackKind::Video(vc) = r.tracks[vi].kind.clone() else { return Err(anyhow!("{} has no video in it", name(&path))) };
    let count = r.tracks[vi].samples.len();
    if count == 0 {
        return Err(anyhow!("{} has no pictures in it", name(&path)));
    }
    let duration = r.tracks[vi].duration().max(1);
    if duration > 10 * NS_PER_SEC {
        return Err(anyhow!("{} is {:.0} seconds long; a stinger should be a few seconds at most", name(&path), duration as f64 / 1e9));
    }
    let matte = match cfg.matte {
        Matte::Auto => detect_matte((vc.width, vc.height), canvas),
        m => m,
    };
    let (cw, ch) = canvas;
    let output = match matte {
        Matte::Right => (cw * 2, ch),
        Matte::Below => (cw, ch * 2),
        _ => (cw, ch),
    };
    let mut packets = Vec::with_capacity(count);
    for i in 0..count {
        packets.push((r.tracks[vi].pts(i), r.read(vi, i)?));
    }
    let bframes = r.tracks[vi].samples.iter().take(30).any(|s| s.cts != 0);
    let frame_ns = (duration / count as Ns).max(1);
    let cut = if cfg.cut_ms == 0 { duration / 2 } else { (cfg.cut_ms as Ns * 1_000_000).min(duration) };
    // Its sound, if it has any.
    let sound = r.audio().and_then(|_| crate::media::audiofile::load(&path).ok()).filter(|s| !s.is_empty()).map(Arc::new);
    let setup = DecoderSetup { codec: vc.codec, parameter_sets: vc.parameter_sets.clone(), width: vc.width, height: vc.height, output };
    // Make sure it decodes before saying it's ready.
    create_video_decoder(&setup).with_context(|| format!("{} can't be decoded", name(&path)))?;
    Ok(Clip { path, matte, duration, cut, frame_ns, setup, packets, bframes, sound, volume_db: cfg.volume_db })
}

fn name(p: &Path) -> String {
    p.file_name().unwrap_or_default().to_string_lossy().to_string()
}

/// Decodes a stinger: the first frames while it waits, the rest as it plays.
struct Decoding {
    clip: Arc<Clip>,
    dec: Box<dyn VideoDecoder>,
    reorder: Reorder,
    next: usize,
    flushed: bool,
    out: Vec<VideoFrame>,
    serial: u64,
}

impl Decoding {
    fn new(clip: Arc<Clip>, serial: u64) -> Result<Decoding> {
        let dec = create_video_decoder(&clip.setup)?;
        let reorder = Reorder::new(if clip.bframes { 4 } else { 0 });
        Ok(Decoding { clip, dec, reorder, next: 0, flushed: false, out: Vec::new(), serial })
    }

    fn done(&self) -> bool {
        self.flushed
    }

    /// Decode one more packet (or flush at the end): pictures in time order.
    fn step(&mut self) -> Vec<VideoFrame> {
        self.out.clear();
        let mut ready = Vec::new();
        if self.next < self.clip.packets.len() {
            let (pts, data) = &self.clip.packets[self.next];
            self.next += 1;
            let _ = self.dec.decode(data, *pts, &mut self.out);
            for f in self.out.drain(..) {
                if let Some(f) = self.reorder.push(f) {
                    ready.push(f);
                }
            }
        } else if !self.flushed {
            self.flushed = true;
            let _ = self.dec.flush(&mut self.out);
            for f in self.out.drain(..) {
                self.reorder.push_hold(f);
            }
            while let Some(f) = self.reorder.pop() {
                ready.push(f);
            }
        }
        for f in ready.iter_mut() {
            self.serial += 1;
            f.serial = self.serial;
        }
        ready
    }
}

fn worker(shared: Arc<Shared>, canvas: (u32, u32)) {
    let mut loaded_gen = 0u64;
    let mut handled = 0u64;
    let mut serial = 1u64 << 60;
    loop {
        // Load what's wanted.
        let (wanted, gen) = {
            let st = shared.state.lock().unwrap();
            if st.stop {
                return;
            }
            (st.wanted.clone(), st.wanted_gen)
        };
        if gen != loaded_gen {
            loaded_gen = gen;
            {
                let mut st = shared.state.lock().unwrap();
                st.loading = wanted.is_some();
                st.clip = None;
                st.problem = None;
                st.ahead.clear();
            }
            let result = wanted.as_ref().map(|w| load(w, canvas));
            let mut st = shared.state.lock().unwrap();
            st.loading = false;
            match result {
                Some(Ok(c)) => st.clip = Some(Arc::new(c)),
                Some(Err(e)) => st.problem = Some(format!("{e:#}")),
                None => {}
            }
            handled = st.triggers;
            continue;
        }
        let clip = shared.state.lock().unwrap().clip.clone();
        let Some(clip) = clip else {
            let st = shared.state.lock().unwrap();
            if st.wanted_gen == loaded_gen && !st.stop {
                let _ = shared.cond.wait_timeout(st, Duration::from_millis(500));
            }
            continue;
        };

        // Ready the first pictures.
        let mut d = match Decoding::new(clip.clone(), serial) {
            Ok(d) => d,
            Err(e) => {
                let mut st = shared.state.lock().unwrap();
                st.problem = Some(format!("{} can't be decoded: {e:#}", name(&clip.path)));
                st.clip = None;
                continue;
            }
        };
        shared.state.lock().unwrap().ahead.clear();
        let mut ready = 0;
        while ready < PRE && !d.done() {
            let frames = d.step();
            ready += frames.len();
            shared.state.lock().unwrap().ahead.extend(frames);
        }
        serial = d.serial;

        // Wait for a trigger.
        {
            let mut st = shared.state.lock().unwrap();
            while !st.stop && st.wanted_gen == loaded_gen && st.triggers == handled {
                st = shared.cond.wait(st).unwrap();
            }
            if st.stop || st.wanted_gen != loaded_gen {
                continue;
            }
            handled = st.triggers;
        }

        // Play: keep a few pictures ahead of what's showing.
        loop {
            let (stop, changed, retriggered, playhead, waiting, started) = {
                let st = shared.state.lock().unwrap();
                let waiting = st.ahead.iter().filter(|f| f.pts > st.playhead).count();
                (st.stop, st.wanted_gen != loaded_gen, st.triggers != handled, st.playhead, waiting, st.triggered_at)
            };
            if stop || changed {
                break;
            }
            if retriggered {
                // Started again mid-way: from the top (a moment late).
                break;
            }
            let over = playhead >= clip.duration || started.is_some_and(|t| t.elapsed() > Duration::from_nanos(clip.duration as u64) + Duration::from_secs(1));
            if over {
                handled = shared.state.lock().unwrap().triggers;
                break;
            }
            if waiting >= AHEAD || d.done() {
                let st = shared.state.lock().unwrap();
                let _ = shared.cond.wait_timeout(st, Duration::from_millis(5));
                continue;
            }
            let frames = d.step();
            serial = d.serial;
            shared.state.lock().unwrap().ahead.extend(frames);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mattes_from_the_shape() {
        assert_eq!(detect_matte((3840, 1080), (1920, 1080)), Matte::Right);
        assert_eq!(detect_matte((1920, 2160), (1920, 1080)), Matte::Below);
        assert_eq!(detect_matte((1280, 720), (1920, 1080)), Matte::Solid);
    }

    #[test]
    fn plays_a_stinger_from_its_first_frame() {
        let dir = std::env::temp_dir().join(format!("cutaway-stinger-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("sting.mp4");
        // 1 s at 25 fps, twice as wide (picture + matte), with sound.
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=640x180:rate=25:duration=1", "-f", "lavfi", "-i", "sine=duration=1:sample_rate=48000"])
            .args(["-c:v", "libx264", "-profile:v", "baseline", "-pix_fmt", "yuv420p", "-c:a", "aac", "-shortest"])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("ffmpeg isn't available; skipping");
            return;
        }
        let s = StingerPlayer::new((320, 180));
        assert!(s.trigger().is_none(), "nothing loaded");
        s.configure(&StingerConfig { file: Some(p.clone()), cut_ms: 0, matte: Matte::Auto, volume_db: -3.0 });
        assert!(s.wait_loaded(Duration::from_secs(10)), "{:?}", s.info());
        let info = s.info();
        assert_eq!(info.matte, Some(Matte::Right));
        assert!((info.seconds - 1.0).abs() < 0.05, "{}", info.seconds);
        assert!((info.cut_seconds - 0.5).abs() < 0.03);
        assert!(info.has_sound);

        let started = s.trigger().unwrap();
        assert_eq!(started.cut, started.duration / 2);
        assert!(started.sound.as_ref().is_some_and(|x| x.len() > 40_000));
        // The first picture is there at once.
        let first = s.frame_at(0).expect("first frame ready");
        assert_eq!(first.pts, 0);
        assert_eq!((first.width, first.height), (640, 180));
        // Then they follow in time (decoded as it plays).
        let mut seen = vec![first.serial];
        let t0 = Instant::now();
        for k in 1..25 {
            let at = k * 40_000_000;
            let mut f = None;
            while f.is_none() && t0.elapsed() < Duration::from_secs(10) {
                f = s.frame_at(at).filter(|f| f.pts >= at - 20_000_000);
                if f.is_none() {
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
            let f = f.unwrap_or_else(|| panic!("frame {k}"));
            assert!((f.pts - at).abs() < 20_000_000, "frame {k}: {}", f.pts);
            seen.push(f.serial);
        }
        seen.dedup();
        assert_eq!(seen.len(), 25);
        // Afterwards it's ready again from the top.
        s.frame_at(2 * NS_PER_SEC);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) && s.frame_at(0).is_none_or(|f| f.pts != 0) {
            std::thread::sleep(Duration::from_millis(10));
        }
        s.trigger().unwrap();
        assert_eq!(s.frame_at(0).map(|f| f.pts), Some(0), "ready again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
