//! Which scene is live, what's in preview, and the transition between
//! scenes. The compositor asks for the frame layout every time it renders.

use crate::util::LockExt;
use crate::config::{parse_color, Direction, TransitionMode};
use crate::media::{clock, Ns};
use crate::scene::{compose, PadState, Scene};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct SwitchEvent {
    /// Host time of the switch.
    pub at: Ns,
    pub scene: String,
    pub mode: TransitionMode,
    pub duration_ms: u32,
    /// The single source shown full frame, if the scene is one: its
    /// identity (`SourceConfig::uid`), which stays put when sources are
    /// added, removed or moved during a take.
    pub angle: Option<usize>,
}

/// What the compositor draws for the program.
pub enum Layout {
    /// One arrangement of sources (including mid-mix glides).
    One(Vec<PadState>),
    /// Two whole scenes combined by a wipe, push or dip.
    Two { from: Vec<PadState>, to: Vec<PadState>, mode: TransitionMode, t: f32, direction: [f32; 2], dip: [f32; 3] },
    /// A stinger playing over a scene: `elapsed` since it started.
    Stinger { states: Vec<PadState>, elapsed: Ns },
}

struct Active {
    from: Scene,
    to: Scene,
    mode: TransitionMode,
    start: Instant,
    dur: Duration,
    /// Stingers: when the scenes change under it.
    cut: Duration,
}

struct State {
    current: Scene,
    active: Option<Active>,
    preview: Option<Scene>,
    /// The scene on air before the one now ("back to the last scene").
    previous: Option<Scene>,
    aspects: Vec<f32>,
    direction: Direction,
    dip: [f32; 3],
}

impl State {
    /// About to take `next`: what's on now becomes the last scene (taking
    /// the same scene again doesn't).
    fn remember(&mut self, next: &Scene) {
        if self.current.name != next.name {
            self.previous = Some(self.current.clone());
        }
    }
}

/// Fade to black: from one level to another over a time.
#[derive(Clone, Copy)]
struct Fade {
    from: f32,
    to: f32,
    start: Instant,
    dur: Duration,
}

impl Fade {
    fn level(&self) -> f32 {
        if self.dur.is_zero() {
            return self.to;
        }
        let t = Instant::now().saturating_duration_since(self.start).as_secs_f32() / self.dur.as_secs_f32().max(1e-3);
        self.from + (self.to - self.from) * ease(t)
    }
}

pub struct Switcher {
    canvas: (u32, u32),
    state: Mutex<State>,
    /// Each source's identity, by number (for the switch log's angles).
    uids: Mutex<Vec<u64>>,
    events: Mutex<Vec<SwitchEvent>>,
    black: Mutex<Fade>,
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Switcher {
    pub fn new(canvas: (u32, u32), aspects: Vec<f32>, initial: Scene) -> Self {
        Switcher {
            canvas,
            state: Mutex::new(State { current: initial, active: None, preview: None, previous: None, aspects, direction: Direction::Right, dip: [0.0; 3] }),
            uids: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            black: Mutex::new(Fade { from: 0.0, to: 0.0, start: Instant::now(), dur: Duration::ZERO }),
        }
    }

    /// Fade the program to black (or back up) over `ms`.
    pub fn fade_to_black(&self, on: bool, ms: u32) {
        let mut f = self.black.lock_ok();
        let from = f.level();
        *f = Fade { from, to: if on { 1.0 } else { 0.0 }, start: Instant::now(), dur: Duration::from_millis(ms as u64) };
    }

    /// How black the program is now (0..1).
    pub fn black_level(&self) -> f32 {
        self.black.lock_ok().level()
    }

    /// Faded (or fading) to black.
    pub fn is_black(&self) -> bool {
        self.black.lock_ok().to > 0.5
    }

    /// Direction for wipes/pushes and the colour for dips.
    pub fn set_style(&self, direction: Direction, dip_color: &str) {
        let c = parse_color(dip_color).unwrap_or(0xff000000);
        let mut st = self.state.lock_ok();
        st.direction = direction;
        st.dip = [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0];
    }

    /// Source shapes changed (cropping).
    pub fn set_aspects(&self, aspects: Vec<f32>) {
        self.state.lock_ok().aspects = aspects;
    }

    /// Each source's identity, by number.
    pub fn set_uids(&self, uids: Vec<u64>) {
        *self.uids.lock_ok() = uids;
    }

    /// How many sources there are.
    pub fn source_count(&self) -> usize {
        self.state.lock_ok().aspects.len()
    }

    /// The identity of the source a scene shows full frame, if it does.
    fn angle_of(&self, scene: &Scene) -> Option<usize> {
        let i = scene.full_frame_source()?;
        Some(self.uids.lock_ok().get(i).map(|u| *u as usize).unwrap_or(i))
    }

    /// The identity of the source on air full frame now, if one is.
    pub fn angle_now(&self) -> Option<usize> {
        self.angle_of(&self.live())
    }

    /// Sources were added, removed or moved: `from[new] = Some(old)` for
    /// those kept. What's live, mid-transition and in preview follows them;
    /// frames of sources that went are dropped.
    pub fn remap_sources(&self, from: &[Option<usize>], aspects: Vec<f32>, uids: Vec<u64>) {
        let new_of = |old: usize| from.iter().position(|f| *f == Some(old));
        let mut st = self.state.lock_ok();
        st.current.remap_sources(&new_of);
        if let Some(a) = &mut st.active {
            a.from.remap_sources(&new_of);
            a.to.remap_sources(&new_of);
        }
        if let Some(p) = &mut st.preview {
            p.remap_sources(&new_of);
        }
        st.aspects = aspects;
        drop(st);
        *self.uids.lock_ok() = uids;
    }

    /// Go live with `scene`.
    pub fn take(&self, scene: Scene, mode: TransitionMode, duration_ms: u32) {
        // A stinger needs its video (`take_stinger`).
        let mode = if mode == TransitionMode::Stinger { TransitionMode::Mix } else { mode };
        let mut st = self.state.lock_ok();
        if let Some(a) = st.active.take() {
            st.current = a.to;
        }
        st.remember(&scene);
        let angle = self.angle_of(&scene);
        self.events.lock_ok().push(SwitchEvent { at: clock::now(), scene: scene.name.clone(), mode, duration_ms, angle });
        if mode == TransitionMode::Cut || duration_ms == 0 || scene == st.current {
            st.current = scene;
        } else {
            let from = st.current.clone();
            st.active = Some(Active { from, to: scene, mode, start: Instant::now(), dur: Duration::from_millis(duration_ms as u64), cut: Duration::ZERO });
        }
    }

    /// Go live with `scene` under a stinger `duration` long that starts
    /// `delay` from now, changing scenes `cut` into it.
    pub fn take_stinger(&self, scene: Scene, duration: Ns, cut: Ns, delay: Ns) {
        let mut st = self.state.lock_ok();
        if let Some(a) = st.active.take() {
            st.current = a.to;
        }
        st.remember(&scene);
        let duration_ms = (duration / 1_000_000).max(1) as u32;
        let angle = self.angle_of(&scene);
        self.events.lock_ok().push(SwitchEvent { at: clock::now() + delay + cut, scene: scene.name.clone(), mode: TransitionMode::Stinger, duration_ms, angle });
        let from = st.current.clone();
        let ns = |n: Ns| Duration::from_nanos(n.max(0) as u64);
        st.active = Some(Active { from, to: scene, mode: TransitionMode::Stinger, start: Instant::now() + ns(delay), dur: ns(duration.max(1)), cut: ns(cut) });
    }

    /// Change the live layout immediately (dragging frames around).
    pub fn set_live(&self, scene: Scene) {
        let mut st = self.state.lock_ok();
        st.active = None;
        st.current = scene;
    }

    /// Put a scene in preview (None clears it).
    pub fn set_preview(&self, scene: Option<Scene>) {
        self.state.lock_ok().preview = scene;
    }

    /// The scene on air before this one, if there's been a switch.
    pub fn previous(&self) -> Option<Scene> {
        self.state.lock_ok().previous.clone()
    }

    pub fn preview(&self) -> Option<Scene> {
        self.state.lock_ok().preview.clone()
    }

    /// Layout of the preview scene, if there is one.
    pub fn preview_states(&self) -> Option<Vec<PadState>> {
        let st = self.state.lock_ok();
        st.preview.as_ref().map(|p| compose(None, p, 1.0, self.canvas, &st.aspects))
    }

    /// What to draw now.
    pub fn layout(&self) -> Layout {
        let mut st = self.state.lock_ok();
        let Some(a) = &st.active else {
            return Layout::One(compose(None, &st.current, 1.0, self.canvas, &st.aspects));
        };
        let elapsed = Instant::now().saturating_duration_since(a.start);
        let t = (elapsed.as_secs_f32() / a.dur.as_secs_f32().max(1e-3)).min(1.0);
        let out = match a.mode {
            TransitionMode::Stinger => {
                let scene = if elapsed < a.cut { &a.from } else { &a.to };
                Layout::Stinger { states: compose(None, scene, 1.0, self.canvas, &st.aspects), elapsed: elapsed.as_nanos() as Ns }
            }
            TransitionMode::Cut | TransitionMode::Mix => Layout::One(compose(Some(&a.from), &a.to, t, self.canvas, &st.aspects)),
            mode => Layout::Two {
                from: compose(None, &a.from, 1.0, self.canvas, &st.aspects),
                to: compose(None, &a.to, 1.0, self.canvas, &st.aspects),
                mode,
                t: ease(t),
                direction: st.direction.vector(),
                dip: st.dip,
            },
        };
        if t >= 1.0 {
            let a = st.active.take().unwrap();
            st.current = a.to;
            return Layout::One(compose(None, &st.current, 1.0, self.canvas, &st.aspects));
        }
        out
    }

    /// What's on now, for the vertical version: the scene being left (mid
    /// transition), the scene, and how far (0..1). Wipes, pushes and dips
    /// become a mix there; a stinger changes scenes at its cut point.
    pub fn scenes_now(&self) -> (Option<Scene>, Scene, f32) {
        let st = self.state.lock_ok();
        let Some(a) = &st.active else { return (None, st.current.clone(), 1.0) };
        let elapsed = Instant::now().saturating_duration_since(a.start);
        let t = (elapsed.as_secs_f32() / a.dur.as_secs_f32().max(1e-3)).min(1.0);
        match a.mode {
            TransitionMode::Stinger => (None, if elapsed < a.cut { a.from.clone() } else { a.to.clone() }, 1.0),
            _ if t >= 1.0 => (None, a.to.clone(), 1.0),
            _ => (Some(a.from.clone()), a.to.clone(), t),
        }
    }

    /// The scene that is live (or about to be, mid-transition).
    pub fn live(&self) -> Scene {
        let st = self.state.lock_ok();
        st.active.as_ref().map(|a| a.to.clone()).unwrap_or_else(|| st.current.clone())
    }

    pub fn in_transition(&self) -> bool {
        let st = self.state.lock_ok();
        st.active.as_ref().is_some_and(|a| Instant::now().saturating_duration_since(a.start) < a.dur)
    }

    /// Switches made between two host times.
    pub fn events_between(&self, from: Ns, to: Ns) -> Vec<SwitchEvent> {
        let mut events = self.events.lock_ok();
        let out = events.iter().filter(|e| e.at >= from && e.at <= to).cloned().collect();
        events.retain(|e| e.at >= from);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::presets;

    #[test]
    fn remembers_the_last_scene() {
        let scenes = presets(&["A".to_string(), "B".to_string()]);
        let (a, b) = (scenes[0].clone(), scenes[1].clone());
        let sw = Switcher::new((1280, 720), vec![16.0 / 9.0; 2], a.clone());
        assert!(sw.previous().is_none());
        sw.take(b.clone(), TransitionMode::Mix, 500);
        assert_eq!(sw.previous().map(|s| s.name), Some(a.name.clone()));
        // Taking it again (or mid-transition) keeps the one before.
        sw.take(b.clone(), TransitionMode::Cut, 0);
        assert_eq!(sw.previous().map(|s| s.name), Some(a.name.clone()));
        // Back and forth.
        sw.take(a.clone(), TransitionMode::Cut, 0);
        assert_eq!(sw.previous().map(|s| s.name), Some(b.name.clone()));
        sw.take_stinger(b.clone(), 1_000_000_000, 500_000_000, 0);
        assert_eq!(sw.previous().map(|s| s.name), Some(a.name));
    }

    #[test]
    fn stinger_changes_scenes_at_its_cut_point() {
        let scenes = presets(&["A".to_string(), "B".to_string()]);
        let (a, b) = (scenes[0].clone(), scenes[1].clone());
        let sw = Switcher::new((1280, 720), vec![16.0 / 9.0; 2], a.clone());
        let shows = |l: &Layout| match l {
            Layout::One(s) | Layout::Stinger { states: s, .. } => {
                if s[0].alpha > 0.5 && s[1].alpha < 0.5 {
                    "A"
                } else if s[1].alpha > 0.5 && s[0].alpha < 0.5 {
                    "B"
                } else {
                    "?"
                }
            }
            _ => "two",
        };
        // Generous times: busy test machines oversleep.
        sw.take_stinger(b.clone(), 3_000_000_000, 1_000_000_000, 0);
        let l = sw.layout();
        assert!(matches!(l, Layout::Stinger { elapsed, .. } if elapsed < 1_000_000_000));
        assert_eq!(shows(&l), "A", "before the cut point");
        assert_eq!(sw.live().name, "B");
        assert!(sw.in_transition());
        std::thread::sleep(Duration::from_millis(1300));
        let l = sw.layout();
        assert!(matches!(l, Layout::Stinger { elapsed, .. } if elapsed >= 1_000_000_000));
        assert_eq!(shows(&l), "B", "after the cut point");
        std::thread::sleep(Duration::from_millis(1800));
        let l = sw.layout();
        assert!(matches!(l, Layout::One(_)), "over");
        assert_eq!(shows(&l), "B");
        // The switch is logged at the cut point.
        let ev = sw.events_between(0, i64::MAX);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].mode, TransitionMode::Stinger);
        // A plain take asking for a stinger mixes instead.
        sw.take(a, TransitionMode::Stinger, 100);
        assert!(matches!(sw.layout(), Layout::One(_)));
    }

    #[test]
    fn follows_sources_that_move_or_go() {
        let names: Vec<String> = ["A", "B", "C"].iter().map(|s| s.to_string()).collect();
        let scenes = presets(&names);
        // Live: C full frame. Preview: B.
        let sw = Switcher::new((1280, 720), vec![16.0 / 9.0; 3], scenes[2].clone());
        sw.set_uids(vec![10, 20, 30]);
        sw.set_preview(Some(scenes[1].clone()));
        assert_eq!(sw.angle_now(), Some(30));
        // A is removed, a new D is added at the end: C is now number 1.
        sw.remap_sources(&[Some(1), Some(2), None], vec![16.0 / 9.0; 3], vec![20, 30, 40]);
        assert_eq!(sw.live().layers.iter().map(|l| l.source).collect::<Vec<_>>(), vec![1]);
        assert_eq!(sw.preview().unwrap().layers[0].source, 0, "B moved up too");
        assert_eq!(sw.angle_now(), Some(30), "the same camera");
        match sw.layout() {
            Layout::One(s) => assert!(s[1].alpha > 0.5 && s[0].alpha < 0.5 && s[2].alpha < 0.5),
            _ => panic!(),
        }
        // The camera on air goes: nothing of it is left.
        sw.remap_sources(&[Some(0), Some(2)], vec![16.0 / 9.0; 2], vec![20, 40]);
        assert!(sw.live().layers.is_empty());
        assert_eq!(sw.source_count(), 2);
    }

    #[test]
    fn fades_to_black_and_back() {
        let scenes = presets(&["A".to_string()]);
        let sw = Switcher::new((1280, 720), vec![16.0 / 9.0], scenes[0].clone());
        assert_eq!(sw.black_level(), 0.0);
        assert!(!sw.is_black());
        sw.fade_to_black(true, 200);
        assert!(sw.is_black());
        let mid = sw.black_level();
        assert!(mid < 0.5, "starts from the program: {mid}");
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(sw.black_level(), 1.0);
        sw.fade_to_black(false, 0);
        assert_eq!(sw.black_level(), 0.0);
    }
}
