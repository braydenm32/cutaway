//! Which scene is live, what's in preview, and the transition between
//! scenes. The compositor asks for the frame layout every time it renders.

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
    /// The single source shown full frame, if the scene is one.
    pub angle: Option<usize>,
}

/// What the compositor draws for the program.
pub enum Layout {
    /// One arrangement of sources (including mid-mix glides).
    One(Vec<PadState>),
    /// Two whole scenes combined by a wipe, push or dip.
    Two { from: Vec<PadState>, to: Vec<PadState>, mode: TransitionMode, t: f32, direction: [f32; 2], dip: [f32; 3] },
}

struct Active {
    from: Scene,
    to: Scene,
    mode: TransitionMode,
    start: Instant,
    dur: Duration,
}

struct State {
    current: Scene,
    active: Option<Active>,
    preview: Option<Scene>,
    aspects: Vec<f32>,
    direction: Direction,
    dip: [f32; 3],
}

pub struct Switcher {
    canvas: (u32, u32),
    state: Mutex<State>,
    events: Mutex<Vec<SwitchEvent>>,
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Switcher {
    pub fn new(canvas: (u32, u32), aspects: Vec<f32>, initial: Scene) -> Self {
        Switcher {
            canvas,
            state: Mutex::new(State { current: initial, active: None, preview: None, aspects, direction: Direction::Right, dip: [0.0; 3] }),
            events: Mutex::new(Vec::new()),
        }
    }

    /// Direction for wipes/pushes and the colour for dips.
    pub fn set_style(&self, direction: Direction, dip_color: &str) {
        let c = parse_color(dip_color).unwrap_or(0xff000000);
        let mut st = self.state.lock().unwrap();
        st.direction = direction;
        st.dip = [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0];
    }

    /// Source shapes changed (cropping).
    pub fn set_aspects(&self, aspects: Vec<f32>) {
        self.state.lock().unwrap().aspects = aspects;
    }

    /// Go live with `scene`.
    pub fn take(&self, scene: Scene, mode: TransitionMode, duration_ms: u32) {
        let mut st = self.state.lock().unwrap();
        if let Some(a) = st.active.take() {
            st.current = a.to;
        }
        self.events.lock().unwrap().push(SwitchEvent { at: clock::now(), scene: scene.name.clone(), mode, duration_ms, angle: scene.full_frame_source() });
        if mode == TransitionMode::Cut || duration_ms == 0 || scene == st.current {
            st.current = scene;
        } else {
            let from = st.current.clone();
            st.active = Some(Active { from, to: scene, mode, start: Instant::now(), dur: Duration::from_millis(duration_ms as u64) });
        }
    }

    /// Change the live layout immediately (dragging frames around).
    pub fn set_live(&self, scene: Scene) {
        let mut st = self.state.lock().unwrap();
        st.active = None;
        st.current = scene;
    }

    /// Put a scene in preview (None clears it).
    pub fn set_preview(&self, scene: Option<Scene>) {
        self.state.lock().unwrap().preview = scene;
    }

    pub fn preview(&self) -> Option<Scene> {
        self.state.lock().unwrap().preview.clone()
    }

    /// Layout of the preview scene, if there is one.
    pub fn preview_states(&self) -> Option<Vec<PadState>> {
        let st = self.state.lock().unwrap();
        st.preview.as_ref().map(|p| compose(None, p, 1.0, self.canvas, &st.aspects))
    }

    /// What to draw now.
    pub fn layout(&self) -> Layout {
        let mut st = self.state.lock().unwrap();
        let Some(a) = &st.active else {
            return Layout::One(compose(None, &st.current, 1.0, self.canvas, &st.aspects));
        };
        let t = (a.start.elapsed().as_secs_f32() / a.dur.as_secs_f32().max(1e-3)).min(1.0);
        let out = match a.mode {
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

    /// The scene that is live (or about to be, mid-transition).
    pub fn live(&self) -> Scene {
        let st = self.state.lock().unwrap();
        st.active.as_ref().map(|a| a.to.clone()).unwrap_or_else(|| st.current.clone())
    }

    pub fn in_transition(&self) -> bool {
        let st = self.state.lock().unwrap();
        st.active.as_ref().is_some_and(|a| a.start.elapsed() < a.dur)
    }

    /// Switches made between two host times.
    pub fn events_between(&self, from: Ns, to: Ns) -> Vec<SwitchEvent> {
        let mut events = self.events.lock().unwrap();
        let out = events.iter().filter(|e| e.at >= from && e.at <= to).cloned().collect();
        events.retain(|e| e.at >= from);
        out
    }
}
