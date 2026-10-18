//! The media player source: plays a playlist of video files and images
//! (intros, outros, stings, holding slides, music beds) as a source, with
//! its sound going to the mixer.

use super::decode::{create_aac_decoder, create_video_decoder, DecoderSetup, Reorder, ToMix};
use super::feed::AudioHub;
use super::sources::{Running, SourceSlot};
use crate::config::MediaConfig;
use crate::media::demux::{Mp4Reader, TrackKind};
use crate::media::{clock, AudioBlock, Ns, VideoFrame, NS_PER_SEC};
use anyhow::{bail, Context, Result};
use crossbeam_channel::{unbounded, Receiver, RecvTimeoutError, Sender};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum PlayerCmd {
    Play,
    Pause,
    TogglePause,
    /// From the start of the current item.
    Restart,
    Next,
    Previous,
    Select(usize),
    Playlist(MediaConfig),
}

/// What the UI shows about a player.
#[derive(Debug, Clone, Default)]
pub struct PlayerState {
    pub playing: bool,
    pub index: usize,
    pub items: Vec<String>,
    pub position: Ns,
    pub duration: Ns,
    pub ended: bool,
    pub error: Option<String>,
}

pub struct PlayerControl {
    tx: Sender<PlayerCmd>,
    pub state: Mutex<PlayerState>,
}

impl PlayerControl {
    pub fn send(&self, c: PlayerCmd) {
        let _ = self.tx.send(c);
    }
}

struct PlayerRunning {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for PlayerRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn is_image(p: &Path) -> bool {
    p.extension().is_some_and(|e| ["png", "jpg", "jpeg"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// An RGBA picture as an NV12 frame of `size` (letterboxed in black).
pub fn image_frame(img: &image::RgbaImage, size: (u32, u32), pts: Ns) -> VideoFrame {
    let (w, h) = (size.0.max(2) & !1, size.1.max(2) & !1);
    let fitted = {
        let (iw, ih) = (img.width().max(1) as f32, img.height().max(1) as f32);
        let k = (w as f32 / iw).min(h as f32 / ih);
        let (fw, fh) = (((iw * k) as u32).max(1), ((ih * k) as u32).max(1));
        image::imageops::resize(img, fw, fh, image::imageops::FilterType::Triangle)
    };
    let (ox, oy) = ((w - fitted.width().min(w)) / 2, (h - fitted.height().min(h)) / 2);
    let mut y = vec![16u8; (w * h) as usize];
    let mut uv = vec![128u8; (w * h / 2) as usize];
    let rgb_at = |x: u32, yy: u32| -> (f32, f32, f32) {
        if x < ox || yy < oy || x >= ox + fitted.width() || yy >= oy + fitted.height() {
            return (0.0, 0.0, 0.0);
        }
        let p = fitted.get_pixel(x - ox, yy - oy);
        let a = p[3] as f32 / 255.0;
        (p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a)
    };
    for yy in 0..h {
        for x in 0..w {
            let (r, g, b) = rgb_at(x, yy);
            y[(yy * w + x) as usize] = (16.0 + 219.0 * (0.2126 * r + 0.7152 * g + 0.0722 * b)).round() as u8;
        }
    }
    for yy in 0..h / 2 {
        for x in 0..w / 2 {
            let (r, g, b) = rgb_at(x * 2, yy * 2);
            uv[(yy * w + x * 2) as usize] = (128.0 + 224.0 * (-0.1146 * r - 0.3854 * g + 0.5 * b)).round() as u8;
            uv[(yy * w + x * 2 + 1) as usize] = (128.0 + 224.0 * (0.5 * r - 0.4542 * g - 0.0458 * b)).round() as u8;
        }
    }
    VideoFrame::new_nv12(w, h, pts, y, uv, 0)
}

/// What playing one item ended with.
enum Outcome {
    Finished,
    Goto(usize),
    NewPlaylist(MediaConfig),
    Stop,
}

struct Player {
    name: String,
    slot: Arc<SourceSlot>,
    hub: Arc<AudioHub>,
    size: (u32, u32),
    cfg: MediaConfig,
    rx: Receiver<PlayerCmd>,
    control: Arc<PlayerControl>,
    stop: Arc<AtomicBool>,
    playing: bool,
    last: Option<VideoFrame>,
}

impl Player {
    fn set_state(&self, f: impl FnOnce(&mut PlayerState)) {
        f(&mut self.control.state.lock().unwrap());
    }

    /// Keep showing the last picture (so the source stays "live").
    fn hold(&mut self) {
        let f = match &self.last {
            Some(f) => f.clone(),
            None => image_frame(&image::RgbaImage::new(2, 2), self.size, 0),
        };
        let mut f = f.detached();
        f.pts = clock::now();
        self.last = Some(f.clone());
        self.slot.push(f);
    }

    /// Wait up to `d` for a command; handles play/pause itself.
    fn wait(&mut self, d: Duration) -> Option<Outcome> {
        match self.rx.recv_timeout(d) {
            Ok(c) => self.command(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Outcome::Stop),
        }
    }

    fn command(&mut self, c: PlayerCmd) -> Option<Outcome> {
        let idx = self.control.state.lock().unwrap().index;
        let n = self.cfg.files.len().max(1);
        match c {
            PlayerCmd::Play => self.playing = true,
            PlayerCmd::Pause => self.playing = false,
            PlayerCmd::TogglePause => self.playing = !self.playing,
            PlayerCmd::Restart => {
                self.playing = true;
                return Some(Outcome::Goto(idx));
            }
            PlayerCmd::Next => return Some(Outcome::Goto((idx + 1) % n)),
            PlayerCmd::Previous => return Some(Outcome::Goto((idx + n - 1) % n)),
            PlayerCmd::Select(i) => {
                self.playing = true;
                return Some(Outcome::Goto(i.min(n - 1)));
            }
            PlayerCmd::Playlist(cfg) => return Some(Outcome::NewPlaylist(cfg)),
        }
        let playing = self.playing;
        self.set_state(|s| s.playing = playing);
        None
    }

    fn run(mut self) {
        let mut index = 0usize;
        self.playing = self.cfg.autoplay;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let items: Vec<String> = self.cfg.files.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string()).collect();
            let playing = self.playing;
            self.set_state(|s| {
                s.items = items;
                s.index = index;
                s.playing = playing;
                s.ended = false;
            });
            let outcome = if self.cfg.files.is_empty() {
                self.set_state(|s| s.error = Some("the playlist is empty".into()));
                self.idle()
            } else {
                let path = self.cfg.files[index.min(self.cfg.files.len() - 1)].clone();
                let r = if is_image(&path) { self.play_image(&path) } else { self.play_file(&path) };
                match r {
                    Ok(o) => {
                        self.set_state(|s| s.error = None);
                        o
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        self.set_state(|s| s.error = Some(msg));
                        // Move on after a moment rather than stopping the show.
                        match self.wait(Duration::from_secs(2)) {
                            Some(o) => o,
                            None => Outcome::Finished,
                        }
                    }
                }
            };
            match outcome {
                Outcome::Stop => return,
                Outcome::Goto(i) => index = i,
                Outcome::NewPlaylist(cfg) => {
                    self.cfg = cfg;
                    index = 0;
                }
                Outcome::Finished => {
                    if index + 1 < self.cfg.files.len() {
                        index += 1;
                    } else if self.cfg.loop_playlist {
                        index = 0;
                    } else {
                        // End of the playlist: hold the last picture.
                        self.playing = false;
                        self.set_state(|s| {
                            s.ended = true;
                            s.playing = false;
                        });
                        match self.idle() {
                            Outcome::Stop => return,
                            Outcome::Goto(i) => index = i,
                            Outcome::NewPlaylist(cfg) => {
                                self.cfg = cfg;
                                index = 0;
                            }
                            Outcome::Finished => index = 0,
                        }
                    }
                }
            }
        }
    }

    /// Hold the picture until a command says otherwise.
    fn idle(&mut self) -> Outcome {
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Outcome::Stop;
            }
            self.hold();
            if let Some(o) = self.wait(Duration::from_millis(100)) {
                return o;
            }
            if self.playing && !self.cfg.files.is_empty() {
                return Outcome::Finished;
            }
        }
    }

    fn play_image(&mut self, path: &Path) -> Result<Outcome> {
        let img = image::open(path).with_context(|| format!("{}", path.display()))?.to_rgba8();
        self.last = Some(image_frame(&img, self.size, clock::now()));
        let len = (self.cfg.image_seconds.max(0.5) * 1e9) as Ns;
        // A single image with nothing else to show stays up.
        let forever = self.cfg.files.len() == 1;
        let mut shown: Ns = 0;
        self.set_state(|s| {
            s.duration = if forever { 0 } else { len };
            s.position = 0;
        });
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(Outcome::Stop);
            }
            self.hold();
            if let Some(o) = self.wait(Duration::from_millis(100)) {
                return Ok(o);
            }
            if self.playing {
                shown += 100_000_000;
                self.set_state(|s| s.position = shown);
                if !forever && shown >= len {
                    return Ok(Outcome::Finished);
                }
            }
        }
    }

    fn play_file(&mut self, path: &Path) -> Result<Outcome> {
        let mut r = Mp4Reader::open(path)?;
        let vi = r.video();
        let ai = r.audio();
        if vi.is_none() && ai.is_none() {
            bail!("{}: nothing playable", path.display());
        }
        let mut vdec = match vi.map(|i| r.tracks[i].kind.clone()) {
            Some(TrackKind::Video(c)) => Some(create_video_decoder(&DecoderSetup { codec: c.codec, parameter_sets: c.parameter_sets.clone(), width: c.width, height: c.height, output: self.size })?),
            _ => None,
        };
        let mut adec = match ai.map(|i| r.tracks[i].kind.clone()) {
            Some(TrackKind::Audio(c)) => create_aac_decoder(&c.asc).ok().map(|d| {
                let m = ToMix::new(d.rate(), d.channels());
                (d, m)
            }),
            _ => None,
        };
        let duration = r.duration();
        self.set_state(|s| {
            s.duration = duration;
            s.position = 0;
        });
        let has_b = vi.is_some_and(|i| r.tracks[i].samples.iter().take(30).any(|s| s.cts != 0));
        let mut reorder = Reorder::new(if has_b { 4 } else { 0 });
        let (mut v_next, mut a_next) = (0usize, 0usize);
        let v_count = vi.map(|i| r.tracks[i].samples.len()).unwrap_or(0);
        let a_count = ai.map(|i| r.tracks[i].samples.len()).unwrap_or(0);
        let mut start = clock::now() + 100_000_000; // host time of media time 0
        let mut audio_out: i64 = 0; // samples sent at 48 kHz
        let mut decoded = Vec::new();
        let mut video_done = vi.is_none();
        let mut audio_done = adec.is_none();
        let mut paused_at: Option<Ns> = None;
        let mut bad_frames = 0u32;
        loop {
            if self.stop.load(Ordering::Relaxed) {
                return Ok(Outcome::Stop);
            }
            while let Ok(c) = self.rx.try_recv() {
                if let Some(o) = self.command(c) {
                    return Ok(o);
                }
            }
            if !self.playing {
                // Paused: hold the picture; the clock stops.
                if paused_at.is_none() {
                    paused_at = Some(clock::now());
                }
                self.hold();
                if let Some(o) = self.wait(Duration::from_millis(100)) {
                    return Ok(o);
                }
                continue;
            }
            if let Some(p) = paused_at.take() {
                start += clock::now() - p;
            }
            let now = clock::now();
            // Audio runs ahead (the mixer lines it up by time).
            while !audio_done && (start + audio_out * NS_PER_SEC / 48_000) < now + 400_000_000 {
                if a_next >= a_count {
                    audio_done = true;
                    break;
                }
                let ai = ai.unwrap();
                let data = r.read(ai, a_next)?;
                let pts = r.tracks[ai].pts(a_next);
                a_next += 1;
                let (d, m) = adec.as_mut().unwrap();
                if let Ok(pcm) = d.decode(&data) {
                    // Time of this output's first sample: the packet's time
                    // minus the decoder's own delay. Anything before the
                    // start (encoder priming) is dropped.
                    let ch = d.channels().max(1) as usize;
                    let rate = d.rate().max(1) as i64;
                    let first = pts - d.delay() as i64 * NS_PER_SEC / rate;
                    let skip = if first < 0 { ((-first) as i128 * rate as i128 / NS_PER_SEC as i128) as usize } else { 0 };
                    if skip * ch >= pcm.len() {
                        continue;
                    }
                    if !m.matches(d.rate(), d.channels()) {
                        *m = ToMix::new(d.rate(), d.channels());
                    }
                    let out = m.push(&pcm[skip * ch..]);
                    if !out.is_empty() {
                        let at = start + audio_out * NS_PER_SEC / 48_000;
                        audio_out += (out.len() / 2) as i64;
                        self.hub.send(&self.name, AudioBlock { pts: at, rate: 48_000, channels: 2, samples: out });
                    }
                }
            }
            // Video: decode until a picture is ready, then show it on time.
            if !video_done {
                let vi = vi.unwrap();
                let mut next_frame = reorder.pop_ready();
                while next_frame.is_none() && v_next < v_count {
                    let data = r.read(vi, v_next)?;
                    let pts = r.tracks[vi].pts(v_next);
                    v_next += 1;
                    decoded.clear();
                    // A damaged frame is skipped, not the whole file.
                    if let Err(e) = vdec.as_mut().unwrap().decode(&data, pts, &mut decoded) {
                        bad_frames += 1;
                        if bad_frames > 30 {
                            return Err(e.context(format!("{}: can't decode the video", path.display())));
                        }
                        continue;
                    }
                    bad_frames = 0;
                    for f in decoded.drain(..) {
                        if let Some(out) = reorder.push(f) {
                            next_frame = Some(out);
                        }
                    }
                }
                if next_frame.is_none() && v_next >= v_count {
                    vdec.as_mut().unwrap().flush(&mut decoded)?;
                    for f in decoded.drain(..) {
                        reorder.push_hold(f);
                    }
                    next_frame = reorder.pop();
                    if next_frame.is_none() {
                        video_done = true;
                    }
                }
                if let Some(mut f) = next_frame {
                    let media_t = f.pts;
                    f.pts = start + media_t;
                    if (f.width, f.height) != self.size {
                        f = f.scaled(self.size.0, self.size.1);
                    }
                    // Wait for its time, still answering commands.
                    loop {
                        let wait = f.pts - clock::now();
                        if wait <= 0 {
                            break;
                        }
                        if let Some(o) = self.wait(Duration::from_nanos(wait.min(20_000_000) as u64)) {
                            return Ok(o);
                        }
                        if !self.playing {
                            break;
                        }
                    }
                    if !self.playing {
                        // Paused while waiting: show this one when resumed.
                        reorder.push_hold(VideoFrame { pts: media_t, ..f });
                        continue;
                    }
                    self.set_state(|s| s.position = media_t);
                    self.last = Some(f.clone());
                    self.slot.push(f);
                }
            } else {
                // Audio-only file (or video ended first): keep the picture up.
                if audio_done {
                    return Ok(Outcome::Finished);
                }
                self.hold();
                let pos = clock::now() - start;
                self.set_state(|s| s.position = pos);
                if let Some(o) = self.wait(Duration::from_millis(40)) {
                    return Ok(o);
                }
            }
            if video_done && audio_done {
                return Ok(Outcome::Finished);
            }
        }
    }
}

pub fn start(name: &str, cfg: &MediaConfig, size: (u32, u32), slot: Arc<SourceSlot>, hub: Arc<AudioHub>) -> Result<(Box<dyn Running>, Arc<PlayerControl>, String)> {
    let (tx, rx) = unbounded();
    let control = Arc::new(PlayerControl { tx, state: Mutex::new(PlayerState::default()) });
    let stop = Arc::new(AtomicBool::new(false));
    let p = Player { name: name.to_string(), slot, hub, size, cfg: cfg.clone(), rx, control: control.clone(), stop: stop.clone(), playing: cfg.autoplay, last: None };
    let thread = std::thread::Builder::new().name(format!("player {name}")).spawn(move || p.run())?;
    let n = cfg.files.len();
    Ok((Box::new(PlayerRunning { stop, thread: Some(thread) }), control, format!("media player ({n} item{})", if n == 1 { "" } else { "s" })))
}

/// Files in a folder, sorted, for "add a folder" in the UI.
pub fn playable_in(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| ["mp4", "mov", "m4v", "png", "jpg", "jpeg"].iter().any(|x| e.eq_ignore_ascii_case(x))))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;

    #[test]
    fn plays_a_file_in_time() {
        let dir = std::env::temp_dir().join(format!("cutaway-player-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("clip.mp4");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=1", "-f", "lavfi", "-i", "sine=duration=1:sample_rate=48000"])
            .args(["-c:v", "libx264", "-profile:v", "baseline", "-c:a", "aac"])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
        let slot = SourceSlot::new(0, 0, 50_000_000);
        let hub = AudioHub::new();
        let (atx, arx) = crossbeam_channel::unbounded();
        hub.register("Media", atx);
        let cfg = MediaConfig { files: vec![p.clone()], loop_playlist: false, ..Default::default() };
        let started = clock::now();
        let (running, control, _) = start("Media", &cfg, (160, 120), slot.clone(), hub.clone()).unwrap();
        std::thread::sleep(Duration::from_millis(1600));
        let st = control.state.lock().unwrap().clone();
        assert!(st.ended, "{st:?}");
        // ~25 frames arrived, at roughly real-time spacing.
        let frames = slot.frames();
        assert!((24..=40).contains(&frames), "{frames}");
        let blocks: Vec<AudioBlock> = arx.try_iter().collect();
        let samples: usize = blocks.iter().map(|b| b.frames()).sum();
        assert!(samples > 40_000 && samples < 52_000, "{samples}");
        assert!(blocks[0].pts >= started && blocks[0].pts < started + 300_000_000);
        drop(running);
        std::fs::remove_dir_all(&dir).ok();
    }
}
