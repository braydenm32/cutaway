//! Instant replay: the last few seconds of the program (or of one camera,
//! or four cameras at once), played again over the program (optionally in
//! slow motion), fading in and out. The pictures come from the recording's
//! files as they're encoded (so nothing extra is kept), decoded a few
//! frames ahead of what's showing.

use crate::util::LockExt;
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

/// One angle's pictures.
struct Lane {
    packets: Vec<VideoPacket>,
    setup: DecoderSetup,
    reorder: bool,
}

struct Job {
    lanes: Vec<Lane>,
    /// Picture times are measured from here (the same moment for every angle).
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
    /// Each angle's pictures, decoded ahead.
    ahead: Vec<VecDeque<VideoFrame>>,
    playhead: Ns,
    stop: bool,
    problem: Option<String>,
    /// Four angles at once: the last picture put together, and what it was
    /// made from.
    quad: Option<(Vec<u64>, VideoFrame)>,
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

/// Where each of up to four angles goes: the canvas split in four, each
/// picture's size (even), and its top-left corner.
fn tiles(canvas: (u32, u32)) -> ((u32, u32), [(u32, u32); 4]) {
    let (w, h) = ((canvas.0 / 2) & !1, (canvas.1 / 2) & !1);
    ((w, h), [(0, 0), (canvas.0 - w, 0), (0, canvas.1 - h), (canvas.0 - w, canvas.1 - h)])
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
    #[cfg(test)]
    pub fn start(&self, packets: Vec<VideoPacket>, speed: f32) -> Result<Duration> {
        self.start_lanes(vec![packets], speed)
    }

    /// The same for one angle or several (up to four, shown as a grid),
    /// all from the same moment.
    pub fn start_lanes(&self, angles: Vec<Vec<VideoPacket>>, speed: f32) -> Result<Duration> {
        let angles: Vec<Vec<VideoPacket>> = angles.into_iter().filter(|a| !a.is_empty()).take(4).collect();
        if angles.is_empty() {
            return Err(anyhow!("nothing to replay"));
        }
        let output = if angles.len() > 1 { tiles(self.canvas).0 } else { self.canvas };
        let mut lanes = Vec::new();
        // Every angle starts at its keyframe; the replay starts once they all have.
        let mut t0 = Ns::MIN;
        let mut end = Ns::MAX;
        for packets in angles {
            let first = &packets[0];
            let cfg = first.config.clone();
            let start = packets.iter().map(|p| p.pts).min().unwrap_or(first.pts);
            let last = packets.iter().map(|p| p.pts).max().unwrap_or(start);
            let frame = if packets.len() > 1 { (last - start) / (packets.len() as Ns - 1) } else { 33_000_000 };
            t0 = t0.max(start);
            end = end.min(last + frame);
            let reorder = packets.iter().any(|p| p.pts != p.dts);
            lanes.push(Lane { setup: DecoderSetup { codec: cfg.codec, parameter_sets: cfg.parameter_sets.clone(), width: cfg.width, height: cfg.height, output }, packets, reorder });
        }
        let span = (end - t0).max(33_000_000);
        let speed = speed.clamp(0.1, 1.0);
        let mut st = self.shared.st.lock_ok();
        st.ahead = vec![VecDeque::new(); lanes.len()];
        st.job = Some(Arc::new(Job { lanes, t0 }));
        st.gen += 1;
        st.started = Some(Instant::now());
        st.speed = speed;
        st.span = span;
        st.playhead = 0;
        st.problem = None;
        st.quad = None;
        self.shared.cond.notify_all();
        Ok(Duration::from_nanos((span as f64 / speed as f64) as u64))
    }
    /// Stop it now (it fades out quickly).
    pub fn stop(&self) {
        let mut st = self.shared.st.lock_ok();
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
        let st = self.shared.st.lock_ok();
        st.started.is_some_and(|s| s.elapsed() < Duration::from_nanos((st.span as f64 / st.speed.max(0.1) as f64) as u64))
    }

    /// Why the last replay couldn't play.
    pub fn problem(&self) -> Option<String> {
        self.shared.st.lock_ok().problem.clone()
    }

    /// The picture to show now, if a replay is on.
    pub fn frame(&self) -> Option<ReplayFrame> {
        let mut st = self.shared.st.lock_ok();
        let started = st.started?;
        let elapsed = started.elapsed().as_secs_f32();
        let on_air = st.span as f32 / 1e9 / st.speed;
        if elapsed >= on_air {
            st.started = None;
            st.job = None;
            st.ahead.clear();
            st.quad = None;
            self.shared.cond.notify_all();
            return None;
        }
        let playhead = (elapsed * st.speed * 1e9) as Ns;
        st.playhead = playhead;
        for lane in st.ahead.iter_mut() {
            while lane.len() >= 2 && lane[1].pts <= playhead {
                lane.pop_front();
            }
        }
        self.shared.cond.notify_all();
        let now: Vec<Option<VideoFrame>> = st.ahead.iter().map(|l| l.front().filter(|f| f.pts <= playhead + 20_000_000).cloned()).collect();
        let frame = if now.len() == 1 {
            now.into_iter().next().flatten()?
        } else {
            // Four angles at once, put together when one of them moves on.
            now.iter().any(|f| f.is_some()).then_some(())?;
            let serials: Vec<u64> = now.iter().map(|f| f.as_ref().map(|f| f.serial).unwrap_or(0)).collect();
            if st.quad.as_ref().is_none_or(|(s, _)| *s != serials) {
                let f = grid(&now, self.canvas, playhead);
                st.quad = Some((serials, f));
            }
            st.quad.as_ref().unwrap().1.clone()
        };
        let alpha = (elapsed / FADE).min((on_air - elapsed) / FADE).clamp(0.0, 1.0);
        Some(ReplayFrame { frame, alpha })
    }
}

/// Up to four pictures in a grid, on black.
fn grid(pictures: &[Option<VideoFrame>], canvas: (u32, u32), pts: Ns) -> VideoFrame {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SERIAL: AtomicU64 = AtomicU64::new(5 << 60);
    let (w, h) = ((canvas.0 & !1) as usize, (canvas.1 & !1) as usize);
    let mut y = vec![16u8; w * h];
    let mut uv = vec![128u8; w * h / 2];
    let (_, corners) = tiles(canvas);
    for (pic, &(x0, y0)) in pictures.iter().zip(corners.iter()) {
        let Some(pic) = pic else { continue };
        let (pw, ph) = ((pic.width as usize).min(w - x0 as usize), (pic.height as usize).min(h - y0 as usize));
        let (x0, y0) = (x0 as usize & !1, y0 as usize & !1);
        pic.pixels.with_planes(|p| {
            for row in 0..ph {
                let src = &p.y[row * p.y_stride..row * p.y_stride + pw];
                y[(y0 + row) * w + x0..(y0 + row) * w + x0 + pw].copy_from_slice(src);
            }
            for row in 0..ph / 2 {
                let n = pw & !1;
                let src = &p.uv[row * p.uv_stride..row * p.uv_stride + n];
                uv[(y0 / 2 + row) * w + x0..(y0 / 2 + row) * w + x0 + n].copy_from_slice(src);
            }
        });
    }
    VideoFrame::new_nv12(w as u32, h as u32, pts, y, uv, SERIAL.fetch_add(1, Ordering::Relaxed))
}

impl Drop for ReplayPlayer {
    fn drop(&mut self) {
        self.shared.st.lock_ok().stop = true;
        self.shared.cond.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn worker(shared: Arc<Shared>) {
    let mut done_gen = 0u64;
    loop {
        // Wait for a new replay.
        let (job, gen) = {
            let mut st = shared.st.lock_ok();
            loop {
                if st.stop {
                    return;
                }
                if let Some(j) = st.job.clone().filter(|_| st.gen != done_gen) {
                    break (j, st.gen);
                }
                st = shared.cond.wait_timeout(st, Duration::from_millis(200)).unwrap_or_else(|e| e.into_inner()).0;
            }
        };
        done_gen = gen;
        // Each angle decodes on its own thread.
        std::thread::scope(|scope| {
            for (k, lane) in job.lanes.iter().enumerate() {
                let (shared, t0) = (&shared, job.t0);
                scope.spawn(move || decode_lane(shared, gen, k, lane, t0));
            }
        });
    }
}

/// Decode one angle a few pictures ahead of the playhead, until the replay
/// ends or another starts.
fn decode_lane(shared: &Shared, gen: u64, k: usize, lane: &Lane, t0: Ns) {
    let mut serial = (3u64 << 60) + ((k as u64) << 56) + (gen << 32);
    let mut dec: Box<dyn VideoDecoder> = match create_video_decoder(&lane.setup) {
        Ok(d) => d,
        Err(e) => {
            let mut st = shared.st.lock_ok();
            if st.gen == gen {
                st.problem = Some(format!("the replay can't be decoded: {e:#}"));
                st.started = None;
                st.job = None;
            }
            return;
        }
    };
    let mut reorder = Reorder::new(if lane.reorder { 4 } else { 0 });
    let mut out = Vec::new();
    let mut next = 0;
    let mut flushed = false;
    loop {
        {
            let st = shared.st.lock_ok();
            if st.stop || st.gen != gen || st.job.is_none() {
                break;
            }
            let waiting = st.ahead.get(k).map(|a| a.iter().filter(|f| f.pts > st.playhead).count()).unwrap_or(0);
            if waiting >= AHEAD || flushed {
                let _ = shared.cond.wait_timeout(st, Duration::from_millis(5));
                continue;
            }
        }
        let mut ready = Vec::new();
        if next < lane.packets.len() {
            let p = &lane.packets[next];
            next += 1;
            let _ = dec.decode(&p.data, p.pts - t0, &mut out);
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
        let mut st = shared.st.lock_ok();
        if st.gen == gen {
            if let Some(a) = st.ahead.get_mut(k) {
                a.extend(ready);
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
        let mut enc = create_video(&s, Arc::new(move |p| g2.lock_ok().push(p))).ok()?;
        for i in 0..frames {
            let shade = (40 + (i * 200 / frames)) as u8;
            let t = i as Ns * 1_000_000_000 / fps as Ns;
            let f = VideoFrame::new_nv12(w, h, t, vec![shade; (w * h) as usize], vec![128; (w * h / 2) as usize], i as u64);
            enc.encode(&f, t, i == 0).ok()?;
        }
        enc.finish().ok()?;
        let v = got.lock_ok().clone();
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

    #[test]
    fn replays_several_angles_in_a_grid() {
        let (Some(a), Some(b)) = (packets(30, 30), packets(30, 30)) else {
            eprintln!("no encoder here; skipping");
            return;
        };
        let r = ReplayPlayer::new((320, 180));
        let on_air = r.start_lanes(vec![a, b], 1.0).unwrap();
        assert!((on_air.as_secs_f32() - 1.0).abs() < 0.1, "{on_air:?}");
        let mut got = None;
        for _ in 0..100 {
            std::thread::sleep(Duration::from_millis(10));
            if let Some(f) = r.frame() {
                got = Some(f);
                if got.as_ref().unwrap().frame.serial != 0 && got.as_ref().unwrap().alpha > 0.0 {
                    std::thread::sleep(Duration::from_millis(300));
                    got = r.frame();
                    break;
                }
            }
        }
        let f = got.expect("a picture").frame;
        assert_eq!((f.width, f.height), (320, 180), "the whole picture");
        let luma = |x: usize, y: usize| f.pixels.with_planes(|p| p.y[p.y_stride * y + x]);
        // Both angles show (top left and top right); the empty corners are black.
        assert!(luma(80, 45) > 40 && luma(240, 45) > 40, "{} {}", luma(80, 45), luma(240, 45));
        assert!(luma(80, 135) <= 17 && luma(240, 135) <= 17, "{} {}", luma(80, 135), luma(240, 135));
    }
}
