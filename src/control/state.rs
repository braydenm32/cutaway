//! A snapshot of the show for controllers: the phone remote and API
//! (as JSON), the Stream Deck plugin (over WebSocket), OSC feedback and
//! MIDI pad lights all read the same thing.

use serde::Serialize;

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct ShowState {
    pub version: String,
    pub recording: bool,
    pub rec_seconds: f64,
    pub live: bool,
    pub live_seconds: f64,
    /// Scene on air.
    pub program: String,
    /// Scene in preview (preview mode).
    pub preview: Option<String>,
    pub preview_mode: bool,
    /// cut, mix, wipe, push or dip.
    pub transition: String,
    pub auto: bool,
    /// Live captions on (and whether they can be turned on at all).
    pub captions: bool,
    pub captions_available: bool,
    /// The latest caption text (for prompters and remotes).
    pub caption_text: String,
    pub markers: usize,
    pub scenes: Vec<SceneState>,
    pub sources: Vec<SourceState>,
    pub graphics: Vec<GraphicState>,
    /// Graphics taking turns.
    pub rotations: Vec<RotationState>,
    pub inputs: Vec<InputState>,
    pub players: Vec<PlayerState>,
    pub disk: String,
    pub problems: Vec<String>,
    pub pads: Vec<PadInfo>,
    /// Syrinscape buttons (empty when Syrinscape control is off).
    pub syrinscape: Vec<SyrinInfo>,
    /// The producer's mic is in the presenters' ears (and there is one).
    pub talkback: bool,
    pub talkback_available: bool,
    /// What the Clip button saves.
    pub clip_seconds: u32,
    /// Clock graphics and what they show.
    pub clocks: Vec<ClockState>,
    pub rundown: Option<RundownState>,
    /// Remote guest spots.
    pub guests: Vec<GuestState>,
    /// The program is faded (or fading) to black.
    pub black: bool,
    /// Macro buttons.
    pub macros: Vec<MacroInfo>,
    /// The teleprompter (when it has a script).
    pub prompter: Option<PrompterState>,
    /// An instant replay is on.
    pub replaying: bool,
    /// PTZ cameras (real and virtual) and their presets.
    pub ptz: Vec<PtzState>,
    /// Viewer comments, while they're on.
    pub comments: Option<CommentsState>,
    /// The take is being finished and saved (null otherwise).
    pub saving: Option<SavingState>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct SavingState {
    /// What it's doing now.
    pub step: String,
    /// 0 to 100.
    pub percent: u32,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct PtzState {
    /// The camera's source name.
    pub name: String,
    /// Its source number (from 1).
    pub source: usize,
    pub presets: Vec<String>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct CommentsState {
    /// One is on screen.
    pub showing: bool,
    /// Comments not shown yet.
    pub waiting: usize,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct PrompterState {
    /// Changes with the script (fetch /api/prompter).
    pub version: u64,
    pub playing: bool,
    /// Characters into the script.
    pub position: f64,
    /// Characters a second while playing.
    pub speed: f64,
    pub words_per_minute: f32,
    pub mirror: bool,
    pub text_size: f32,
    pub text_color: String,
    pub background: String,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct MacroInfo {
    pub name: String,
    /// "#RRGGBB"
    pub color: String,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct GuestState {
    /// The spot (its source's name).
    pub name: String,
    /// Its source number (from 1).
    pub source: usize,
    /// The name the guest gave.
    pub guest_name: String,
    pub connected: bool,
    pub connecting: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct PadInfo {
    pub name: String,
    /// "#RRGGBB"
    pub color: String,
    pub playing: bool,
    /// 0 to 1 while playing.
    pub progress: f32,
    pub remaining: f32,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct SyrinInfo {
    pub name: String,
    /// "mood", "element" or "stopall".
    pub kind: String,
    pub playing: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct ClockState {
    /// The graphic's name.
    pub name: String,
    pub text: String,
    /// Shown on the program.
    pub on: bool,
    /// A countdown or stopwatch (started and stopped by buttons).
    pub timer: bool,
    pub running: bool,
    pub over: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct RundownState {
    /// Segment number from 1 (0: not started).
    pub number: usize,
    pub count: usize,
    pub name: String,
    pub next: Option<String>,
    pub elapsed: f64,
    /// Seconds planned (0: open-ended).
    pub planned: f64,
    /// "4:59", or "-0:12" when it's over.
    pub left: String,
    pub over: bool,
    /// Seconds the show is over (+) or under (-) plan.
    pub drift: f64,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct SceneState {
    pub name: String,
    pub program: bool,
    pub preview: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct SourceState {
    pub name: String,
    /// "program", "preview" or "off".
    pub tally: String,
    pub offline: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct RotationState {
    pub name: String,
    pub running: bool,
    /// The graphic on screen because of it.
    pub showing: Option<String>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct GraphicState {
    pub name: String,
    pub on: bool,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct InputState {
    pub name: String,
    pub on: bool,
    /// Loudest channel, dBFS.
    pub level: f32,
    pub gain_db: f64,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct PlayerState {
    pub name: String,
    /// Source number (from 1).
    pub source: usize,
    pub playing: bool,
    pub item: String,
    pub position: f64,
    pub duration: f64,
}

impl ShowState {
    pub fn tally(&self, source: usize) -> &str {
        self.sources.get(source).map(|s| s.tally.as_str()).unwrap_or("off")
    }
}

/// "01:02:03".
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}
