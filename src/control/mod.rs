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
//! pad:1  pad:Applause        a sound pad (press: plays, or stops if playing)
//! pad:1:play / stop / release           pads:stop    every pad
//! talkback  talkback:on  talkback:off   the producer's mic to presenters
//! syrinscape:2  syrinscape:Tavern       a Syrinscape button (by number or name)
//! syrinscape:mood:19  syrinscape:element:1541818  syrinscape:stop  syrinscape:volume:0.8
//! clip  clip:30              save the last 30 seconds of the recording as a clip
//! timer:Countdown:start / pause / toggle / reset / stop    a clock graphic's timer
//! rundown:next  rundown:previous  rundown:start  rundown:stop  rundown:3  rundown:Interview
//! ```

pub mod keys;
pub mod midi;
pub mod osc;
pub mod profiles;
pub mod remote;
pub mod rundown;
pub mod state;
pub mod syrinscape;
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

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::Index(i) => write!(f, "{}", i + 1),
            Target::Name(n) => write!(f, "{n}"),
        }
    }
}

impl Target {
    pub fn parse(s: &str) -> Target {
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
pub enum PadCmd {
    /// Like pressing the pad: plays, or stops it if it's playing.
    Press,
    /// Let go (hold pads stop).
    Release,
    Play,
    Stop,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TimerCmd {
    Start,
    Pause,
    Toggle,
    /// Back to the start.
    Reset,
    /// Pause and go back to the start.
    Stop,
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
    Pad(Target, PadCmd),
    /// Stop every pad.
    PadsStop,
    /// The producer's mic in the presenters' (and guests') ears.
    Talkback(Toggle),
    Syrinscape(syrinscape::SyrinCmd),
    /// Save the last N seconds (None: as set) as a clip.
    Clip(Option<u32>),
    /// A clock graphic's countdown or stopwatch.
    Timer(Target, TimerCmd),
    Rundown(rundown::RundownCmd),
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
            "pad" | "sound" => {
                let r = need(rest, &verb)?;
                let (target, cmd) = match r.rsplit_once(':') {
                    Some((a, b)) => match b.trim().to_lowercase().as_str() {
                        "play" | "start" => (a, PadCmd::Play),
                        "stop" => (a, PadCmd::Stop),
                        "release" | "up" => (a, PadCmd::Release),
                        "press" | "toggle" | "down" => (a, PadCmd::Press),
                        _ => (r, PadCmd::Press),
                    },
                    None => (r, PadCmd::Press),
                };
                if target.trim().eq_ignore_ascii_case("all") && cmd == PadCmd::Stop {
                    Action::PadsStop
                } else {
                    Action::Pad(Target::parse(target), cmd)
                }
            }
            "pads" if rest.is_some_and(|r| r.trim().eq_ignore_ascii_case("stop")) => Action::PadsStop,
            "talkback" | "tb" => Action::Talkback(toggle(rest)?),
            "syrinscape" | "syrin" => Action::Syrinscape(syrinscape::SyrinCmd::parse(need(rest, &verb)?)?),
            "clip" => match rest.map(str::trim).filter(|r| !r.is_empty()) {
                None => Action::Clip(None),
                Some(r) => Action::Clip(Some(r.trim_end_matches('s').trim().parse::<u32>().map_err(|_| format!("\"{r}\" isn't a number of seconds"))?.clamp(1, 600))),
            },
            "timer" | "countdown" | "stopwatch" => {
                let r = need(rest, &verb)?;
                let (target, cmd) = match r.rsplit_once(':') {
                    Some((a, b)) => match b.trim().to_lowercase().as_str() {
                        "start" | "go" | "play" => (a, TimerCmd::Start),
                        "pause" | "hold" => (a, TimerCmd::Pause),
                        "toggle" => (a, TimerCmd::Toggle),
                        "reset" | "restart" => (a, TimerCmd::Reset),
                        "stop" => (a, TimerCmd::Stop),
                        other => return Err(format!("unknown timer command \"{other}\" (start, pause, toggle, reset or stop)")),
                    },
                    None => (r, TimerCmd::Toggle),
                };
                Action::Timer(Target::parse(target), cmd)
            }
            "rundown" | "segment" => Action::Rundown(rundown::RundownCmd::parse(rest.unwrap_or("next"))),
            other => return Err(format!("unknown command \"{other}\"")),
        })
    }
}

impl Action {
    /// In words, for lists of cues.
    pub fn describe(&self) -> String {
        let t = |x: &Toggle, on: &str, off: &str, both: &str| match x {
            Toggle::On => on.to_string(),
            Toggle::Off => off.to_string(),
            Toggle::Toggle => both.to_string(),
        };
        match self {
            Action::Scene(x) => format!("Take scene {x}"),
            Action::CutTo(x) => format!("Cut to scene {x}"),
            Action::Preview(x) => format!("Preview scene {x}"),
            Action::Take => "Take".into(),
            Action::Cut => "Cut".into(),
            Action::Transition(m) => format!("Transition: {}", m.label()),
            Action::Record(x) => t(x, "Start recording", "Stop recording", "Record on/off"),
            Action::Live(x) => t(x, "Go live", "End the stream", "Live on/off"),
            Action::Marker(n) => n.as_ref().map(|n| format!("Marker \"{n}\"")).unwrap_or_else(|| "Add a marker".into()),
            Action::Auto(x) => t(x, "Automatic switching on", "Automatic switching off", "Automatic switching on/off"),
            Action::Graphic(g, x) => format!("{} {g}", t(x, "Show", "Hide", "Show/hide")),
            Action::Audio(a, x) => format!("{} {a}", t(x, "Turn on", "Turn off", "Turn on/off")),
            Action::Gain(a, db) => format!("{a} to {db:+.1} dB"),
            Action::Media(s, c) => format!("{s}: {}", format!("{c:?}").to_lowercase()),
            Action::Source(s) => format!("Source {s} full screen"),
            Action::Captions(x) => t(x, "Captions on", "Captions off", "Captions on/off"),
            Action::Pad(p, c) => match c {
                PadCmd::Press => format!("Pad {p}"),
                PadCmd::Play => format!("Play pad {p}"),
                PadCmd::Stop => format!("Stop pad {p}"),
                PadCmd::Release => format!("Release pad {p}"),
            },
            Action::PadsStop => "Stop every pad".into(),
            Action::Talkback(x) => t(x, "Talkback on", "Talkback off", "Talkback on/off"),
            Action::Syrinscape(c) => match c {
                syrinscape::SyrinCmd::Button(b) => format!("Syrinscape {b}"),
                syrinscape::SyrinCmd::StopButton(b) => format!("Syrinscape: stop {b}"),
                syrinscape::SyrinCmd::Play(k, id) => format!("Syrinscape: play {k:?} {id}").to_lowercase(),
                syrinscape::SyrinCmd::Stop(k, id) => format!("Syrinscape: stop {k:?} {id}").to_lowercase(),
                syrinscape::SyrinCmd::StopAll => "Syrinscape: stop everything".into(),
                syrinscape::SyrinCmd::Volume(v) => format!("Syrinscape volume {:.0}%", v * 100.0),
            },
            Action::Clip(s) => s.map(|s| format!("Save a {s} s clip")).unwrap_or_else(|| "Save a clip".into()),
            Action::Timer(c, cmd) => format!("{c}: {}", format!("{cmd:?}").to_lowercase()),
            Action::Rundown(c) => match c {
                rundown::RundownCmd::Start => "Rundown: start".into(),
                rundown::RundownCmd::Next => "Rundown: next".into(),
                rundown::RundownCmd::Previous => "Rundown: previous".into(),
                rundown::RundownCmd::Stop => "Rundown: stop".into(),
                rundown::RundownCmd::Go(s) => format!("Rundown: go to {s}"),
            },
        }
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
            ("clip", "Save a clip (the last few seconds)"),
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
    for g in cfg.graphics.iter().filter(|g| g.kind == crate::config::GraphicKind::Clock && g.clock.is_timer()) {
        v.push((format!("timer:{}:toggle", g.name), format!("Timer start/pause: {}", g.name)));
        v.push((format!("timer:{}:reset", g.name), format!("Timer reset: {}", g.name)));
    }
    if !cfg.rundown.segments.is_empty() {
        v.push(("rundown:next".into(), "Rundown: next segment".into()));
        v.push(("rundown:previous".into(), "Rundown: previous segment".into()));
        v.push(("rundown:stop".into(), "Rundown: stop".into()));
        for (i, s) in cfg.rundown.segments.iter().enumerate() {
            v.push((format!("rundown:{}", i + 1), format!("Rundown {}: {}", i + 1, s.name)));
        }
    }
    for a in &cfg.audio {
        v.push((format!("audio:{}", a.name), format!("Audio on/off: {}", a.name)));
    }
    for (i, s) in cfg.sources.iter().enumerate() {
        v.push((format!("source:{}", i + 1), format!("Source {}: {} full screen", i + 1, s.name)));
    }
    for (i, p) in cfg.pads.iter().enumerate() {
        v.push((format!("pad:{}", i + 1), format!("Pad {}: {}", i + 1, p.name)));
    }
    if !cfg.pads.is_empty() {
        v.push(("pads:stop".into(), "Stop every pad".into()));
    }
    if cfg.syrinscape.enabled {
        for (i, b) in cfg.syrinscape.buttons.iter().enumerate() {
            v.push((format!("syrinscape:{}", i + 1), format!("Syrinscape {}: {}", i + 1, b.name)));
        }
        v.push(("syrinscape:stop".into(), "Syrinscape: stop everything".into()));
    }
    if !cfg.mix.talkback_input.trim().is_empty() {
        v.push(("talkback".into(), "Talkback on/off".into()));
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
        assert_eq!(Action::parse("pad:2"), Ok(Action::Pad(Target::Index(1), PadCmd::Press)));
        assert_eq!(Action::parse("pad:Applause:stop"), Ok(Action::Pad(Target::Name("Applause".into()), PadCmd::Stop)));
        assert_eq!(Action::parse("pad:Ding: dong"), Ok(Action::Pad(Target::Name("Ding: dong".into()), PadCmd::Press)));
        assert_eq!(Action::parse("pads:stop"), Ok(Action::PadsStop));
        assert_eq!(Action::parse("pad:all:stop"), Ok(Action::PadsStop));
        assert_eq!(Action::parse("talkback:on"), Ok(Action::Talkback(Toggle::On)));
        assert_eq!(Action::parse("clip"), Ok(Action::Clip(None)));
        assert_eq!(Action::parse("clip:45s"), Ok(Action::Clip(Some(45))));
        assert!(Action::parse("clip:soon").is_err());
        assert_eq!(Action::parse("timer:Back in:start"), Ok(Action::Timer(Target::Name("Back in".into()), TimerCmd::Start)));
        assert_eq!(Action::parse("countdown:2"), Ok(Action::Timer(Target::Index(1), TimerCmd::Toggle)));
        assert!(Action::parse("timer:Clock:explode").is_err());
        assert_eq!(Action::parse("rundown"), Ok(Action::Rundown(rundown::RundownCmd::Next)));
        assert_eq!(Action::parse("rundown:prev"), Ok(Action::Rundown(rundown::RundownCmd::Previous)));
        assert_eq!(Action::parse("rundown:3"), Ok(Action::Rundown(rundown::RundownCmd::Go(Target::Index(2)))));
        assert_eq!(Action::parse("pad:Applause").unwrap().describe(), "Pad Applause");
        assert_eq!(Action::parse("graphic:2:on").unwrap().describe(), "Show 2");
        assert_eq!(Action::parse("timer:Back in:start").unwrap().describe(), "Back in: start");
        assert!(Action::parse("explode").is_err());
        assert_eq!(fader_db(0.0), -60.0);
        assert!((fader_db(100.0 / 127.0)).abs() < 1e-9);
        assert!((fader_db(1.0) - 12.0).abs() < 1e-9);
        assert!(Action::parse("scene:").is_err());
        assert_eq!(Target::Name("host".into()).find(&["Wide".into(), "Host".into()]), Some(1));
    }
}
