//! Undo and redo for what you build: scene layouts (moving, resizing,
//! filling and zooming frames; adding, removing, renaming, duplicating and
//! deleting scenes) and changes applied in Settings. Not the show itself:
//! taking scenes, recording or going live are never undone.

use crate::config::Config;
use crate::scene::Scene;
use std::time::{Duration, Instant};

/// What a step changed, so undoing it puts back only that (a scene edit
/// never undoes, say, the transition picked since).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Scene layouts and the list of scenes.
    Scenes,
    /// Titles' and tickers' text.
    Titles,
    /// Everything in Settings (and switching shows).
    Settings,
}

/// What an undo puts back.
#[derive(Clone)]
pub struct Snapshot {
    /// What the change was, for "Undone: …".
    pub label: String,
    pub kind: Kind,
    pub cfg: Config,
    pub scenes: Vec<Scene>,
    /// The scene on air (by name) when it was taken.
    pub live: String,
    at: Instant,
}

impl Snapshot {
    pub fn new(kind: Kind, label: impl Into<String>, cfg: &Config, scenes: &[Scene], live: &str) -> Self {
        Snapshot { label: label.into(), kind, cfg: cfg.clone(), scenes: scenes.to_vec(), live: live.to_string(), at: Instant::now() }
    }

    /// Whether putting this back changes the settings (not just scenes).
    pub fn settings_differ(&self, cfg: &Config) -> bool {
        let strip = |c: &Config| Config { scenes: Vec::new(), ..c.clone() };
        strip(&self.cfg) != strip(cfg)
    }
}

#[derive(Default)]
pub struct History {
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
}

/// How many steps back are kept.
const DEPTH: usize = 50;
/// Changes of the same kind this close together are one step (a slider
/// dragged, the wheel scrolled).
const COALESCE: Duration = Duration::from_millis(1200);

impl History {
    /// Remember how things were before a change. A new change ends what
    /// could be redone.
    pub fn record(&mut self, before: Snapshot) {
        if let Some(top) = self.undo.last_mut() {
            if top.label == before.label && before.at.duration_since(top.at) < COALESCE {
                // Same gesture going on: keep the state from its start, but
                // count from now.
                top.at = before.at;
                self.redo.clear();
                return;
            }
        }
        self.undo.push(before);
        if self.undo.len() > DEPTH {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// The state to go back to; `now` is kept to redo it.
    pub fn undo(&mut self, now: Snapshot) -> Option<Snapshot> {
        let back = self.undo.pop()?;
        self.redo.push(Snapshot { label: back.label.clone(), kind: back.kind, ..now });
        Some(back)
    }

    /// The state to go forward to; `now` is kept to undo it again.
    pub fn redo(&mut self, now: Snapshot) -> Option<Snapshot> {
        let fwd = self.redo.pop()?;
        self.undo.push(Snapshot { label: fwd.label.clone(), kind: fwd.kind, at: Instant::now() - COALESCE, ..now });
        Some(fwd)
    }

    /// An undo or redo that couldn't be applied goes back on its stack.
    pub fn put_back_undo(&mut self, s: Snapshot) {
        self.redo.pop();
        self.undo.push(s);
    }

    pub fn put_back_redo(&mut self, s: Snapshot) {
        self.undo.pop();
        self.redo.push(s);
    }

    pub fn can_undo(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn can_redo(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(label: &str, n: usize) -> Snapshot {
        let scenes = (0..n).map(|i| Scene { name: format!("S{i}"), ..Default::default() }).collect::<Vec<_>>();
        Snapshot::new(Kind::Scenes, label, &Config::default(), &scenes, "S0")
    }

    #[test]
    fn undo_then_redo() {
        let mut h = History::default();
        h.record(snap("Add scene", 1));
        assert_eq!(h.can_undo(), Some("Add scene"));
        let back = h.undo(snap("now", 2)).unwrap();
        assert_eq!(back.scenes.len(), 1);
        assert_eq!(h.can_redo(), Some("Add scene"));
        let fwd = h.redo(snap("now", 1)).unwrap();
        assert_eq!(fwd.scenes.len(), 2);
        assert_eq!(h.can_undo(), Some("Add scene"));
        assert!(h.can_redo().is_none());
    }

    #[test]
    fn one_gesture_is_one_step() {
        let mut h = History::default();
        h.record(snap("Zoom Cam 1", 1));
        h.record(snap("Zoom Cam 1", 2));
        h.record(snap("Zoom Cam 1", 3));
        // Back to before the gesture, in one step.
        assert_eq!(h.undo(snap("now", 4)).unwrap().scenes.len(), 1);
        assert!(h.can_undo().is_none());
        // Different changes are different steps.
        h.record(snap("Move Cam 1", 1));
        h.record(snap("Zoom Cam 1", 2));
        assert_eq!(h.undo(snap("now", 3)).unwrap().scenes.len(), 2);
        assert_eq!(h.undo(snap("now", 2)).unwrap().scenes.len(), 1);
    }

    #[test]
    fn a_new_change_ends_redo() {
        let mut h = History::default();
        h.record(snap("A", 1));
        h.undo(snap("now", 2));
        assert!(h.can_redo().is_some());
        h.record(snap("B", 1));
        assert!(h.can_redo().is_none());
    }

    #[test]
    fn keeps_a_limit() {
        let mut h = History::default();
        for i in 0..(DEPTH + 10) {
            h.record(snap(&format!("step {i}"), 1));
        }
        let mut n = 0;
        while h.undo(snap("now", 1)).is_some() {
            n += 1;
        }
        assert_eq!(n, DEPTH);
    }

    #[test]
    fn scenes_alone_dont_count_as_settings() {
        let a = snap("x", 1);
        let mut cfg = Config::default();
        cfg.scenes = vec![Scene::default(); 3];
        assert!(!a.settings_differ(&cfg));
        cfg.program.fps = 50;
        assert!(a.settings_differ(&cfg));
    }
}
