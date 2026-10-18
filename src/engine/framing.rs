//! Auto-framing: a virtual camera cut from a wide (ideally 4K) shot that
//! follows the people in it, moving smoothly like a camera operator.
//!
//! [`Framer`] decides where the frame goes (pure logic, tested here);
//! [`start`] runs it on a source's pictures and delivers the cut-out as a
//! source of its own, so it can be switched, recorded and keyed like any
//! camera.

use super::sources::{Running, SourceSlot};
use crate::config::{FramingConfig, FramingFollow, FramingShot, SourceConfig};
use crate::media::{Ns, VideoFrame, NS_PER_SEC};
use crate::platform::vision::{self, Detection, Detector};
use anyhow::Result;
use crossbeam_channel::{bounded, RecvTimeoutError};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// How often people are looked for.
const DETECT_EVERY: Ns = NS_PER_SEC / 10;
/// Someone not seen for this long has left (covers turning away, blinks
/// of the detector).
const FORGET_AFTER: Ns = 3 * NS_PER_SEC / 2;

/// A head being followed, in source pixels.
#[derive(Clone, Copy, Debug)]
struct Track {
    head: [f32; 4],
    last_seen: Ns,
}

impl Track {
    fn cx(&self) -> f32 {
        self.head[0] + self.head[2] / 2.0
    }
    fn cy(&self) -> f32 {
        self.head[1] + self.head[3] / 2.0
    }
    fn area(&self) -> f32 {
        self.head[2] * self.head[3]
    }
}

/// Where the frame is: centre and height in source pixels (the width
/// follows from the output's shape).
#[derive(Clone, Copy, Debug, PartialEq)]
struct Shot {
    cx: f32,
    cy: f32,
    h: f32,
}

/// Decides where the virtual camera points.
pub struct Framer {
    cfg: FramingConfig,
    src: (f32, f32),
    /// Output width / height.
    aspect: f32,
    out_h: f32,
    tracks: Vec<Track>,
    /// The person followed (for steadiness when following one).
    chosen: Option<(f32, f32)>,
    goal: Option<Shot>,
    pos: Option<Shot>,
    vel: [f32; 3],
    last_t: Option<Ns>,
}

impl Framer {
    pub fn new(cfg: FramingConfig, src: (u32, u32), out: (u32, u32)) -> Self {
        Framer {
            cfg,
            src: (src.0.max(2) as f32, src.1.max(2) as f32),
            aspect: out.0.max(2) as f32 / out.1.max(2) as f32,
            out_h: out.1.max(2) as f32,
            tracks: Vec::new(),
            chosen: None,
            goal: None,
            pos: None,
            vel: [0.0; 3],
            last_t: None,
        }
    }

    pub fn set_config(&mut self, cfg: FramingConfig) {
        if cfg.follow != self.cfg.follow || cfg.shot != self.cfg.shot {
            self.chosen = None;
            self.goal = None; // re-decide without the dead zone
        }
        self.cfg = cfg;
    }

    /// People currently followed.
    pub fn people(&self) -> usize {
        self.tracks.len()
    }

    /// The source picture changed size: start over.
    pub fn set_source_size(&mut self, src: (u32, u32)) {
        let s = (src.0.max(2) as f32, src.1.max(2) as f32);
        if s != self.src {
            self.src = s;
            self.tracks.clear();
            self.goal = None;
            self.pos = None;
        }
    }

    /// Heads found in a picture taken at `t`.
    pub fn observe(&mut self, t: Ns, found: &[Detection]) {
        let heads = heads(found, self.src);
        let mut matched = vec![false; self.tracks.len()];
        for head in heads {
            let (cx, cy) = (head[0] + head[2] / 2.0, head[1] + head[3] / 2.0);
            // The nearest track that isn't taken, if close enough.
            let best = self
                .tracks
                .iter()
                .enumerate()
                .filter(|(i, _)| !matched[*i])
                .map(|(i, tr)| (i, ((tr.cx() - cx).powi(2) + (tr.cy() - cy).powi(2)).sqrt() / tr.head[2].max(head[2]).max(1.0)))
                .filter(|(_, d)| *d < 1.5)
                .min_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((i, _)) => {
                    let tr = &mut self.tracks[i];
                    // Light smoothing of the detector's jitter.
                    for k in 0..4 {
                        tr.head[k] += (head[k] - tr.head[k]) * 0.6;
                    }
                    tr.last_seen = t;
                    matched[i] = true;
                }
                None => {
                    self.tracks.push(Track { head, last_seen: t });
                    matched.push(true);
                }
            }
        }
        self.tracks.retain(|tr| t - tr.last_seen <= FORGET_AFTER);
        self.update_goal();
    }

    /// Where the frame should go for the people followed now.
    fn update_goal(&mut self) {
        let target = match self.choose() {
            Some(subjects) => self.shot_for(&subjects),
            None => self.widest(),
        };
        let target = self.clamp(target);
        let keep = self.goal.is_some_and(|g| {
            let w = g.h * self.aspect;
            // Small moves are ignored so the shot doesn't drift with every nod.
            (target.cx - g.cx).abs() < 0.1 * w && (target.cy - g.cy).abs() < 0.08 * g.h && (target.h / g.h - 1.0).abs() < 0.15
        });
        if !keep {
            self.goal = Some(target);
        }
    }

    fn choose(&mut self) -> Option<Vec<Track>> {
        if self.tracks.is_empty() {
            self.chosen = None;
            return None;
        }
        let one = |tr: &Track| Some(vec![*tr]);
        let pick = match self.cfg.follow {
            FramingFollow::Everyone => return Some(self.tracks.clone()),
            FramingFollow::Left => self.tracks.iter().min_by(|a, b| a.cx().total_cmp(&b.cx())).copied(),
            FramingFollow::Right => self.tracks.iter().max_by(|a, b| a.cx().total_cmp(&b.cx())).copied(),
            FramingFollow::Middle => self.tracks.iter().min_by(|a, b| (a.cx() - self.src.0 / 2.0).abs().total_cmp(&(b.cx() - self.src.0 / 2.0).abs())).copied(),
            FramingFollow::Largest => {
                let biggest = self.tracks.iter().max_by(|a, b| a.area().total_cmp(&b.area())).copied();
                // Stay with the current person unless someone is clearly bigger.
                let current = self.chosen.and_then(|(x, y)| {
                    self.tracks
                        .iter()
                        .filter(|tr| ((tr.cx() - x).powi(2) + (tr.cy() - y).powi(2)).sqrt() < tr.head[2] * 1.5)
                        .min_by(|a, b| ((a.cx() - x).abs() + (a.cy() - y).abs()).total_cmp(&((b.cx() - x).abs() + (b.cy() - y).abs())))
                        .copied()
                });
                match (current, biggest) {
                    (Some(c), Some(b)) if b.area() < c.area() * 1.3 => Some(c),
                    (_, b) => b,
                }
            }
        };
        let pick = pick?;
        self.chosen = Some((pick.cx(), pick.cy()));
        one(&pick)
    }

    /// A shot of these heads at the configured size.
    fn shot_for(&self, heads: &[Track]) -> Shot {
        let fh = heads.iter().map(|t| t.head[3]).sum::<f32>() / heads.len() as f32;
        let (size, headroom) = match self.cfg.shot {
            FramingShot::CloseUp => (3.2, 0.55),
            FramingShot::Medium => (5.0, 0.7),
            FramingShot::Wide => (8.0, 1.0),
        };
        let x0 = heads.iter().map(|t| t.head[0]).fold(f32::MAX, f32::min);
        let x1 = heads.iter().map(|t| t.head[0] + t.head[2]).fold(f32::MIN, f32::max);
        let y0 = heads.iter().map(|t| t.head[1]).fold(f32::MAX, f32::min);
        let y1 = heads.iter().map(|t| t.head[1] + t.head[3]).fold(f32::MIN, f32::max);
        let mut h = size * fh;
        // Everyone in, with a head's width of room at each side…
        let need_w = (x1 - x0) + 2.0 * fh;
        if h * self.aspect < need_w {
            h = need_w / self.aspect;
        }
        // …and below the lowest chin.
        h = h.max((y1 - y0) + headroom * fh + 1.5 * fh);
        let top = y0 - headroom * fh;
        Shot { cx: (x0 + x1) / 2.0, cy: top + h / 2.0, h }
    }

    /// The whole picture (as much of it as fits the output's shape).
    fn widest(&self) -> Shot {
        Shot { cx: self.src.0 / 2.0, cy: self.src.1 / 2.0, h: self.max_h() }
    }

    fn max_h(&self) -> f32 {
        self.src.1.min(self.src.0 / self.aspect)
    }

    /// Keep a shot inside the picture and no tighter than the zoom limit.
    fn clamp(&self, s: Shot) -> Shot {
        let max_h = self.max_h();
        let min_h = (self.out_h / self.cfg.max_zoom.clamp(1.0, 4.0)).max(64.0).min(max_h);
        let h = s.h.clamp(min_h, max_h);
        let w = h * self.aspect;
        Shot { cx: s.cx.clamp(w / 2.0, self.src.0 - w / 2.0), cy: s.cy.clamp(h / 2.0, self.src.1 - h / 2.0), h }
    }

    /// Move towards the goal for a picture at `t`; returns the part of the
    /// source to show (x, y, width, height in pixels).
    pub fn advance(&mut self, t: Ns) -> [f32; 4] {
        let goal = self.goal.unwrap_or_else(|| self.clamp(self.widest()));
        let dt = self.last_t.map(|l| ((t - l) as f32 / 1e9).clamp(0.0, 0.25)).unwrap_or(0.0);
        self.last_t = Some(t);
        let mut pos = match self.pos {
            Some(p) => p,
            None => {
                self.vel = [0.0; 3];
                goal
            }
        };
        // A critically damped spring: eases in and out, never overshoots
        // much, settles in about `reframe_seconds`.
        let omega = 4.75 / self.cfg.reframe_seconds.clamp(0.1, 10.0);
        let steps = ((omega * dt) / 0.25).ceil().max(1.0) as usize;
        let h = dt / steps as f32;
        let target = [goal.cx, goal.cy, goal.h];
        for _ in 0..steps {
            let mut cur = [pos.cx, pos.cy, pos.h];
            for k in 0..3 {
                let acc = omega * omega * (target[k] - cur[k]) - 2.0 * omega * self.vel[k];
                self.vel[k] += acc * h;
                cur[k] += self.vel[k] * h;
            }
            pos = Shot { cx: cur[0], cy: cur[1], h: cur[2] };
        }
        let pos = self.clamp(pos);
        self.pos = Some(pos);
        let w = pos.h * self.aspect;
        [pos.cx - w / 2.0, pos.cy - pos.h / 2.0, w, pos.h]
    }
}

/// Heads (in source pixels) from what the detector found: faces, plus the
/// heads of upper bodies whose face it couldn't see.
fn heads(found: &[Detection], src: (f32, f32)) -> Vec<[f32; 4]> {
    let px = |r: [f32; 4]| [r[0] * src.0, r[1] * src.1, r[2] * src.0, r[3] * src.1];
    let faces: Vec<[f32; 4]> = found.iter().filter(|d| d.face && d.confidence >= 0.4).map(|d| px(d.rect)).collect();
    let mut out = faces.clone();
    for body in found.iter().filter(|d| !d.face && d.confidence >= 0.4).map(|d| px(d.rect)) {
        let has_face = faces.iter().any(|f| {
            let (cx, cy) = (f[0] + f[2] / 2.0, f[1] + f[3] / 2.0);
            cx >= body[0] && cx <= body[0] + body[2] && cy >= body[1] && cy <= body[1] + body[3]
        });
        if !has_face {
            // The head sits at the top middle of an upper body.
            out.push([body[0] + body[2] * 0.3, body[1] + body[3] * 0.04, body[2] * 0.4, body[3] * 0.38]);
        }
    }
    out
}

/// Live settings and state of a running auto-framed source.
#[derive(Default)]
pub struct FramingShared {
    pub cfg: Mutex<FramingConfig>,
    /// People being followed right now.
    pub people: AtomicU64,
    /// Why people aren't being looked for, if they aren't.
    pub problem: Mutex<Option<String>>,
}

impl FramingShared {
    pub fn new(cfg: &FramingConfig) -> Arc<Self> {
        Arc::new(FramingShared { cfg: Mutex::new(cfg.clone()), ..Default::default() })
    }
}

/// Watcher ids for parents' slots (kept clear of the engine's tap ids).
static WATCHER_IDS: AtomicU64 = AtomicU64::new(1 << 48);

struct Framed {
    stop: Arc<AtomicBool>,
    parent: Arc<SourceSlot>,
    watcher: u64,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Framed {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.parent.remove_watcher(self.watcher);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start cutting `cfg`'s picture out of `parent`'s.
pub fn start(cfg: &SourceConfig, parent_name: &str, parent: Arc<SourceSlot>, slot: Arc<SourceSlot>, shared: Arc<FramingShared>) -> Result<(Box<dyn Running>, String)> {
    let (w, h) = (cfg.width, cfg.height);
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = bounded::<VideoFrame>(3);
    let watcher = WATCHER_IDS.fetch_add(1, Ordering::Relaxed);
    parent.add_watcher(watcher, tx);

    // People are looked for on a thread of their own, so a slow detector
    // never holds pictures up.
    let detector: Option<Box<dyn Detector>> = match vision::people_detector() {
        Ok(d) => {
            *shared.problem.lock().unwrap() = None;
            Some(d)
        }
        Err(e) => {
            *shared.problem.lock().unwrap() = Some(format!("{e:#}"));
            None
        }
    };
    let (det_tx, det_rx) = bounded::<VideoFrame>(1);
    let (found_tx, found_rx) = bounded::<(Ns, Vec<Detection>)>(4);
    if let Some(mut detector) = detector {
        let sh = shared.clone();
        std::thread::Builder::new().name(format!("find people {}", slot.index)).spawn(move || {
            let mut failed = false;
            for frame in det_rx.iter() {
                match detector.detect(&frame) {
                    Ok(found) => {
                        if failed {
                            failed = false;
                            *sh.problem.lock().unwrap() = None;
                        }
                        let _ = found_tx.try_send((frame.pts, found));
                    }
                    Err(e) => {
                        failed = true;
                        *sh.problem.lock().unwrap() = Some(format!("{e:#}"));
                    }
                }
            }
        })?;
    }

    let s2 = stop.clone();
    let initial = shared.cfg.lock().unwrap().clone();
    let problem = shared.problem.lock().unwrap().clone();
    let thread = std::thread::Builder::new().name(format!("auto-frame {}", slot.index)).spawn(move || {
        let mut framer: Option<Framer> = None;
        let mut applied = initial;
        let mut last_detect: Option<Ns> = None;
        while !s2.load(Ordering::Relaxed) {
            let mut frame = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            // Behind: skip to the newest picture.
            while let Ok(f) = rx.try_recv() {
                frame = f;
            }
            let fr = framer.get_or_insert_with(|| Framer::new(applied.clone(), (frame.width, frame.height), (w, h)));
            fr.set_source_size((frame.width, frame.height));
            {
                let now = shared.cfg.lock().unwrap();
                if *now != applied {
                    applied = now.clone();
                    fr.set_config(applied.clone());
                }
            }
            if last_detect.is_none_or(|l| frame.pts - l >= DETECT_EVERY) && det_tx.try_send(frame.clone()).is_ok() {
                last_detect = Some(frame.pts);
            }
            while let Ok((t, found)) = found_rx.try_recv() {
                fr.observe(t, &found);
                shared.people.store(fr.people() as u64, Ordering::Relaxed);
            }
            let rect = fr.advance(frame.pts);
            slot.push(frame.crop_scaled(rect, w, h));
        }
    })?;
    let what = match problem {
        Some(p) => format!("whole picture of {parent_name} ({p})"),
        None => format!("auto-framed from {parent_name}"),
    };
    Ok((Box::new(Framed { stop, parent, watcher, thread: Some(thread) }), what))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(x: f32, y: f32, w: f32, h: f32) -> Detection {
        Detection { rect: [x, y, w, h], face: true, confidence: 0.9 }
    }

    const S: Ns = NS_PER_SEC;

    /// Run the framer at 30 fps for `secs`, observing `found` every 100 ms.
    fn run(f: &mut Framer, from: Ns, secs: f32, found: &[Detection]) -> [f32; 4] {
        let mut rect = [0.0; 4];
        let frames = (secs * 30.0) as i64;
        for k in 0..frames {
            let t = from + k * S / 30;
            if k % 3 == 0 {
                f.observe(t, found);
            }
            rect = f.advance(t);
        }
        rect
    }

    fn cfg(follow: FramingFollow, shot: FramingShot) -> FramingConfig {
        FramingConfig { follow, shot, ..Default::default() }
    }

    #[test]
    fn frames_a_close_up_with_headroom() {
        let mut f = Framer::new(cfg(FramingFollow::Everyone, FramingShot::CloseUp), (3840, 2160), (1920, 1080));
        // One face, 240 px tall, centred at (1500, 800).
        let r = run(&mut f, 0, 3.0, &[face(1380.0 / 3840.0, 680.0 / 2160.0, 240.0 / 3840.0, 240.0 / 2160.0)]);
        let (cx, h) = (r[0] + r[2] / 2.0, r[3]);
        assert!((cx - 1500.0).abs() < 10.0, "centred on the face: {r:?}");
        assert!((h - 3.2 * 240.0).abs() < 15.0, "head-and-shoulders height: {r:?}");
        assert!((r[2] / r[3] - 16.0 / 9.0).abs() < 0.01, "output shape: {r:?}");
        // Face in the upper part of the frame, with some room above.
        let face_cy = 800.0 - r[1];
        assert!(face_cy > 0.25 * h && face_cy < 0.42 * h, "eye line: {face_cy} of {h}");
    }

    #[test]
    fn respects_the_zoom_limit_and_the_picture_edges() {
        let mut f = Framer::new(cfg(FramingFollow::Everyone, FramingShot::CloseUp), (1920, 1080), (1920, 1080));
        // A small face near the top left corner.
        let r = run(&mut f, 0, 3.0, &[face(0.01, 0.01, 0.03, 0.05)]);
        assert!(r[3] >= 1080.0 / 1.5 - 1.0, "no more than 1.5x: {r:?}");
        assert!(r[0] >= 0.0 && r[1] >= 0.0 && r[0] + r[2] <= 1920.5 && r[1] + r[3] <= 1080.5, "inside: {r:?}");
    }

    #[test]
    fn follows_one_person_or_everyone() {
        let found = [face(0.25, 0.3, 0.05, 0.09), face(0.7, 0.32, 0.05, 0.09)];
        let mut left = Framer::new(cfg(FramingFollow::Left, FramingShot::CloseUp), (3840, 2160), (1920, 1080));
        let r = run(&mut left, 0, 3.0, &found);
        let cx = r[0] + r[2] / 2.0;
        assert!((cx - 0.275 * 3840.0).abs() < 20.0, "left person: {r:?}");
        let mut right = Framer::new(cfg(FramingFollow::Right, FramingShot::CloseUp), (3840, 2160), (1920, 1080));
        let r = run(&mut right, 0, 3.0, &found);
        assert!((r[0] + r[2] / 2.0 - 0.725 * 3840.0).abs() < 20.0, "right person: {r:?}");
        let mut all = Framer::new(cfg(FramingFollow::Everyone, FramingShot::CloseUp), (3840, 2160), (1920, 1080));
        let r = run(&mut all, 0, 3.0, &found);
        assert!(r[0] < 0.25 * 3840.0 && r[0] + r[2] > 0.75 * 3840.0, "both in: {r:?}");
    }

    #[test]
    fn moves_smoothly_and_goes_wide_when_everyone_leaves() {
        let mut f = Framer::new(cfg(FramingFollow::Everyone, FramingShot::CloseUp), (3840, 2160), (1920, 1080));
        let a = [face(0.2, 0.3, 0.05, 0.09)];
        run(&mut f, 0, 3.0, &a);
        // The person walks to the other side: the frame glides, no jumps.
        let b = [face(0.7, 0.3, 0.05, 0.09)];
        let mut last = f.advance(3 * S);
        let mut biggest_step = 0.0f32;
        // (The first person is remembered for a moment, in case they were
        // only hidden.)
        for k in 1..120 {
            let t = 3 * S + k * S / 30;
            if k % 3 == 0 {
                f.observe(t, &b);
            }
            let r = f.advance(t);
            biggest_step = biggest_step.max((r[0] - last[0]).abs());
            last = r;
        }
        let travel = (0.7 - 0.2) * 3840.0;
        assert!(biggest_step < travel / 8.0, "glides ({biggest_step} px in one frame)");
        assert!((last[0] + last[2] / 2.0 - 0.725 * 3840.0).abs() < 40.0, "arrives: {last:?}");
        // Nobody there for a while: back to the whole picture.
        let r = run(&mut f, 7 * S, 4.0, &[]);
        assert!((r[3] - 2160.0).abs() < 2.0 && r[0].abs() < 2.0, "wide: {r:?}");
    }

    #[test]
    fn ignores_small_moves() {
        let mut f = Framer::new(cfg(FramingFollow::Everyone, FramingShot::CloseUp), (3840, 2160), (1920, 1080));
        let r1 = run(&mut f, 0, 3.0, &[face(0.4, 0.3, 0.05, 0.09)]);
        let r2 = run(&mut f, 3 * S, 2.0, &[face(0.402, 0.302, 0.05, 0.09)]);
        assert!((r1[0] - r2[0]).abs() < 0.5 && (r1[1] - r2[1]).abs() < 0.5, "steady: {r1:?} {r2:?}");
    }

    #[test]
    fn upper_bodies_stand_in_for_hidden_faces() {
        let body = Detection { rect: [0.5, 0.2, 0.2, 0.5], face: false, confidence: 0.8 };
        let h = heads(&[body], (1000.0, 1000.0));
        assert_eq!(h.len(), 1);
        assert!((h[0][0] + h[0][2] / 2.0 - 600.0).abs() < 1.0);
        // A body with its face found is the same person.
        let h = heads(&[body, face(0.57, 0.22, 0.06, 0.08)], (1000.0, 1000.0));
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn crop_scaled_cuts_the_right_part() {
        // Left half dark, right half bright.
        let (w, h) = (64usize, 32usize);
        let y: Vec<u8> = (0..w * h).map(|i| if i % w < w / 2 { 30 } else { 200 }).collect();
        let frame = VideoFrame::new_nv12(w as u32, h as u32, 0, y, vec![128; w * h / 2], 0);
        let right = frame.crop_scaled([32.0, 0.0, 32.0, 32.0], 16, 16);
        let left = frame.crop_scaled([0.0, 0.0, 32.0, 32.0], 16, 16);
        right.pixels.with_planes(|p| assert!(p.y.iter().all(|&v| v == 200)));
        left.pixels.with_planes(|p| assert!(p.y.iter().all(|&v| v == 30)));
        // Large pictures (several threads) match small ones.
        let big = VideoFrame::new_nv12(1280, 720, 0, (0..1280 * 720).map(|i| (i % 1280 / 5) as u8).collect(), vec![128; 1280 * 360], 0);
        let out = big.crop_scaled([640.0, 0.0, 640.0, 360.0], 1280, 720);
        out.pixels.with_planes(|p| {
            assert_eq!(p.y[0], 128);
            assert_eq!(p.y[719 * 1280 + 1279], 255);
        });
    }
}

#[cfg(test)]
mod people_tests {
    use crate::media::testsrc::TestPattern;
    use crate::media::VideoFrame;
    use crate::platform::vision::{Detector, TestDetector};

    #[test]
    fn test_detector_finds_the_test_people() {
        let mut tp = TestPattern::new("people", 1280, 720);
        let img = tp.render(1_000_000_000, 30);
        let frame = VideoFrame::new_nv12(1280, 720, 0, img.y, img.uv, 0);
        let found = TestDetector.detect(&frame).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
    }
}
