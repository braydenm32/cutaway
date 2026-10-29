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
//! back                       back to the scene on air before this one
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
//! ftb  ftb:on  ftb:off       fade to black (and back)
//! ptz:Cam 2:preset:3  ptz:Cam 2:preset:Wide  ptz:Cam 2:store:3  ptz:Cam 2:home
//! ptz:Cam 2:left / right / up / down / in / out (:0.5 = half speed)  ptz:Cam 2:stop
//! replay  replay:8  replay:8:slow  replay:stop      the last seconds again
//! macro:Intro  macro:2       run a macro          wait:1.5   (in macros and cues)
//! prompter:play / pause / toggle / faster / slower / top / back / forward
//! comment:next  comment:hide  comment:toggle        viewer comments on screen
//! ```

pub mod keys;
pub mod midi;
pub mod osc;
pub mod profiles;
pub mod remote;
pub mod rundown;
pub mod shortcuts;
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

    /// Position in a list of names. A number is a position, unless
    /// something is named that number ("scene:2" with a scene called "2").
    pub fn find(&self, names: &[String]) -> Option<usize> {
        match self {
            Target::Index(i) => names.iter().position(|x| x.trim() == (i + 1).to_string()).or((*i < names.len()).then_some(*i)),
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

/// Moving a PTZ camera (real or virtual).
#[derive(Clone, Debug, PartialEq)]
pub enum PtzCmd {
    /// Move at these speeds (-1..1: right and up are positive) until Stop.
    Drive { pan: f32, tilt: f32 },
    /// Zoom at this speed (-1 out .. 1 in) until Stop.
    Zoom(f32),
    Stop,
    Home,
    Recall(Target),
    Store(Target),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ReplayCmd {
    /// Play the last `seconds` (None: as set), in slow motion or not, from
    /// the program or a camera (None: as set).
    Play { seconds: Option<f32>, slow: bool, angle: Option<ReplayAngle> },
    Stop,
}

/// What a replay shows.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ReplayAngle {
    /// The program, as it went out.
    #[default]
    Program,
    /// One source, from its own recording.
    Source(Target),
    /// Up to four sources at once, in a grid.
    Quad,
}

impl ReplayAngle {
    /// From settings or a command word: "" or "program", "quad", or a source.
    pub fn parse(s: &str) -> ReplayAngle {
        match s.trim().to_lowercase().as_str() {
            "" | "program" | "pgm" => ReplayAngle::Program,
            "quad" | "all" | "angles" | "every angle" | "grid" => ReplayAngle::Quad,
            _ => ReplayAngle::Source(Target::Name(s.trim().to_string())),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PrompterCmd {
    Play,
    Pause,
    Toggle,
    Faster,
    Slower,
    /// Back to the start.
    Top,
    /// A screenful back / forward.
    Back,
    Forward,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CommentCmd {
    /// Show the next comment waiting (oldest first).
    Next,
    Hide,
    /// Hide the one showing, or show the next.
    Toggle,
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
    /// Back to the scene on air before this one.
    Back,
    Transition(TransitionMode),
    Record(Toggle),
    Live(Toggle),
    Marker(Option<String>),
    Auto(Toggle),
    Graphic(Target, Toggle),
    /// Start or stop graphics taking turns.
    Rotation(Target, Toggle),
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
    /// Fade the program to black and back.
    Ftb(Toggle),
    Ptz(Target, PtzCmd),
    Replay(ReplayCmd),
    Macro(Target),
    /// Pause a macro or a list of cues (seconds).
    Wait(f32),
    Prompter(PrompterCmd),
    Comment(CommentCmd),
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
            "back" | "last" => Action::Back,
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
            "rotation" | "rotate" | "sponsors" => {
                let r = need(rest, &verb)?;
                let (target, t) = match r.rsplit_once(':') {
                    Some((a, b)) if toggle(Some(b)).is_ok() => (a, Some(b)),
                    _ => (r, None),
                };
                Action::Rotation(Target::parse(target), toggle(t)?)
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
            "ftb" | "black" | "fadetoblack" => Action::Ftb(toggle(rest)?),
            "ptz" => {
                let r = need(rest, &verb)?;
                let parts: Vec<&str> = r.split(':').map(str::trim).collect();
                let num = |s: Option<&&str>| -> Result<f32, String> {
                    match s {
                        None => Ok(1.0),
                        Some(v) => v.parse::<f32>().map(|x| x.clamp(0.05, 1.0)).map_err(|_| format!("\"{v}\" isn't a speed (0.1 to 1)")),
                    }
                };
                // The camera can have colons in its name: find the command word.
                let words = ["preset", "recall", "store", "save", "home", "left", "right", "up", "down", "in", "out", "zoomin", "zoomout", "stop"];
                let k = parts.iter().position(|p| words.contains(&p.to_lowercase().as_str())).filter(|&k| k > 0).ok_or("ptz needs a camera and a move, e.g. ptz:Cam 2:preset:1")?;
                let cam = Target::parse(&parts[..k].join(":"));
                let arg = parts.get(k + 1);
                let cmd = match parts[k].to_lowercase().as_str() {
                    "preset" | "recall" => PtzCmd::Recall(Target::parse(arg.ok_or("which preset?")?)),
                    "store" | "save" => PtzCmd::Store(Target::parse(arg.ok_or("which preset?")?)),
                    "home" => PtzCmd::Home,
                    "left" => PtzCmd::Drive { pan: -num(arg)?, tilt: 0.0 },
                    "right" => PtzCmd::Drive { pan: num(arg)?, tilt: 0.0 },
                    "up" => PtzCmd::Drive { pan: 0.0, tilt: num(arg)? },
                    "down" => PtzCmd::Drive { pan: 0.0, tilt: -num(arg)? },
                    "in" | "zoomin" => PtzCmd::Zoom(num(arg)?),
                    "out" | "zoomout" => PtzCmd::Zoom(-num(arg)?),
                    _ => PtzCmd::Stop,
                };
                Action::Ptz(cam, cmd)
            }
            "replay" => {
                let mut seconds = None;
                let mut slow = false;
                let mut angle = None;
                for part in rest.unwrap_or("").split(':').map(str::trim).filter(|p| !p.is_empty()) {
                    let p = part.to_lowercase();
                    match p.as_str() {
                        "stop" | "off" | "end" => return Ok(Action::Replay(ReplayCmd::Stop)),
                        "slow" | "slowmo" | "slow-mo" => slow = true,
                        "play" | "normal" => {}
                        n => match n.trim_end_matches('s').parse::<f32>() {
                            Ok(v) => seconds = Some(v.clamp(1.0, 60.0)),
                            // Anything else is the angle: a source's name, or "quad".
                            Err(_) => angle = Some(ReplayAngle::parse(part)),
                        },
                    }
                }
                Action::Replay(ReplayCmd::Play { seconds, slow, angle })
            }
            "macro" => Action::Macro(Target::parse(need(rest, &verb)?)),
            "wait" | "pause" | "delay" => {
                let r = need(rest, &verb)?.trim();
                let secs = if let Some(ms) = r.strip_suffix("ms") { ms.trim().parse::<f32>().map(|v| v / 1000.0) } else { r.trim_end_matches('s').trim().parse::<f32>() };
                Action::Wait(secs.map_err(|_| format!("\"{r}\" isn't a number of seconds"))?.clamp(0.0, 600.0))
            }
            "prompter" | "teleprompter" => Action::Prompter(match rest.unwrap_or("toggle").trim().to_lowercase().as_str() {
                "play" | "start" | "go" => PrompterCmd::Play,
                "pause" | "stop" | "hold" => PrompterCmd::Pause,
                "toggle" | "" => PrompterCmd::Toggle,
                "faster" | "up" | "+" => PrompterCmd::Faster,
                "slower" | "down" | "-" => PrompterCmd::Slower,
                "top" | "start-over" | "reset" => PrompterCmd::Top,
                "back" | "previous" | "prev" => PrompterCmd::Back,
                "forward" | "next" | "skip" => PrompterCmd::Forward,
                other => return Err(format!("unknown prompter command \"{other}\"")),
            }),
            "comment" | "comments" | "chat" => Action::Comment(match rest.unwrap_or("toggle").trim().to_lowercase().as_str() {
                "next" | "show" => CommentCmd::Next,
                "hide" | "off" | "clear" => CommentCmd::Hide,
                "toggle" | "" => CommentCmd::Toggle,
                other => return Err(format!("unknown comment command \"{other}\" (next, hide or toggle)")),
            }),
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
            Action::Back => "Back to the last scene".into(),
            Action::Transition(m) => format!("Transition: {}", m.label()),
            Action::Record(x) => t(x, "Start recording", "Stop recording", "Record on/off"),
            Action::Live(x) => t(x, "Go live", "End the stream", "Live on/off"),
            Action::Marker(n) => n.as_ref().map(|n| format!("Marker \"{n}\"")).unwrap_or_else(|| "Add a marker".into()),
            Action::Auto(x) => t(x, "Automatic switching on", "Automatic switching off", "Automatic switching on/off"),
            Action::Graphic(g, x) => format!("{} {g}", t(x, "Show", "Hide", "Show/hide")),
            Action::Rotation(r, x) => format!("{} rotation {r}", t(x, "Start", "Stop", "Start/stop")),
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
            Action::Ftb(x) => t(x, "Fade to black", "Fade up from black", "Fade to black / back"),
            Action::Ptz(c, cmd) => match cmd {
                PtzCmd::Recall(p) => format!("{c}: go to preset {p}"),
                PtzCmd::Store(p) => format!("{c}: save preset {p}"),
                PtzCmd::Home => format!("{c}: home"),
                PtzCmd::Stop => format!("{c}: stop moving"),
                PtzCmd::Zoom(z) => format!("{c}: zoom {}", if *z > 0.0 { "in" } else { "out" }),
                PtzCmd::Drive { pan, tilt } => format!(
                    "{c}: move {}",
                    if pan.abs() >= tilt.abs() {
                        if *pan > 0.0 { "right" } else { "left" }
                    } else if *tilt > 0.0 {
                        "up"
                    } else {
                        "down"
                    }
                ),
            },
            Action::Replay(r) => match r {
                ReplayCmd::Stop => "Stop the replay".into(),
                ReplayCmd::Play { seconds, slow, angle } => {
                    let secs = seconds.map(|s| format!(" the last {s:.0} s")).unwrap_or_default();
                    let from = match angle {
                        None => String::new(),
                        Some(ReplayAngle::Program) => " of the program".into(),
                        Some(ReplayAngle::Source(t)) => format!(" from {t}"),
                        Some(ReplayAngle::Quad) => " from every angle".into(),
                    };
                    format!("Replay{secs}{from}{}", if *slow { " in slow motion" } else { "" })
                }
            },
            Action::Macro(m) => format!("Macro {m}"),
            Action::Wait(s) => format!("Wait {s} s"),
            Action::Prompter(p) => format!("Prompter: {}", format!("{p:?}").to_lowercase()),
            Action::Comment(c) => match c {
                CommentCmd::Next => "Show the next comment".into(),
                CommentCmd::Hide => "Hide the comment".into(),
                CommentCmd::Toggle => "Comment on/off".into(),
            },
        }
    }
}

/// Scenes were moved, renamed or deleted (`before` → `after`, by name):
/// what the show keeps that picks a scene goes on picking the same one.
/// Commands saved in macros, MIDI mappings, your own keys and cues that
/// take a scene by number (`scene:3`) get its new number; anything that
/// names a renamed scene (`scene:Wide`, a rundown segment, "if it drops,
/// cut to", automatic switching, vertical layouts) gets its new name. A
/// deleted scene's references are left as they are. True if anything changed.
/// (Numbers from outside, like the number keys, a Stream Deck key or an OSC
/// message, mean the button in that place, so they aren't changed.)
pub fn follow_scenes(cfg: &mut Config, before: &[String], after: &[String]) -> bool {
    if before == after {
        return false;
    }
    let renamed = scene_renames(before, after);
    let new_name = |n: &str| renamed.iter().find(|(a, _)| a.eq_ignore_ascii_case(n.trim())).map(|(_, b)| b.clone());
    let new_pos = |i: usize| -> Option<usize> {
        let name = before.get(i)?;
        let name = new_name(name).unwrap_or_else(|| name.clone());
        after.iter().position(|a| *a == name)
    };
    let mut changed = false;
    let mut fix = |c: &mut String| {
        if let Some(n) = follow_command(c, before, &new_pos, &new_name) {
            *c = n;
            changed = true;
        }
    };
    for m in &mut cfg.macros {
        m.commands.iter_mut().for_each(&mut fix);
    }
    for m in &mut cfg.midi.mappings {
        fix(&mut m.action);
    }
    for k in &mut cfg.keys.custom {
        fix(&mut k.command);
    }
    for s in &mut cfg.rundown.segments {
        s.cues.iter_mut().for_each(&mut fix);
    }
    for s in &mut cfg.scenes {
        s.cues.iter_mut().for_each(&mut fix);
    }
    let mut rename = |n: &mut String| {
        if let Some(x) = new_name(n) {
            *n = x;
            changed = true;
        }
    };
    for s in &mut cfg.sources {
        rename(&mut s.on_drop_scene);
    }
    rename(&mut cfg.auto_switch.wide_scene);
    for r in &mut cfg.auto_switch.rules {
        rename(&mut r.scene);
    }
    for s in &mut cfg.rundown.segments {
        rename(&mut s.scene);
    }
    for l in &mut cfg.vertical.layouts {
        rename(&mut l.scene);
    }
    changed
}

/// Scenes renamed between two lists of the same scenes (old name, new
/// name): the same place, a name the other list doesn't have. Lists of
/// another length (a scene added or deleted) aren't renames, nor scenes
/// that only moved.
pub fn scene_renames(before: &[String], after: &[String]) -> Vec<(String, String)> {
    if before.len() != after.len() {
        return Vec::new();
    }
    before.iter().zip(after).filter(|(a, b)| a != b && !after.contains(a) && !before.contains(b)).map(|(a, b)| (a.clone(), b.clone())).collect()
}

/// A command that picks a scene, made to pick it where it is now (None:
/// it doesn't pick a scene, or still picks the same one).
fn follow_command(c: &str, before: &[String], new_pos: &dyn Fn(usize) -> Option<usize>, new_name: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let (verb, rest) = c.split_once(':')?;
    if !matches!(verb.trim().to_lowercase().as_str(), "scene" | "take" | "cut" | "preview" | "pvw") || rest.trim().is_empty() {
        return None;
    }
    match Target::parse(rest) {
        // (A number that's a scene's name is that name.)
        Target::Index(i) if !before.iter().any(|n| n.trim() == (i + 1).to_string()) => {
            let j = new_pos(i)?;
            (j != i).then(|| format!("{verb}:{}", j + 1))
        }
        _ => new_name(rest.trim()).map(|n| format!("{verb}:{n}")),
    }
}

/// What in the show picks the scene at `i` (in `names`), for "are you sure"
/// before it's deleted: "Macro Break", "MIDI note 36", "Rundown: Intro"…
pub fn scene_uses(cfg: &Config, names: &[String], i: usize) -> Vec<String> {
    let Some(name) = names.get(i) else { return Vec::new() };
    let picks = |c: &str| match Action::parse(c) {
        Ok(Action::Scene(t) | Action::CutTo(t) | Action::Preview(t)) => t.find(names) == Some(i),
        _ => false,
    };
    let mut out = Vec::new();
    for m in &cfg.macros {
        if m.commands.iter().any(|c| picks(c)) {
            out.push(format!("macro {}", m.name));
        }
    }
    for m in cfg.midi.mappings.iter().filter(|m| picks(&m.action)) {
        out.push(format!("MIDI {}", m.message));
    }
    for k in cfg.keys.custom.iter().filter(|k| picks(&k.command)) {
        out.push(format!("the {} key", k.key));
    }
    for s in &cfg.rundown.segments {
        if s.scene.eq_ignore_ascii_case(name) || s.cues.iter().any(|c| picks(c)) {
            out.push(format!("rundown segment {}", s.name));
        }
    }
    for s in cfg.sources.iter().filter(|s| s.on_drop_scene.eq_ignore_ascii_case(name)) {
        out.push(format!("\"if it drops\" on {}", s.name));
    }
    if cfg.auto_switch.wide_scene.eq_ignore_ascii_case(name) || cfg.auto_switch.rules.iter().any(|r| r.scene.eq_ignore_ascii_case(name)) {
        out.push("automatic switching".into());
    }
    out
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
            ("back", "Back to the last scene"),
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
            ("ftb", "Fade to black / back up"),
            ("replay", "Instant replay"),
            ("replay:slow", "Instant replay in slow motion"),
        ]
        .map(|(a, b)| (a.to_string(), b.to_string())),
    );
    for g in &cfg.graphics {
        v.push((format!("graphic:{}", g.name), format!("Graphic on/off: {}", g.name)));
    }
    for r in &cfg.rotations {
        v.push((format!("rotation:{}", r.name), format!("Rotation start/stop: {}", r.name)));
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
    for m in &cfg.macros {
        v.push((format!("macro:{}", m.name), format!("Macro: {}", m.name)));
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
    fn saved_commands_follow_their_scene() {
        use crate::config::{AutoRule, CustomKey, Macro, MidiMapping, Segment, SourceConfig};
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let mut cfg = Config::default();
        cfg.macros.push(Macro { name: "Break".into(), commands: vec!["scene:2".into(), "wait:1".into(), "Cut: 3".into(), "scene:Host".into()], ..Default::default() });
        cfg.midi.mappings.push(MidiMapping { message: "note 36".into(), action: "preview:1".into() });
        cfg.keys.custom.push(CustomKey { key: "G".into(), command: "scene:7".into() });
        cfg.rundown.segments.push(Segment { name: "Intro".into(), scene: "Host".into(), cues: vec!["take:2".into()], ..Default::default() });
        cfg.sources.push(SourceConfig { name: "Cam 2".into(), on_drop_scene: "Wide".into(), ..Default::default() });
        cfg.auto_switch.rules.push(AutoRule { input: "Mic".into(), channel: 1, scene: "Host".into() });
        let before = names(&["Wide", "Host", "Guest", "7"]);
        // Wide moves to the end: Host is 1, Guest 2.
        let after = names(&["Host", "Guest", "7", "Wide"]);
        assert!(follow_scenes(&mut cfg, &before, &after));
        assert_eq!(cfg.macros[0].commands, vec!["scene:1", "wait:1", "Cut:2", "scene:Host"]);
        assert_eq!(cfg.midi.mappings[0].action, "preview:4");
        assert_eq!(cfg.keys.custom[0].command, "scene:7", "a scene called 7 is picked by name");
        assert_eq!(cfg.rundown.segments[0].cues, vec!["take:1"]);
        assert_eq!(cfg.sources[0].on_drop_scene, "Wide");
        // Nothing moved: nothing changes.
        assert!(!follow_scenes(&mut cfg, &after, &after));
        // Host renamed: names follow, numbers stay.
        let renamed = names(&["Presenter", "Guest", "7", "Wide"]);
        assert!(follow_scenes(&mut cfg, &after, &renamed));
        assert_eq!(cfg.macros[0].commands[3], "scene:Presenter");
        assert_eq!(cfg.macros[0].commands[0], "scene:1");
        assert_eq!(cfg.rundown.segments[0].scene, "Presenter");
        assert_eq!(cfg.auto_switch.rules[0].scene, "Presenter");
        // Guest deleted: what pointed at it stays; what was after it moves up.
        cfg.keys.custom[0].command = "cut:2".into();
        cfg.midi.mappings[0].action = "scene:4".into();
        assert!(follow_scenes(&mut cfg, &renamed, &names(&["Presenter", "7", "Wide"])));
        assert_eq!(cfg.keys.custom[0].command, "cut:2");
        assert_eq!(cfg.midi.mappings[0].action, "scene:3");
    }

    #[test]
    fn says_what_uses_a_scene() {
        use crate::config::{Macro, SourceConfig};
        let mut cfg = Config::default();
        cfg.macros.push(Macro { name: "Break".into(), commands: vec!["scene:2".into()], ..Default::default() });
        cfg.sources.push(SourceConfig { name: "Cam 1".into(), on_drop_scene: "Host".into(), ..Default::default() });
        let names: Vec<String> = ["Wide", "Host"].iter().map(|s| s.to_string()).collect();
        assert_eq!(scene_uses(&cfg, &names, 1), vec!["macro Break", "\"if it drops\" on Cam 1"]);
        assert!(scene_uses(&cfg, &names, 0).is_empty());
    }

    #[test]
    fn parses_commands() {
        assert_eq!(Action::parse("scene:3"), Ok(Action::Scene(Target::Index(2))));
        assert_eq!(Action::parse("Scene: Host"), Ok(Action::Scene(Target::Name("Host".into()))));
        assert_eq!(Action::parse("take"), Ok(Action::Take));
        assert_eq!(Action::parse("rotation:Sponsors:off"), Ok(Action::Rotation(Target::Name("Sponsors".into()), Toggle::Off)));
        assert_eq!(Action::parse("rotation:1"), Ok(Action::Rotation(Target::Index(0), Toggle::Toggle)));
        assert_eq!(Action::parse("cut:2"), Ok(Action::CutTo(Target::Index(1))));
        assert_eq!(Action::parse("record:off"), Ok(Action::Record(Toggle::Off)));
        assert_eq!(Action::parse("live"), Ok(Action::Live(Toggle::Toggle)));
        assert_eq!(Action::parse("back"), Ok(Action::Back));
        assert_eq!(Action::parse(" Back "), Ok(Action::Back));
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
        assert_eq!(Action::parse("ftb"), Ok(Action::Ftb(Toggle::Toggle)));
        assert_eq!(Action::parse("ftb:on"), Ok(Action::Ftb(Toggle::On)));
        assert_eq!(Action::parse("ptz:Cam 2:preset:3"), Ok(Action::Ptz(Target::Name("Cam 2".into()), PtzCmd::Recall(Target::Index(2)))));
        assert_eq!(Action::parse("ptz:2:left:0.5"), Ok(Action::Ptz(Target::Index(1), PtzCmd::Drive { pan: -0.5, tilt: 0.0 })));
        assert_eq!(Action::parse("ptz:Cam: 1:up"), Ok(Action::Ptz(Target::Name("Cam:1".into()), PtzCmd::Drive { pan: 0.0, tilt: 1.0 })));
        assert_eq!(Action::parse("ptz:Cam:store:Wide"), Ok(Action::Ptz(Target::Name("Cam".into()), PtzCmd::Store(Target::Name("Wide".into())))));
        assert_eq!(Action::parse("ptz:Cam:out"), Ok(Action::Ptz(Target::Name("Cam".into()), PtzCmd::Zoom(-1.0))));
        assert!(Action::parse("ptz:preset:1").is_err());
        assert_eq!(Action::parse("replay"), Ok(Action::Replay(ReplayCmd::Play { seconds: None, slow: false, angle: None })));
        assert_eq!(Action::parse("replay:8:slow"), Ok(Action::Replay(ReplayCmd::Play { seconds: Some(8.0), slow: true, angle: None })));
        assert_eq!(Action::parse("replay:Cam 2:slow"), Ok(Action::Replay(ReplayCmd::Play { seconds: None, slow: true, angle: Some(ReplayAngle::Source(Target::Name("Cam 2".into()))) })));
        assert_eq!(Action::parse("replay:quad:5"), Ok(Action::Replay(ReplayCmd::Play { seconds: Some(5.0), slow: false, angle: Some(ReplayAngle::Quad) })));
        assert_eq!(Action::parse("replay:program"), Ok(Action::Replay(ReplayCmd::Play { seconds: None, slow: false, angle: Some(ReplayAngle::Program) })));
        assert_eq!(Action::parse("replay:stop"), Ok(Action::Replay(ReplayCmd::Stop)));
        assert_eq!(Action::parse("macro:Intro"), Ok(Action::Macro(Target::Name("Intro".into()))));
        assert_eq!(Action::parse("wait:1.5"), Ok(Action::Wait(1.5)));
        assert_eq!(Action::parse("wait:250ms"), Ok(Action::Wait(0.25)));
        assert_eq!(Action::parse("prompter:faster"), Ok(Action::Prompter(PrompterCmd::Faster)));
        assert_eq!(Action::parse("comment:next"), Ok(Action::Comment(CommentCmd::Next)));
        assert_eq!(Action::parse("ptz:Cam 2:preset:Wide").unwrap().describe(), "Cam 2: go to preset Wide");
        assert!(Action::parse("explode").is_err());
        assert_eq!(fader_db(0.0), -60.0);
        assert!((fader_db(100.0 / 127.0)).abs() < 1e-9);
        assert!((fader_db(1.0) - 12.0).abs() < 1e-9);
        assert!(Action::parse("scene:").is_err());
        assert_eq!(Target::Name("host".into()).find(&["Wide".into(), "Host".into()]), Some(1));
    }
}
