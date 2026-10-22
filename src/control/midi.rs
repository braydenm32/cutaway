//! MIDI controllers: pads and buttons run commands; faders set input
//! levels; pads light up to show what's on air (where the controller
//! supports it).
//!
//! A mapping is a message ("note 36", "cc 20", "note 36 ch 10") and a
//! command (see `control`). For a fader, map a CC to `fader:<input>`.

use super::{Action, Request};
use crate::config::{MidiConfig, MidiMapping};
use crate::engine::Waker;
use crossbeam_channel::Sender;
use std::sync::{Arc, Mutex};

/// A parsed message pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Msg {
    Note { ch: u8, note: u8 },
    Cc { ch: u8, cc: u8 },
}

impl Msg {
    pub fn parse(s: &str) -> Option<Msg> {
        let words: Vec<String> = s.split_whitespace().map(|w| w.to_lowercase()).collect();
        let num = |i: usize| words.get(i).and_then(|w| w.parse::<u8>().ok());
        let ch = match words.iter().position(|w| w == "ch" || w == "channel") {
            Some(i) => num(i + 1)?.clamp(1, 16) - 1,
            None => 0,
        };
        match words.first()?.as_str() {
            "note" => Some(Msg::Note { ch, note: num(1)?.min(127) }),
            "cc" => Some(Msg::Cc { ch, cc: num(1)?.min(127) }),
            _ => None,
        }
    }

    pub fn describe(&self) -> String {
        match *self {
            Msg::Note { ch: 0, note } => format!("note {note}"),
            Msg::Note { ch, note } => format!("note {note} ch {}", ch + 1),
            Msg::Cc { ch: 0, cc } => format!("cc {cc}"),
            Msg::Cc { ch, cc } => format!("cc {cc} ch {}", ch + 1),
        }
    }

    /// From raw bytes: (message, value). Note-offs come back as value 0.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn from_bytes(b: &[u8]) -> Option<(Msg, u8)> {
        let (status, d1, d2) = (*b.first()?, *b.get(1)?, *b.get(2).unwrap_or(&0));
        let ch = status & 0x0f;
        match status & 0xf0 {
            0x90 => Some((Msg::Note { ch, note: d1 }, d2)),
            0x80 => Some((Msg::Note { ch, note: d1 }, 0)),
            0xB0 => Some((Msg::Cc { ch, cc: d1 }, d2)),
            _ => None,
        }
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn bytes(&self, value: u8) -> [u8; 3] {
        match *self {
            Msg::Note { ch, note } => [0x90 | ch, note, value.min(127)],
            Msg::Cc { ch, cc } => [0xB0 | ch, cc, value.min(127)],
        }
    }
}

/// The command a mapping runs for a message value (None: ignore).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn action_for(m: &MidiMapping, msg: Msg, value: u8) -> Option<Action> {
    let a = m.action.trim();
    if let Some(input) = a.strip_prefix("fader:") {
        let db = super::fader_db(value as f64 / 127.0);
        return Action::parse(&format!("gain:{input}:{db:.1}")).ok();
    }
    let pressed = match msg {
        Msg::Note { .. } => value > 0,
        Msg::Cc { .. } => value >= 64,
    };
    pressed.then(|| Action::parse(a).ok()).flatten()
}

/// Shared with the settings window ("learn").
#[derive(Default)]
pub struct MidiShared {
    /// Last message received (for learning a mapping).
    pub last: Mutex<Option<(Msg, std::time::Instant)>>,
    /// Controllers connected.
    pub connected: Mutex<Vec<String>>,
}

pub fn available() -> bool {
    cfg!(any(target_os = "macos", windows))
}

#[cfg(any(target_os = "macos", windows))]
mod imp {
    use super::*;
    use crate::util::LockExt;
    use midir::{MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};
    use std::collections::HashMap;

    pub struct Midi {
        _inputs: Vec<MidiInputConnection<()>>,
        outputs: Vec<MidiOutputConnection>,
        sent: HashMap<Msg, u8>,
        pub shared: Arc<MidiShared>,
    }

    pub fn port_names() -> Vec<String> {
        let Ok(m) = MidiInput::new("Cutaway") else { return Vec::new() };
        m.ports().iter().filter_map(|p| m.port_name(p).ok()).collect()
    }

    impl Midi {
        pub fn start(cfg: &MidiConfig, requests: Sender<Request>, waker: Waker) -> anyhow::Result<Midi> {
            let shared = Arc::new(MidiShared::default());
            let want = cfg.device.trim().to_lowercase();
            let matches = |name: &str| want.is_empty() || name.to_lowercase().contains(&want);
            let mut inputs = Vec::new();
            let probe = MidiInput::new("Cutaway")?;
            let ports = probe.ports();
            for p in &ports {
                let name = probe.port_name(p).unwrap_or_default();
                if !matches(&name) {
                    continue;
                }
                let input = MidiInput::new("Cutaway")?;
                let (map, tx, sh, wk) = (cfg.mappings.clone(), requests.clone(), shared.clone(), waker.clone());
                let cb = move |_t: u64, bytes: &[u8], _: &mut ()| {
                    let Some((msg, value)) = Msg::from_bytes(bytes) else { return };
                    *sh.last.lock_ok() = Some((msg, std::time::Instant::now()));
                    for m in map.iter().filter(|m| Msg::parse(&m.message) == Some(msg)) {
                        if let Some(a) = action_for(m, msg, value) {
                            let _ = tx.send(Request { action: a, reply: None });
                            wk();
                        }
                    }
                };
                match input.connect(p, "Cutaway in", cb, ()) {
                    Ok(c) => {
                        inputs.push(c);
                        shared.connected.lock_ok().push(name);
                    }
                    Err(e) => eprintln!("MIDI: couldn't open {name}: {e}"),
                }
            }
            let mut outputs = Vec::new();
            if cfg.feedback {
                let probe = MidiOutput::new("Cutaway")?;
                for p in probe.ports() {
                    let name = probe.port_name(&p).unwrap_or_default();
                    if !matches(&name) {
                        continue;
                    }
                    if let Ok(out) = MidiOutput::new("Cutaway") {
                        if let Ok(c) = out.connect(&p, "Cutaway out") {
                            outputs.push(c);
                        }
                    }
                }
            }
            Ok(Midi { _inputs: inputs, outputs, sent: HashMap::new(), shared })
        }

        /// Light pads: (message, value). Only changes are sent.
        pub fn feedback(&mut self, lights: &[(Msg, u8)]) {
            for (m, v) in lights {
                if self.sent.get(m) == Some(v) {
                    continue;
                }
                self.sent.insert(*m, *v);
                for o in &mut self.outputs {
                    let _ = o.send(&m.bytes(*v));
                }
            }
        }
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod imp {
    use super::*;

    pub struct Midi {
        pub shared: Arc<MidiShared>,
    }

    pub fn port_names() -> Vec<String> {
        Vec::new()
    }

    impl Midi {
        pub fn start(_cfg: &MidiConfig, _requests: Sender<Request>, _waker: Waker) -> anyhow::Result<Midi> {
            anyhow::bail!("MIDI controllers aren't available in this build (macOS and Windows only)")
        }
        pub fn feedback(&mut self, _lights: &[(Msg, u8)]) {}
    }
}

pub use imp::{port_names, Midi};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_and_mappings() {
        assert_eq!(Msg::parse("note 36"), Some(Msg::Note { ch: 0, note: 36 }));
        assert_eq!(Msg::parse("CC 20 ch 10"), Some(Msg::Cc { ch: 9, cc: 20 }));
        assert_eq!(Msg::parse("note 36 ch 10").unwrap().describe(), "note 36 ch 10");
        assert_eq!(Msg::from_bytes(&[0x99, 36, 100]), Some((Msg::Note { ch: 9, note: 36 }, 100)));
        assert_eq!(Msg::from_bytes(&[0x80, 36, 64]), Some((Msg::Note { ch: 0, note: 36 }, 0)));
        let m = MidiMapping { message: "note 36".into(), action: "scene:2".into() };
        assert_eq!(action_for(&m, Msg::Note { ch: 0, note: 36 }, 90), Some(Action::parse("scene:2").unwrap()));
        assert_eq!(action_for(&m, Msg::Note { ch: 0, note: 36 }, 0), None, "note off does nothing");
        let f = MidiMapping { message: "cc 7".into(), action: "fader:Mic".into() };
        assert_eq!(action_for(&f, Msg::Cc { ch: 0, cc: 7 }, 100), Some(Action::parse("gain:Mic:0").unwrap()));
        assert_eq!(action_for(&f, Msg::Cc { ch: 0, cc: 7 }, 127), Some(Action::parse("gain:Mic:12").unwrap()));
    }
}
