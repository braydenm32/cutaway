//! Remote control: every button on the desk as a short text command, so
//! MIDI controllers, the phone/tablet remote, Stream Deck (via its website
//! or HTTP plugins), Bitfocus Companion and scripts can all drive Cutaway.
//!
//! Commands (case-insensitive; numbers count from 1):
//!
//! ```text
//! scene:3  scene:Host        take a scene (with the current transition)
//! cut:3                      take a scene with a cut
//! preview:3                  put a scene in preview      take / cut   preview to program
//! transition:mix             cut, mix, wipe, push or dip
//! record  record:on  record:off       live  live:on  live:off
//! marker  marker:Big moment
//! auto  auto:on  auto:off    automatic switching
//! graphic:Logo  graphic:2:on  graphic:2:off
//! mute:Mic  unmute:Mic  audio:Mic (toggle)   gain:Mic:-6
//! media:Clips:play / pause / toggle / restart / next / previous
//! source:2                   source 2 full screen (to preview in preview mode)
//! captions  captions:on  captions:off   live captions on screen
//! ```

pub mod keys;
pub mod midi;
pub mod osc;
pub mod profiles;
pub mod remote;
pub mod state;
pub mod ws;

use crate::config::{Config, TransitionMode};
use crossbeam_channel::Sender;

#[derive(Clone, Debug, PartialEq)]
pub enum Toggle {
    On,
    Off,
    Toggle,
}

/// A scene, source, graphic or input: by number (from 1) or by name.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Index(usize),
    Name(String),
}

impl Target {
    fn parse(s: &str) -> Target {
        match s.trim().parse::<usize>() {
            Ok(n) if n >= 1 => Target::Index(n - 1),
            _ => Target::Name(s.trim().to_string()),
        }
    }

    /// Position in a list of names.
    pub fn find(&self, names: &[String]) -> Option<usize> {
        match self {
            Target::Index(i) => (*i < names.len()).then_some(*i),
            Target::Name(n) => names.iter().position(|x| x.eq_ignore_ascii_case(n)),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MediaCmd {
    Play,
    Pause,
    Toggle,
    Restart,
    Next,
    Previous,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Scene(Target),
    CutTo(Target),
    Preview(Target),
    /// Preview to program with the transition.
    Take,
    /// Preview to program, instantly.
    Cut,
    Transition(TransitionMode),
    Record(Toggle),
    Live(Toggle),
    Marker(Option<String>),
    Auto(Toggle),
    Graphic(Target, Toggle),
    Audio(Target, Toggle),
    /// Input level in dB (MIDI faders).
    Gain(Target, f64),
    Media(Target, MediaCmd),
    /// A source full screen, like clicking it in the multiview.
    Source(Target),
    /// Live captions on screen.
    Captions(Toggle),
}

/// A fader position (0..1) as a level: 0 is silence (-60 dB), 0.79 is
/// 0 dB, the top is +12 dB.
pub fn fader_db(position: f64) -> f64 {
    let v = position.clamp(0.0, 1.0) * 127.0;
    if v < 0.5 {
        -60.0
    } else if v <= 100.0 {
        -40.0 + 40.0 * v / 100.0
    } else {
        12.0 * (v - 100.0) / 27.0
    }
}

fn toggle(s: Option<&str>) -> Result<Toggle, String> {
    match s.map(|s| s.trim().to_lowercase()).as_deref() {
        None | Some("") | Some("toggle") => Ok(Toggle::Toggle),
        Some("on") | Some("start") | Some("1") | Some("true") => Ok(Toggle::On),
        Some("off") | Some("stop") | Some("0") | Some("false") => Ok(Toggle::Off),
        Some(o) => Err(format!("\"{o}\" should be on, off or toggle")),
    }
}

impl Action {
    pub fn parse(s: &str) -> Result<Action, String> {
        let s = s.trim();
        let (verb, rest) = s.split_once(':').map(|(a, b)| (a, Some(b))).unwrap_or((s, None));
        let verb = verb.trim().to_lowercase();
        fn need<'a>(r: Option<&'a str>, verb: &str) -> Result<&'a str, String> {
            r.filter(|x| !x.trim().is_empty()).ok_or_else(|| format!("\"{verb}\" needs something after the colon"))
        }
        Ok(match verb.as_str() {
            "scene" | "take" if rest.is_some() => Action::Scene(Target::parse(need(rest, &verb)?)),
            "take" => Action::Take,
            "cut" if rest.is_some() => Action::CutTo(Target::parse(need(rest, &verb)?)),
            "cut" => Action::Cut,
            "preview" | "pvw" => Action::Preview(Target::parse(need(rest, &verb)?)),
            "transition" => {
                let m = need(rest, &verb)?.trim().to_lowercase();
                Action::Transition(TransitionMode::ALL.into_iter().find(|t| t.name() == m).ok_or_else(|| format!("unknown transition \"{m}\""))?)
            }
            "record" | "rec" => Action::Record(toggle(rest)?),
            "live" | "stream" => Action::Live(toggle(rest)?),
            "marker" | "mark" => Action::Marker(rest.map(|r| r.trim().to_string()).filter(|r| !r.is_empty())),
            "auto" | "autoswitch" => Action::Auto(toggle(rest)?),
            "graphic" | "gfx" | "overlay" => {
                let r = need(rest, &verb)?;
                let (target, t) = match r.rsplit_once(':') {
                    Some((a, b)) if toggle(Some(b)).is_ok() => (a, Some(b)),
                    _ => (r, None),
                };
                Action::Graphic(Target::parse(target), toggle(t)?)
            }
            "mute" => Action::Audio(Target::parse(need(rest, &verb)?), Toggle::Off),
            "unmute" => Action::Audio(Target::parse(need(rest, &verb)?), Toggle::On),
            "audio" | "input" => {
                let r = need(rest, &verb)?;
                let (target, t) = match r.rsplit_once(':') {
                    Some((a, b)) if toggle(Some(b)).is_ok() => (a, Some(b)),
                    _ => (r, None),
                };
                Action::Audio(Target::parse(target), toggle(t)?)
            }
            "gain" | "fader" => {
                let r = need(rest, &verb)?;
                let (target, db) = r.rsplit_once(':').ok_or("gain needs an input and a level, e.g. gain:Mic:-6")?;
                let db: f64 = db.trim().trim_end_matches("dB").trim().parse().map_err(|_| format!("\"{db}\" isn't a level in dB"))?;
                Action::Gain(Target::parse(target), db.clamp(-60.0, 12.0))
            }
            "media" | "player" => {
                let r = need(rest, &verb)?;
                let (target, cmd) = r.rsplit_once(':').ok_or("media needs a source and a command, e.g. media:Clips:play")?;
                let cmd = match cmd.trim().to_lowercase().as_str() {
                    "play" => MediaCmd::Play,
                    "pause" => MediaCmd::Pause,
                    "toggle" | "playpause" => MediaCmd::Toggle,
                    "restart" | "start" => MediaCmd::Restart,
                    "next" => MediaCmd::Next,
                    "previous" | "prev" => MediaCmd::Previous,
                    other => return Err(format!("unknown media command \"{other}\"")),
                };
                Action::Media(Target::parse(target), cmd)
            }
            "source" | "camera" => Action::Source(Target::parse(need(rest, &verb)?)),
            "captions" | "cc" | "subtitles" => Action::Captions(toggle(rest)?),
            other => return Err(format!("unknown command \"{other}\"")),
        })
    }
}

/// A request from a controller, with somewhere to send the result.
pub struct Request {
    pub action: Action,
    pub reply: Option<Sender<Result<String, String>>>,
}

/// Commands worth offering in pickers (MIDI learn, the remote), as
/// (command, label).
pub fn suggestions(cfg: &Config) -> Vec<(String, String)> {
    let mut v = Vec::new();
    for (i, s) in cfg.effective_scenes().iter().enumerate() {
        v.push((format!("scene:{}", i + 1), format!("Scene {}: {}", i + 1, s.name)));
    }
    for (i, s) in cfg.effective_scenes().iter().enumerate() {
        v.push((format!("preview:{}", i + 1), format!("Preview {}: {}", i + 1, s.name)));
    }
    v.extend(
        [
            ("take", "Take (preview to program)"),
            ("cut", "Cut (preview to program)"),
            ("record", "Record on/off"),
            ("live", "Go live / end stream"),
            ("marker", "Add a marker"),
            ("auto", "Automatic switching on/off"),
            ("captions", "Captions on/off"),
            ("transition:cut", "Transition: cut"),
            ("transition:mix", "Transition: mix"),
            ("transition:wipe", "Transition: wipe"),
            ("transition:push", "Transition: push"),
            ("transition:dip", "Transition: dip"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string())),
    );
    for g in &cfg.graphics {
        v.push((format!("graphic:{}", g.name), format!("Graphic on/off: {}", g.name)));
    }
    for a in &cfg.audio {
        v.push((format!("audio:{}", a.name), format!("Audio on/off: {}", a.name)));
    }
    for (i, s) in cfg.sources.iter().enumerate() {
        v.push((format!("source:{}", i + 1), format!("Source {}: {} full screen", i + 1, s.name)));
    }
    for s in cfg.sources.iter().filter(|s| s.device.trim().eq_ignore_ascii_case("media")) {
        for (c, l) in [("toggle", "play/pause"), ("restart", "restart"), ("next", "next")] {
            v.push((format!("media:{}:{c}", s.name), format!("{}: {l}", s.name)));
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_commands() {
        assert_eq!(Action::parse("scene:3"), Ok(Action::Scene(Target::Index(2))));
        assert_eq!(Action::parse("Scene: Host"), Ok(Action::Scene(Target::Name("Host".into()))));
        assert_eq!(Action::parse("take"), Ok(Action::Take));
        assert_eq!(Action::parse("cut:2"), Ok(Action::CutTo(Target::Index(1))));
        assert_eq!(Action::parse("record:off"), Ok(Action::Record(Toggle::Off)));
        assert_eq!(Action::parse("live"), Ok(Action::Live(Toggle::Toggle)));
        assert_eq!(Action::parse("marker:Big moment"), Ok(Action::Marker(Some("Big moment".into()))));
        assert_eq!(Action::parse("graphic:Lower third:on"), Ok(Action::Graphic(Target::Name("Lower third".into()), Toggle::On)));
        assert_eq!(Action::parse("graphic:2"), Ok(Action::Graphic(Target::Index(1), Toggle::Toggle)));
        assert_eq!(Action::parse("mute:Mic 1"), Ok(Action::Audio(Target::Name("Mic 1".into()), Toggle::Off)));
        assert_eq!(Action::parse("media:Clips:next"), Ok(Action::Media(Target::Name("Clips".into()), MediaCmd::Next)));
        assert_eq!(Action::parse("transition:wipe"), Ok(Action::Transition(TransitionMode::Wipe)));
        assert_eq!(Action::parse("gain:Mic:-6"), Ok(Action::Gain(Target::Name("Mic".into()), -6.0)));
        assert_eq!(Action::parse("source:2"), Ok(Action::Source(Target::Index(1))));
        assert_eq!(Action::parse("cc:off"), Ok(Action::Captions(Toggle::Off)));
        assert!(Action::parse("explode").is_err());
        assert_eq!(fader_db(0.0), -60.0);
        assert!((fader_db(100.0 / 127.0)).abs() < 1e-9);
        assert!((fader_db(1.0) - 12.0).abs() < 1e-9);
        assert!(Action::parse("scene:").is_err());
        assert_eq!(Target::Name("host".into()).find(&["Wide".into(), "Host".into()]), Some(1));
    }
}
