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
    pub inputs: Vec<InputState>,
    pub players: Vec<PlayerState>,
    pub disk: String,
    pub problems: Vec<String>,
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
