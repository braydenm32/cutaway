//! Automatic switching from audio: cut to whoever is talking, go wide when
//! several people talk at once or a shot has been up a long time, and step
//! aside whenever the operator switches by hand.

use super::audio::InputShared;
use super::switcher::Switcher;
use super::{LogLevel, Status, Waker};
use crate::config::{AutoSwitchConfig, Config};
use crate::scene::Scene;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// What the UI shows about automatic switching.
#[derive(Clone, Debug, Default)]
pub struct AutoStatus {
    pub enabled: bool,
    /// Per rule: talking now.
    pub talking: Vec<bool>,
    /// Seconds left of the pause after a manual switch.
    pub paused_for: f32,
}

/// Shared between the engine (manual switches, on/off) and the thread.
pub struct AutoShared {
    pub enabled: AtomicBool,
    last_manual: Mutex<Option<Instant>>,
}

impl AutoShared {
    pub fn new(enabled: bool) -> Arc<Self> {
        Arc::new(AutoShared { enabled: AtomicBool::new(enabled), last_manual: Mutex::new(None) })
    }

    /// The operator switched: hold off for a while.
    pub fn manual_switch(&self) {
        *self.last_manual.lock().unwrap() = Some(Instant::now());
    }
}

/// Talk detection for one rule.
#[derive(Clone, Default)]
struct Voice {
    /// Above the threshold continuously (allowing short gaps) since.
    since: Option<Instant>,
    last_loud: Option<Instant>,
    level: f32,
}

/// The decision part, kept free of threads and devices for testing.
pub struct Director {
    voices: Vec<Voice>,
    last_switch: Instant,
    /// When a "breather" wide shot started (after a long shot).
    breather: Option<Instant>,
}

/// One tick's inputs.
pub struct Tick<'a> {
    pub now: Instant,
    pub cfg: &'a AutoSwitchConfig,
    /// Level (dB) per rule.
    pub levels: &'a [f32],
    /// Name of the scene on air.
    pub live: &'a str,
    pub manual_pause_left: f32,
}

const GAP: Duration = Duration::from_millis(350);

impl Director {
    pub fn new(now: Instant) -> Self {
        Director { voices: Vec::new(), last_switch: now, breather: None }
    }

    pub fn talking(&self, now: Instant, hold: Duration) -> Vec<bool> {
        self.voices.iter().map(|v| v.since.is_some_and(|s| now.duration_since(s) >= hold) && v.last_loud.is_some_and(|l| now.duration_since(l) < GAP)).collect()
    }

    /// The scene to switch to now, if any.
    pub fn decide(&mut self, t: &Tick) -> Option<String> {
        let cfg = t.cfg;
        if self.voices.len() != cfg.rules.len() {
            self.voices = vec![Voice::default(); cfg.rules.len()];
        }
        for (v, &level) in self.voices.iter_mut().zip(t.levels) {
            v.level = level;
            if level >= cfg.threshold_db {
                v.last_loud = Some(t.now);
                if v.since.is_none() {
                    v.since = Some(t.now);
                }
            } else if v.last_loud.is_none_or(|l| t.now.duration_since(l) >= GAP) {
                v.since = None;
            }
        }
        if t.manual_pause_left > 0.0 {
            self.last_switch = t.now;
            self.breather = None;
            return None;
        }
        let hold = Duration::from_millis(cfg.hold_ms as u64);
        let talking = self.talking(t.now, hold);
        let speakers: Vec<usize> = talking.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| i).collect();
        let wide = (!cfg.wide_scene.trim().is_empty()).then(|| cfg.wide_scene.trim().to_string());
        let shot_len = t.now.duration_since(self.last_switch).as_secs_f32();

        // A breather wide shot runs its course first.
        if let Some(b) = self.breather {
            if t.now.duration_since(b).as_secs_f32() < cfg.wide_for_s.max(1.0) {
                return None;
            }
            self.breather = None;
        }
        let mut want: Option<String> = match speakers.len() {
            0 => None,
            1 => Some(cfg.rules[speakers[0]].scene.clone()),
            _ => {
                // Several at once: everyone on the same shot, the wide shot, or the loudest.
                let scenes: Vec<&str> = speakers.iter().map(|&i| cfg.rules[i].scene.as_str()).collect();
                if scenes.iter().all(|s| *s == scenes[0]) {
                    Some(scenes[0].to_string())
                } else if cfg.wide_on_crosstalk && wide.is_some() {
                    wide.clone()
                } else {
                    let loudest = speakers.iter().copied().max_by(|a, b| self.voices[*a].level.total_cmp(&self.voices[*b].level)).unwrap();
                    Some(cfg.rules[loudest].scene.clone())
                }
            }
        };
        // Someone has been on screen a long time: a moment on the wide shot.
        if cfg.max_shot_s > 0.0 && shot_len >= cfg.max_shot_s {
            if let Some(w) = &wide {
                if t.live != w && want.as_deref().is_none_or(|s| s == t.live) {
                    want = Some(w.clone());
                    self.breather = Some(t.now);
                }
            }
        }
        let want = want.filter(|s| !s.is_empty() && s != t.live)?;
        if shot_len < cfg.min_shot_s.max(0.5) {
            self.breather = None;
            return None;
        }
        self.last_switch = t.now;
        Some(want)
    }
}

/// Levels for each rule's input/channel, in dB.
fn rule_levels(cfg: &AutoSwitchConfig, inputs: &[(String, Arc<InputShared>)]) -> Vec<f32> {
    cfg.rules
        .iter()
        .map(|r| {
            let Some((_, shared)) = inputs.iter().find(|(n, _)| n.eq_ignore_ascii_case(r.input.trim())) else { return -120.0 };
            if !shared.is_on() {
                return -120.0; // a muted mic doesn't count
            }
            let levels = shared.levels_db();
            match r.channel {
                0 => levels.iter().copied().fold(-120.0, f32::max),
                c => levels.get(c as usize - 1).copied().unwrap_or(-120.0),
            }
        })
        .collect()
}

pub struct AutoSwitcher {
    pub shared: Arc<AutoShared>,
    pub cfg: Arc<Mutex<Config>>,
    /// The audio inputs by name (they change when inputs are added or removed).
    pub inputs: Arc<Mutex<Vec<(String, Arc<InputShared>)>>>,
    pub switcher: Arc<Switcher>,
    /// Does the switching (applies scene audio too).
    pub take: Box<dyn Fn(&Scene) + Send>,
    pub status: Arc<Mutex<Status>>,
    pub stop: Arc<AtomicBool>,
    pub waker: Waker,
}

impl AutoSwitcher {
    pub fn run(self) {
        let mut director = Director::new(Instant::now());
        let mut last_status = AutoStatus::default();
        let mut warned_missing = String::new();
        while !self.stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
            let now = Instant::now();
            // Only its own settings (this runs twenty times a second).
            let a = &self.cfg.lock().unwrap().auto_switch.clone();
            let enabled = self.shared.enabled.load(Ordering::Relaxed);
            let pause_left = self.shared.last_manual.lock().unwrap().map(|m| (a.manual_pause_s - now.duration_since(m).as_secs_f32()).max(0.0)).unwrap_or(0.0);
            let levels = rule_levels(a, &self.inputs.lock().unwrap());
            let live = self.switcher.live();
            let decision = if enabled {
                director.decide(&Tick { now, cfg: a, levels: &levels, live: &live.name, manual_pause_left: pause_left })
            } else {
                director.decide(&Tick { now, cfg: a, levels: &levels, live: &live.name, manual_pause_left: f32::MAX });
                None
            };
            if let Some(name) = decision {
                let scenes = self.cfg.lock().unwrap().effective_scenes();
                match scenes.into_iter().find(|s| s.name.eq_ignore_ascii_case(&name)) {
                    Some(scene) => (self.take)(&scene),
                    None if warned_missing != name => {
                        warned_missing = name.clone();
                        self.status.lock().unwrap().push(LogLevel::Warning, format!("Auto switching: there's no scene called \"{name}\""));
                    }
                    None => {}
                }
            }
            let st = AutoStatus { enabled, talking: director.talking(now, Duration::from_millis(a.hold_ms as u64)), paused_for: if enabled { pause_left } else { 0.0 } };
            let changed = st.enabled != last_status.enabled || st.talking != last_status.talking || (st.paused_for > 0.0) != (last_status.paused_for > 0.0);
            if changed || st.paused_for > 0.0 {
                self.status.lock().unwrap().auto_switch = st.clone();
                if changed {
                    (self.waker)();
                }
                last_status = st;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AutoRule;

    fn cfg() -> AutoSwitchConfig {
        AutoSwitchConfig {
            enabled: true,
            rules: vec![
                AutoRule { input: "Mics".into(), channel: 1, scene: "Host".into() },
                AutoRule { input: "Mics".into(), channel: 2, scene: "Guest".into() },
            ],
            threshold_db: -40.0,
            hold_ms: 300,
            min_shot_s: 2.0,
            wide_scene: "Wide".into(),
            wide_on_crosstalk: true,
            max_shot_s: 0.0,
            wide_for_s: 3.0,
            manual_pause_s: 5.0,
        }
    }

    /// Runs the director over a script of (seconds, host dB, guest dB) and
    /// returns the switches made.
    fn run(c: &AutoSwitchConfig, script: &[(f32, f32, f32)], until: f32, pause_at: Option<f32>) -> Vec<(f32, String)> {
        let start = Instant::now();
        let mut d = Director::new(start);
        let mut live = "Wide".to_string();
        let mut out = Vec::new();
        let mut t = 0.0f32;
        while t < until {
            let lv = script.iter().rev().find(|s| s.0 <= t).map(|s| [s.1, s.2]).unwrap_or([-90.0, -90.0]);
            let now = start + Duration::from_secs_f32(t);
            let pause = pause_at.map(|p| if t >= p { (c.manual_pause_s - (t - p)).max(0.0) } else { 0.0 }).unwrap_or(0.0);
            if let Some(s) = d.decide(&Tick { now, cfg: c, levels: &lv, live: &live, manual_pause_left: pause }) {
                out.push(((t * 10.0).round() / 10.0, s.clone()));
                live = s;
            }
            t += 0.05;
        }
        out
    }

    #[test]
    fn follows_the_talker() {
        let c = cfg();
        // Host talks from 2.5 s, guest answers from 8 s, both at once from 14 s.
        let script = [(0.0, -90.0, -90.0), (2.5, -20.0, -90.0), (8.0, -90.0, -22.0), (14.0, -20.0, -20.0)];
        let sw = run(&c, &script, 20.0, None);
        let names: Vec<&str> = sw.iter().map(|s| s.1.as_str()).collect();
        assert_eq!(names, ["Host", "Guest", "Wide"], "{sw:?}");
        assert!((sw[0].0 - 2.8).abs() < 0.11, "waits for the hold time: {sw:?}");
    }

    #[test]
    fn respects_minimum_shot_and_manual_pause() {
        let c = cfg();
        // Rapid back and forth every second: never shorter than 2 s shots.
        let script: Vec<(f32, f32, f32)> = (0..12).map(|i| (i as f32, if i % 2 == 0 { -20.0 } else { -90.0 }, if i % 2 == 1 { -20.0 } else { -90.0 })).collect();
        let sw = run(&c, &script, 12.0, None);
        for w in sw.windows(2) {
            assert!(w[1].0 - w[0].0 >= 1.99, "{sw:?}");
        }
        // A manual switch at 3 s holds everything for 5 s.
        let sw = run(&c, &script, 12.0, Some(3.0));
        assert!(sw.iter().all(|s| s.0 < 3.0 || s.0 >= 8.0), "{sw:?}");
    }

    #[test]
    fn long_shots_get_a_breather() {
        let c = AutoSwitchConfig { max_shot_s: 6.0, ..cfg() };
        let script = [(0.0, -20.0, -90.0)]; // the host talks forever
        let sw = run(&c, &script, 20.0, None);
        let names: Vec<&str> = sw.iter().map(|s| s.1.as_str()).collect();
        assert_eq!(&names[..3], ["Host", "Wide", "Host"], "{sw:?}");
    }
}
