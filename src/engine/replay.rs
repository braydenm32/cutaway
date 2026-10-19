//! Instant replay: the last few seconds of the program, played again over
//! it (optionally in slow motion), fading in and out. The pictures come
//! from the recording's program (already encoded, so nothing extra is
//! kept), decoded a few frames ahead of what's showing.

use super::decode::{create_video_decoder, DecoderSetup, Reorder, VideoDecoder};
use crate::media::{Ns, VideoFrame, VideoPacket};
use anyhow::{anyhow, Result};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Pictures decoded ahead of what's showing.
const AHEAD: usize = 8;
/// Fades in and out.
const FADE: f32 = 0.3;

struct Job {
    packets: Vec<VideoPacket>,
    setup: DecoderSetup,
    reorder: bool,
    /// Picture times are measured from here.
    t0: Ns,
}

#[derive(Default)]
struct State {
    job: Option<Arc<Job>>,
    gen: u64,
    started: Option<Instant>,
    speed: f32,
    /// How much of the recording it shows.
    span: Ns,
    ahead: VecDeque<VideoFrame>,
    playhead: Ns,
    stop: bool,
    problem: Option<String>,
}

struct Shared {
    st: Mutex<State>,
    cond: Condvar,
}

pub struct ReplayPlayer {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
    canvas: (u32, u32),
}

/// What's on screen: a picture and how solid it is (fading in and out).
pub struct ReplayFrame {
    pub frame: VideoFrame,
    pub alpha: f32,
}

impl ReplayPlayer {
    pub fn new(canvas: (u32, u32)) -> ReplayPlayer {
        let shared = Arc::new(Shared { st: Mutex::new(State::default()), cond: Condvar::new() });
        let s2 = shared.clone();
        let thread = std::thread::Builder::new().name("replay".into()).spawn(move || worker(s2)).ok();
        ReplayPlayer { shared, thread, canvas }
    }

    /// Play these pictures again at `speed` (1 = as recorded). Returns how
    /// long it'll be on air.
    pub fn start(&self, packets: Vec<VideoPacket>, speed: f32) -> Result<Duration> {
        let first = packets.first().ok_or_else(|| anyhow!("nothing to replay"))?;
        let cfg = first.config.clone();
        let setup = DecoderSetup { codec: cfg.codec, parameter_sets: cfg.parameter_sets.clone(), width: cfg.width, height: cfg.height, output: self.canvas };
        let t0 = packets.iter().map(|p| p.pts).min().unwrap_or(first.pts);
        let end = packets.iter().map(|p| p.pts).max().unwrap_or(t0);
        let frame = if packets.len() > 1 { (end - t0) / (packets.len() as Ns - 1) } else { 33_000_000 };
        let span = end - t0 + frame;
        let reorder = packets.iter().any(|p| p.pts != p.dts);
        let speed = speed.clamp(0.1, 1.0);
        let mut st = self.shared.st.lock().unwrap();
        st.job = Some(Arc::new(Job { packets, setup, reorder, t0 }));
        st.gen += 1;
        st.started = Some(Instant::now());
        st.speed = speed;
        st.span = span;
        st.ahead.clear();
        st.playhead = 0;
        st.problem = None;
        self.shared.cond.notify_all();
        Ok(Duration::from_nanos((span as f64 / speed as f64) as u64))
    }

    /// Stop it now (it fades out quickly).
    pub fn stop(&self) {
        let mut st = self.shared.st.lock().unwrap();
        if let Some(s) = st.started {
            // Jump to the fade-out.
            let on_air = Duration::from_nanos((st.span as f64 / st.speed as f64) as u64);
            let fade = Duration::from_secs_f32(FADE);
            let elapsed = s.elapsed();
            if elapsed + fade < on_air {
                st.started = Some(Instant::now() + fade - on_air);
            }
        }
    }

    pub fn playing(&self) -> bool {
        let st = self.shared.st.lock().unwrap();
        st.started.is_some_and(|s| s.elapsed() < Duration::from_nanos((st.span as f64 / st.speed.max(0.1) as f64) as u64))
    }

    /// Why the last replay couldn't play.
    pub fn problem(&self) -> Option<String> {
        self.shared.st.lock().unwrap().problem.clone()
    }

    /// The picture to show now, if a replay is on.
    pub fn frame(&self) -> Option<ReplayFrame> {
        let mut st = self.shared.st.lock().unwrap();
        let started = st.started?;
        let elapsed = started.elapsed().as_secs_f32();
        let on_air = st.span as f32 / 1e9 / st.speed;
        if elapsed >= on_air {
            st.started = None;
            st.job = None;
            st.ahead.clear();
            self.shared.cond.notify_all();
            return None;
        }
        let playhead = (elapsed * st.speed * 1e9) as Ns;
        st.playhead = playhead;
        while st.ahead.len() >= 2 && st.ahead[1].pts <= playhead {
            st.ahead.pop_front();
        }
        self.shared.cond.notify_all();
        let frame = st.ahead.front().filter(|f| f.pts <= playhead + 20_000_000).cloned()?;
        let alpha = (elapsed / FADE).min((on_air - elapsed) / FADE).clamp(0.0, 1.0);
        Some(ReplayFrame { frame, alpha })
    }
}

impl Drop for ReplayPlayer {
    fn drop(&mut self) {
        self.shared.st.lock().unwrap().stop = true;
        self.shared.cond.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn worker(shared: Arc<Shared>) {
    let mut serial = 3u64 << 60;
    let mut done_gen = 0u64;
    loop {
        // Wait for a new replay.
        let (job, gen) = {
            let mut st = shared.st.lock().unwrap();
            loop {
                if st.stop {
                    return;
                }
                if let Some(j) = st.job.clone().filter(|_| st.gen != done_gen) {
                    break (j, st.gen);
                }
                st = shared.cond.wait_timeout(st, Duration::from_millis(200)).unwrap().0;
            }
        };
        done_gen = gen;
        let mut dec: Box<dyn VideoDecoder> = match create_video_decoder(&job.setup) {
            Ok(d) => d,
            Err(e) => {
                let mut st = shared.st.lock().unwrap();
                st.problem = Some(format!("the replay can't be decoded: {e:#}"));
                st.started = None;
                st.job = None;
                continue;
            }
        };
        let mut reorder = Reorder::new(if job.reorder { 4 } else { 0 });
        let mut out = Vec::new();
        let mut next = 0;
        let mut flushed = false;
        loop {
            {
                let st = shared.st.lock().unwrap();
                if st.stop || st.gen != gen || st.job.is_none() {
                    break;
                }
                let waiting = st.ahead.iter().filter(|f| f.pts > st.playhead).count();
                if waiting >= AHEAD || flushed {
                    let _ = shared.cond.wait_timeout(st, Duration::from_millis(5));
                    continue;
                }
            }
            let mut ready = Vec::new();
            if next < job.packets.len() {
                let p = &job.packets[next];
                next += 1;
                let _ = dec.decode(&p.data, p.pts - job.t0, &mut out);
                for f in out.drain(..) {
                    if let Some(f) = reorder.push(f) {
                        ready.push(f);
                    }
                }
            } else {
                flushed = true;
                let _ = dec.flush(&mut out);
                for f in out.drain(..) {
                    reorder.push_hold(f);
                }
                while let Some(f) = reorder.pop() {
                    ready.push(f);
                }
            }
            for f in ready.iter_mut() {
                serial += 1;
                f.serial = serial;
            }
            let mut st = shared.st.lock().unwrap();
            if st.gen == gen {
                st.ahead.extend(ready);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::VideoCodec;

    /// Encode a few pictures the way the recorder does.
    fn packets(frames: usize, fps: u32) -> Option<Vec<VideoPacket>> {
        use crate::engine::encode::{create_video, Purpose, VideoSettings};
        let (w, h) = (320u32, 180u32);
        let got: Arc<Mutex<Vec<VideoPacket>>> = Arc::new(Mutex::new(Vec::new()));
        let g2 = got.clone();
        let s = VideoSettings { codec: VideoCodec::H264, width: w, height: h, fps, bitrate_kbps: 2000, purpose: Purpose::Record, backend: "auto".into() };
        let mut enc = create_video(&s, Arc::new(move |p| g2.lock().unwrap().push(p))).ok()?;
        for i in 0..frames {
            let shade = (40 + (i * 200 / frames)) as u8;
            let t = i as Ns * 1_000_000_000 / fps as Ns;
            let f = VideoFrame::new_nv12(w, h, t, vec![shade; (w * h) as usize], vec![128; (w * h / 2) as usize], i as u64);
            enc.encode(&f, t, i == 0).ok()?;
        }
        enc.finish().ok()?;
        let v = got.lock().unwrap().clone();
        Some(v)
    }

    #[test]
    fn replays_in_slow_motion_and_fades() {
        let Some(pk) = packets(30, 30) else {
            eprintln!("no encoder here; skipping");
            return;
        };
        let r = ReplayPlayer::new((320, 180));
        assert!(r.frame().is_none());
        let on_air = r.start(pk, 0.5).unwrap();
        assert!((on_air.as_secs_f32() - 2.0).abs() < 0.1, "1 s at half speed is 2 s: {on_air:?}");
        assert!(r.playing());
        // Fading in at first, then solid, getting brighter (later pictures).
        std::thread::sleep(Duration::from_millis(100));
        let mut early = None;
        for _ in 0..50 {
            if let Some(f) = r.frame() {
                early = Some(f);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let early = early.expect("a picture to show");
        assert!(early.alpha < 1.0, "fading in: {}", early.alpha);
        // Shown every frame, as the compositor does.
        let until = Instant::now() + Duration::from_millis(900);
        while Instant::now() < until {
            let _ = r.frame();
            std::thread::sleep(Duration::from_millis(30));
        }
        let mid = r.frame().expect("still playing");
        assert_eq!(mid.alpha, 1.0);
        let luma = |f: &VideoFrame| f.pixels.with_planes(|p| p.y[p.y_stride * 90 + 160]);
        assert!(luma(&mid.frame) > luma(&early.frame), "later pictures");
        assert!(mid.frame.pts > 400_000_000 && mid.frame.pts < 700_000_000, "half speed: {}", mid.frame.pts);
        r.stop();
        std::thread::sleep(Duration::from_millis(400));
        assert!(r.frame().is_none(), "stopped");
        assert!(!r.playing());
    }
}
