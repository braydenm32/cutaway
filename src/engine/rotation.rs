//! Graphics taking turns: every few minutes the next graphic in a rotation
//! comes on for a while, then goes off. Sponsors, social handles and
//! reminders without anyone having to remember them.

use super::graphics::Graphics;
use crate::config::{Rotation, RotationStart};
use crate::control::Toggle;
use crate::util::LockExt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    name: String,
    running: bool,
    /// Whose turn is next (counts up; wraps over the list).
    turn: usize,
    next_at: Option<Instant>,
    /// The graphic it put on, and when it goes off.
    showing: Option<(String, Instant)>,
    /// Live (or recording) at the last tick, to start and stop on a change.
    was_on_air: bool,
}

/// What a rotation is doing, for its button.
#[derive(Clone, Debug, PartialEq)]
pub struct RotationInfo {
    pub name: String,
    pub running: bool,
    /// The graphic on screen because of it.
    pub showing: Option<String>,
    /// Until the next one comes on.
    pub next_in: Option<Duration>,
}

#[derive(Default)]
pub struct Rotations {
    states: Mutex<Vec<State>>,
}

impl Rotations {
    pub fn new() -> Rotations {
        Rotations::default()
    }

    /// Called every second: start or stop with going live or recording,
    /// take graphics off whose time is up, and put the next one on.
    pub fn tick(&self, rotations: &[Rotation], graphics: &Graphics, live: bool, recording: bool, now: Instant) {
        let mut states = self.states.lock_ok();
        // Follow the settings by name, keeping each one's place.
        let mut old = std::mem::take(&mut *states);
        for r in rotations {
            let st = match old.iter().position(|s| s.name == r.name) {
                Some(i) => old.remove(i),
                None => State { name: r.name.clone(), ..Default::default() },
            };
            states.push(st);
        }
        // Removed while running: take its graphic off.
        for st in old {
            if let Some((g, _)) = st.showing {
                hide(graphics, &g);
            }
        }
        for (r, st) in rotations.iter().zip(states.iter_mut()) {
            let on_air = match r.start {
                RotationStart::Manual => None,
                RotationStart::Live => Some(live),
                RotationStart::OnAir => Some(live || recording),
            };
            if let Some(on) = on_air {
                if on != st.was_on_air {
                    st.running = on;
                    if on {
                        st.turn = 0;
                    }
                }
                st.was_on_air = on;
            }
            step(r, st, graphics, now);
        }
    }

    /// Start or stop one (by number or name). Returns its name and whether
    /// it's running now.
    pub fn set(&self, rotations: &[Rotation], target: &crate::control::Target, t: Toggle, graphics: &Graphics) -> Result<(String, bool), String> {
        let names: Vec<String> = rotations.iter().map(|r| r.name.clone()).collect();
        let i = target.find(&names).ok_or("there's no such rotation")?;
        let mut states = self.states.lock_ok();
        if states.len() != rotations.len() || states.iter().zip(rotations).any(|(s, r)| s.name != r.name) {
            // Not ticked since the settings changed: line up first.
            drop(states);
            self.tick(rotations, graphics, false, false, Instant::now());
            states = self.states.lock_ok();
        }
        let st = states.get_mut(i).ok_or("there's no such rotation")?;
        let on = match t {
            Toggle::On => true,
            Toggle::Off => false,
            Toggle::Toggle => !st.running,
        };
        if on != st.running {
            st.running = on;
            st.next_at = None;
            if on {
                st.turn = 0;
            }
            step(&rotations[i], st, graphics, Instant::now());
        }
        Ok((names[i].clone(), on))
    }

    pub fn list(&self) -> Vec<RotationInfo> {
        let now = Instant::now();
        self.states
            .lock_ok()
            .iter()
            .map(|s| RotationInfo {
                name: s.name.clone(),
                running: s.running,
                showing: s.showing.as_ref().map(|(g, _)| g.clone()),
                next_in: s.next_at.filter(|_| s.running).map(|t| t.saturating_duration_since(now)),
            })
            .collect()
    }
}

fn hide(graphics: &Graphics, name: &str) {
    if let Some(i) = graphics.index_of(name) {
        graphics.set_visible(i, false);
    }
}

fn step(r: &Rotation, st: &mut State, graphics: &Graphics, now: Instant) {
    if !st.running {
        if let Some((g, _)) = st.showing.take() {
            hide(graphics, &g);
        }
        st.next_at = None;
        return;
    }
    if let Some((g, until)) = &st.showing {
        if now >= *until {
            hide(graphics, g);
            st.showing = None;
        }
    }
    // The first one comes on as soon as it starts.
    let next_at = *st.next_at.get_or_insert(now);
    if now < next_at {
        return;
    }
    let present: Vec<&String> = r.graphics.iter().filter(|g| graphics.index_of(g).is_some()).collect();
    if present.is_empty() {
        return;
    }
    let show = Duration::from_secs(r.show_seconds.max(1) as u64);
    let every = Duration::from_secs_f32((r.every_minutes * 60.0).clamp(1.0, 24.0 * 3600.0));
    let name = present[st.turn % present.len()].clone();
    st.turn += 1;
    if let Some((g, _)) = st.showing.take() {
        if g != name {
            hide(graphics, &g);
        }
    }
    if let Some(i) = graphics.index_of(&name) {
        graphics.set_visible(i, true);
    }
    st.showing = Some((name, now + show));
    // Back to back when each stays on longer than the gap.
    st.next_at = Some(now + every.max(show));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Graphic;
    use crate::control::Target;
    use crate::engine::timers::Timers;
    use std::sync::Arc;

    fn setup() -> (Arc<Graphics>, Vec<Rotation>) {
        let cfgs: Vec<Graphic> = ["Acme", "Bolt", "Handles"].iter().map(|n| Graphic::title(n, n, "")).collect();
        let g = Graphics::new(&cfgs, (1280, 720), Timers::new());
        let r = Rotation { name: "Sponsors".into(), graphics: vec!["Acme".into(), "Bolt".into(), "Gone".into()], every_minutes: 1.0, show_seconds: 10, start: RotationStart::Live };
        (g, vec![r])
    }

    fn on(g: &Graphics) -> Vec<String> {
        g.list().into_iter().filter(|x| x.1).map(|x| x.0).collect()
    }

    #[test]
    fn takes_turns_while_live() {
        let (g, rots) = setup();
        let r = Rotations::new();
        let t = Instant::now();
        r.tick(&rots, &g, false, false, t);
        assert!(on(&g).is_empty(), "not live yet");
        r.tick(&rots, &g, true, false, t);
        assert_eq!(on(&g), ["Acme"], "the first one comes on straight away");
        r.tick(&rots, &g, true, false, t + Duration::from_secs(11));
        assert!(on(&g).is_empty(), "off after its time");
        assert_eq!(r.list()[0].next_in.map(|d| d.as_secs() > 0), Some(true));
        r.tick(&rots, &g, true, false, t + Duration::from_secs(60));
        assert_eq!(on(&g), ["Bolt"], "a missing graphic is skipped");
        r.tick(&rots, &g, true, false, t + Duration::from_secs(120));
        assert_eq!(on(&g), ["Acme"], "round again");
        r.tick(&rots, &g, false, false, t + Duration::from_secs(125));
        assert!(on(&g).is_empty(), "the stream ended: off, and stopped");
        assert!(!r.list()[0].running);
        r.tick(&rots, &g, false, false, t + Duration::from_secs(300));
        assert!(on(&g).is_empty());
    }

    #[test]
    fn started_and_stopped_by_hand() {
        let (g, mut rots) = setup();
        rots[0].start = RotationStart::Manual;
        rots[0].every_minutes = 0.1;
        rots[0].show_seconds = 30;
        let r = Rotations::new();
        let t = Instant::now();
        r.tick(&rots, &g, true, true, t);
        assert!(on(&g).is_empty(), "manual ones don't start with the stream");
        assert_eq!(r.set(&rots, &Target::Name("sponsors".into()), Toggle::Toggle, &g), Ok(("Sponsors".into(), true)));
        assert_eq!(on(&g), ["Acme"]);
        // Each stays on longer than the gap: back to back.
        r.tick(&rots, &g, true, true, Instant::now() + Duration::from_secs(31));
        assert_eq!(on(&g), ["Bolt"]);
        assert_eq!(r.set(&rots, &Target::Index(0), Toggle::Off, &g), Ok(("Sponsors".into(), false)));
        assert!(on(&g).is_empty());
        assert!(r.set(&rots, &Target::Index(3), Toggle::On, &g).is_err());
    }

    #[test]
    fn counts_time_on_screen() {
        let (g, _) = setup();
        let t0 = crate::media::clock::now();
        g.set_visible(0, true);
        std::thread::sleep(Duration::from_millis(30));
        g.set_visible(0, false);
        g.set_visible(1, true);
        g.set_visible(0, true);
        let t1 = crate::media::clock::now();
        let report = g.on_air(t0 - 1_000_000_000, t1);
        assert_eq!(report.iter().map(|o| (o.name.as_str(), o.showings.len())).collect::<Vec<_>>(), [("Acme", 2), ("Bolt", 1)]);
        assert!(report[0].seconds() > 0.02);
        assert!(g.on_air(t1 + 1_000_000_000, t1 + 2_000_000_000).len() == 2, "still on later");
        assert!(g.on_air(t0 - 3_000_000_000, t0 - 2_000_000_000).is_empty());
    }
}
