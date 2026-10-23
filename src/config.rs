//! Settings, stored as TOML. Everything here can also be changed from the UI.

use crate::scene::Scene;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
#[derive(Default)]
pub struct Config {
    pub output: OutputConfig,
    pub program: ProgramConfig,
    pub transition: TransitionConfig,
    pub sources: Vec<SourceConfig>,
    pub audio: Vec<AudioConfig>,
    pub streaming: StreamingConfig,
    pub ui: UiConfig,
    /// Your own scenes. When empty, presets are generated from the sources.
    pub scenes: Vec<Scene>,
    /// Corners, borders and shadows for frames that don't fill the screen.
    pub style: FrameStyle,
    pub mix: MixConfig,
    pub auto_switch: AutoSwitchConfig,
    /// Logos, titles and other overlays above every scene.
    pub graphics: Vec<Graphic>,
    pub display: DisplayConfig,
    pub remote: RemoteConfig,
    pub midi: MidiConfig,
    pub osc: OscConfig,
    pub podcast: PodcastConfig,
    pub captions: CaptionsConfig,
    /// Sound pads: jingles, stings and beds on keys, Stream Deck and MIDI.
    pub pads: Vec<Pad>,
    /// Syrinscape (online soundboard) control.
    pub syrinscape: SyrinscapeConfig,
    /// The show's running order.
    pub rundown: RundownConfig,
    /// Guests joining from a web browser by link.
    pub guests: GuestsConfig,
    /// Named lists of commands, run with one button.
    pub macros: Vec<Macro>,
    /// Instant replay.
    pub replay: ReplayConfig,
    /// The teleprompter.
    pub prompter: PrompterConfig,
    /// Viewer comments from the live chat, shown on screen.
    pub comments: CommentsConfig,
    /// The program (and sources) as NDI on the network.
    pub ndi_out: NdiOutConfig,
    /// A vertical (9:16) version of the show for Shorts, Reels and TikTok.
    pub vertical: VerticalConfig,
    /// Uploading takes to YouTube and Google Drive.
    pub upload: UploadConfig,
    /// Graphics that take turns on screen: sponsors, social handles, reminders.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rotations: Vec<Rotation>,
}

/// Graphics taking turns: every few minutes the next one comes on for a
/// while, then goes off.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Rotation {
    pub name: String,
    /// The graphics, by name, in the order they take turns.
    pub graphics: Vec<String>,
    /// How often one comes on, in minutes.
    pub every_minutes: f32,
    /// How long each stays on, in seconds.
    pub show_seconds: u32,
    /// When it starts and stops by itself.
    pub start: RotationStart,
}

impl Default for Rotation {
    fn default() -> Self {
        Rotation { name: "Sponsors".into(), graphics: Vec::new(), every_minutes: 10.0, show_seconds: 15, start: RotationStart::Live }
    }
}

/// When a rotation runs by itself (its button starts and stops it any time).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum RotationStart {
    /// Only from its button (or a command).
    Manual,
    /// While you're live.
    #[default]
    Live,
    /// While you're live or recording.
    OnAir,
}

impl RotationStart {
    pub const ALL: [RotationStart; 3] = [RotationStart::Live, RotationStart::OnAir, RotationStart::Manual];

    pub fn label(self) -> &'static str {
        match self {
            RotationStart::Manual => "only when I start it",
            RotationStart::Live => "while I'm live",
            RotationStart::OnAir => "while I'm live or recording",
        }
    }
    /// Your own keyboard shortcuts.
    pub keys: KeysConfig,
}

/// Your own keyboard shortcuts, on top of (or instead of) the built-in ones.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct KeysConfig {
    /// Built-in shortcuts you've changed, by name ("record" = "Cmd+Shift+R").
    /// Several keys are separated by commas; an empty one turns it off.
    pub builtin: std::collections::BTreeMap<String, String>,
    /// Keys that run any command.
    pub custom: Vec<CustomKey>,
}

/// A key that runs a command, like "G" for "Graphic 2 Toggle".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct CustomKey {
    pub key: String,
    pub command: String,
}

/// Syrinscape Online: moods and sounds started from Cutaway's buttons,
/// keys, Stream Deck and scenes. Syrinscape's player plays them; its sound
/// comes in through an audio input (an app's sound, or a virtual cable).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct SyrinscapeConfig {
    pub enabled: bool,
    /// Your auth token (syrinscape.com/account/auth-token/). Keep it private.
    pub token: String,
    /// The buttons in Cutaway's Syrinscape panel.
    pub buttons: Vec<SyrinButton>,
}

/// What a Syrinscape button starts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SyrinKind {
    /// A mood (a whole scene's sound; starting one replaces the last).
    #[default]
    Mood,
    /// One element: a music track, ambience or sound effect.
    Element,
    /// Stop everything.
    StopAll,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct SyrinButton {
    pub name: String,
    pub kind: SyrinKind,
    /// Syrinscape's number for the mood or element.
    pub id: u64,
    /// Its soundset (for display).
    pub soundset: String,
}


#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Container {
    /// MP4, written crash-safe: a power cut or crash loses at most the last
    /// second, and the file is repaired on the next start.
    #[serde(alias = "mkv")]
    Mp4,
    /// Same, QuickTime flavoured (Final Cut Pro, Premiere, Resolve).
    Mov,
}

impl Container {
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    H264,
    H265,
    /// Apple ProRes 422 Proxy / LT / 422 / HQ: large files, effortless editing.
    ProresProxy,
    ProresLt,
    Prores,
    ProresHq,
}

impl Codec {
    pub fn video_codec(self) -> crate::media::VideoCodec {
        use crate::media::{ProRes, VideoCodec};
        match self {
            Codec::H264 => VideoCodec::H264,
            Codec::H265 => VideoCodec::Hevc,
            Codec::ProresProxy => VideoCodec::ProRes(ProRes::Proxy),
            Codec::ProresLt => VideoCodec::ProRes(ProRes::Lt),
            Codec::Prores => VideoCodec::ProRes(ProRes::Standard),
            Codec::ProresHq => VideoCodec::ProRes(ProRes::Hq),
        }
    }

    pub fn is_prores(self) -> bool {
        self.video_codec().is_prores()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OutputConfig {
    /// Each take goes into its own dated subfolder of this folder.
    pub folder: PathBuf,
    /// Take folders are named "<prefix>_<date>_<time>".
    pub take_prefix: String,
    pub container: Container,
    pub codec: Codec,
    /// "auto" picks the best encoder in this build ("videotoolbox" on a Mac).
    pub encoder: String,
    /// 0 = automatic, based on resolution and frame rate.
    pub iso_bitrate_kbps: u32,
    /// 0 = automatic.
    pub program_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    /// Put the program audio mix into every per-source file, which makes
    /// multicam sync in an editor trivial.
    pub iso_audio: bool,
    /// 16, 24 or 32 (float) for the WAV files.
    pub wav_bit_depth: u32,
    /// Write a second copy of every file here as it records (another drive).
    pub backup_folder: Option<PathBuf>,
    /// Stamp every file with the time of day as timecode.
    pub timecode: bool,
    /// Save Final Cut Pro / Resolve (.fcpxml) and Premiere (.xml) timelines
    /// of the live edit with every take.
    pub export_timelines: bool,
    /// Also record small H.264 copies of every video file (in "Proxies"),
    /// for smooth editing on a laptop.
    pub proxies: bool,
    /// Width of the proxy files (at most).
    pub proxy_width: u32,
    /// Put each audio input on its own track in the Program file (after the
    /// mix, for re-balancing in an editor).
    pub input_tracks: bool,
    /// Input tracks as heard in the mix (after the fader and ON button)
    /// instead of before the fader.
    pub input_tracks_post_fader: bool,
    /// Save the recording mix as a WAV too.
    pub mix_wav: bool,
    /// REAPER (.rpp) and DAWproject (Fender Studio Pro, Bitwig, Cubase)
    /// sessions with every WAV on its own track.
    pub daw_sessions: bool,
    /// The take file Cutaway's DaVinci Resolve script builds a project from.
    pub resolve_project: bool,
    /// Length of an instant clip, in seconds.
    pub clip_seconds: u32,
}

impl OutputConfig {
    /// ProRes only goes in QuickTime files.
    pub fn effective_container(&self) -> Container {
        if self.codec.is_prores() {
            Container::Mov
        } else {
            self.container
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            folder: default_output_folder(),
            take_prefix: "Take".into(),
            container: Container::Mp4,
            codec: Codec::H264,
            encoder: "auto".into(),
            iso_bitrate_kbps: 0,
            program_bitrate_kbps: 0,
            audio_bitrate_kbps: 256,
            iso_audio: true,
            wav_bit_depth: 24,
            backup_folder: None,
            timecode: true,
            export_timelines: true,
            proxies: false,
            proxy_width: 960,
            input_tracks: false,
            input_tracks_post_fader: false,
            mix_wav: false,
            daw_sessions: false,
            resolve_project: false,
            clip_seconds: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CaptionPlace {
    Bottom,
    Top,
}

/// Live captions from speech recognition (on the device), and transcripts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CaptionsConfig {
    /// Listen and caption (needed for transcripts too).
    pub enabled: bool,
    /// Show captions on screen when Cutaway starts (the CC button turns
    /// them on and off).
    pub show: bool,
    /// Also burn them into the recording (otherwise the stream only; the
    /// recording gets caption files instead).
    pub on_recording: bool,
    /// What to listen to: "recording" (the mix), "stream", or an input's name.
    pub source: String,
    /// Language, e.g. "en-AU" ("" = the computer's).
    pub language: String,
    /// Text height as a fraction of the picture height.
    pub size: f32,
    pub lines: u32,
    pub place: CaptionPlace,
    /// Save Transcript.txt and Captions.srt / .vtt with each take.
    pub transcript: bool,
    /// Windows and Linux (Whisper): the speech model to use.
    pub model: SpeechModel,
}

/// Whisper speech models (Windows and Linux; a Mac uses its own).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SpeechModel {
    /// Whisper base: quick on any computer.
    #[default]
    Fast,
    /// Whisper small: fewer mistakes, needs a stronger computer.
    Accurate,
}

impl Default for CaptionsConfig {
    fn default() -> Self {
        CaptionsConfig {
            enabled: false,
            show: true,
            on_recording: false,
            source: "recording".into(),
            language: String::new(),
            size: 0.045,
            lines: 2,
            place: CaptionPlace::Bottom,
            transcript: true,
            model: SpeechModel::Fast,
        }
    }
}

/// An audio-only copy of each take for podcast hosts: the recording mix,
/// brought to a standard loudness, with the markers as chapters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PodcastConfig {
    pub enabled: bool,
    /// Integrated loudness to aim for: -16 LUFS suits Apple Podcasts and Spotify.
    pub target_lufs: f64,
    /// One channel (smaller; most speech podcasts are mono).
    pub mono: bool,
    pub bitrate_kbps: u32,
    /// Episode title ("" = the show name and date).
    pub title: String,
    /// The podcast's name, for the album tag ("" = the show name).
    pub show: String,
    pub author: String,
}

impl Default for PodcastConfig {
    fn default() -> Self {
        PodcastConfig { enabled: false, target_lufs: -16.0, mono: false, bitrate_kbps: 128, title: String::new(), show: String::new(), author: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProgramConfig {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// "#RRGGBB"
    pub background_color: String,
    /// Optional PNG/JPEG shown behind the frames.
    pub background_image: Option<PathBuf>,
    /// Width of the program monitor image in the UI.
    pub preview_width: u32,
    /// Width of each source thumbnail in the multiview.
    pub multiview_width: u32,
}

impl Default for ProgramConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            fps: 30,
            background_color: "#10131a".into(),
            background_image: None,
            preview_width: 960,
            multiview_width: 480,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TransitionMode {
    Cut,
    /// Frames glide to their new positions while sources fade in and out.
    Mix,
    /// A soft edge sweeps across, revealing the new scene.
    Wipe,
    /// The new scene slides in and pushes the old one out.
    Push,
    /// Fade through a colour (black by default).
    Dip,
    /// A video plays over the top and the scenes change under it.
    Stinger,
}

impl TransitionMode {
    pub const ALL: [TransitionMode; 6] = [TransitionMode::Cut, TransitionMode::Mix, TransitionMode::Wipe, TransitionMode::Push, TransitionMode::Dip, TransitionMode::Stinger];

    pub fn label(self) -> &'static str {
        match self {
            TransitionMode::Cut => "Cut",
            TransitionMode::Mix => "Mix",
            TransitionMode::Wipe => "Wipe",
            TransitionMode::Push => "Push",
            TransitionMode::Dip => "Dip",
            TransitionMode::Stinger => "Stinger",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TransitionMode::Cut => "cut",
            TransitionMode::Mix => "mix",
            TransitionMode::Wipe => "wipe",
            TransitionMode::Push => "push",
            TransitionMode::Dip => "dip",
            TransitionMode::Stinger => "stinger",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    pub const ALL: [Direction; 4] = [Direction::Left, Direction::Right, Direction::Up, Direction::Down];

    pub fn label(self) -> &'static str {
        match self {
            Direction::Left => "Left",
            Direction::Right => "Right",
            Direction::Up => "Up",
            Direction::Down => "Down",
        }
    }

    /// Direction of travel as a unit vector (y down).
    pub fn vector(self) -> [f32; 2] {
        match self {
            Direction::Left => [-1.0, 0.0],
            Direction::Right => [1.0, 0.0],
            Direction::Up => [0.0, -1.0],
            Direction::Down => [0.0, 1.0],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TransitionConfig {
    pub mode: TransitionMode,
    pub duration_ms: u32,
    /// Which way wipes and pushes travel.
    pub direction: Direction,
    /// Colour dips go through.
    pub dip_color: String,
    /// Switch through a preview: pick the next scene, then Take.
    pub preview_mode: bool,
    pub stinger: StingerConfig,
    /// Fade to black: how long the fade takes.
    pub ftb_ms: u32,
    /// The recording and stream sound fade with it.
    pub ftb_audio: bool,
}

impl Default for TransitionConfig {
    fn default() -> Self {
        Self {
            mode: TransitionMode::Mix,
            duration_ms: 400,
            direction: Direction::Right,
            dip_color: "#000000".into(),
            preview_mode: false,
            stinger: StingerConfig::default(),
            ftb_ms: 1000,
            ftb_audio: true,
        }
    }
}

/// Where a stinger video keeps its see-through parts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Matte {
    /// Right or below if the video is twice as wide or tall, else solid.
    #[default]
    Auto,
    /// The picture on the left, a black-and-white matte on the right.
    Right,
    /// The picture on top, the matte below.
    Below,
    /// Black is see-through (light effects on black).
    Black,
    /// No see-through parts: it covers the screen.
    Solid,
}

impl Matte {
    pub const ALL: [Matte; 5] = [Matte::Auto, Matte::Right, Matte::Below, Matte::Black, Matte::Solid];

    pub fn label(self) -> &'static str {
        match self {
            Matte::Auto => "Automatic",
            Matte::Right => "Matte on the right",
            Matte::Below => "Matte below",
            Matte::Black => "Black is see-through",
            Matte::Solid => "Solid (covers the screen)",
        }
    }
}

/// The stinger transition's video.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct StingerConfig {
    /// An MP4 or MOV file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    /// When the scenes change under it, in ms from its start (0 = halfway).
    pub cut_ms: u32,
    pub matte: Matte,
    /// Its sound (played through the Sound pads input), in dB.
    pub volume_db: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SourceConfig {
    pub name: String,
    /// Which device. One of:
    ///   - the device name as listed by `cutaway --list-devices` (a partial,
    ///     case-insensitive match is fine), e.g. "Cam Link 4K"
    ///   - "#2" for the second detected video device
    ///   - "screen" to capture the main display; "screen:display:<name>",
    ///     "screen:window:<app>|<title>" or "screen:app:<app>" for another
    ///     display, a window or an app (see `platform::capture`)
    ///   - "test" or "test:<pattern>" for a generated test picture
    ///     (patterns: smpte, ball, pinwheel, spokes, gradient, snow, colors)
    ///   - "frame:<source name>" for an auto-framed close-up cut from
    ///     another (wide) source, following the people in it
    pub device: String,
    /// Capture size and rate. The source is scaled/rate-converted to this
    /// if the device can't provide it exactly, so every file is consistent.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    /// Record this source to its own file.
    pub record: bool,
    /// Delay this video, in milliseconds, to line it up with slower sources.
    pub delay_ms: u32,
    /// Crop, mirror and colour (program only; the source's own file is untouched).
    pub look: SourceLook,
    /// Media player sources (device "media"): what to play.
    pub media: MediaConfig,
    /// Network inputs: how far behind real time to play, to ride out
    /// network hiccups.
    pub network_latency_ms: u32,
    /// Auto-framed sources (device "frame:<source>"): who to follow and how.
    pub framing: FramingConfig,
    /// Pan, tilt and zoom: a PTZ camera's controls, or a virtual one's
    /// (device "vptz:<source>") presets.
    pub ptz: PtzConfig,
    /// Which source this is while Cutaway runs (never saved): a settings
    /// change finds the same source by it, however it was renamed or moved.
    #[serde(skip)]
    pub uid: u64,
}

/// How a PTZ camera is moved.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PtzControl {
    #[default]
    None,
    /// VISCA over IP (Sony, BirdDog, Panasonic…): UDP port 52381.
    ViscaIp,
    /// Plain VISCA over TCP (PTZOptics and many others): port 5678.
    ViscaTcp,
    /// Plain VISCA over UDP: port 1259.
    ViscaUdp,
    /// An NDI camera's own controls (NDI sources only).
    Ndi,
}

impl PtzControl {
    pub const ALL: [PtzControl; 5] = [PtzControl::None, PtzControl::ViscaIp, PtzControl::ViscaTcp, PtzControl::ViscaUdp, PtzControl::Ndi];

    pub fn label(self) -> &'static str {
        match self {
            PtzControl::None => "None",
            PtzControl::ViscaIp => "VISCA over IP (Sony, BirdDog, Panasonic: UDP 52381)",
            PtzControl::ViscaTcp => "VISCA over TCP (PTZOptics and others: 5678)",
            PtzControl::ViscaUdp => "VISCA over UDP (1259)",
            PtzControl::Ndi => "NDI (the camera's own controls)",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            PtzControl::ViscaIp => 52381,
            PtzControl::ViscaTcp => 5678,
            PtzControl::ViscaUdp => 1259,
            _ => 0,
        }
    }
}

/// A saved camera position. Real cameras keep their own (by number, in
/// order); virtual ones keep it here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PtzPreset {
    pub name: String,
    /// Virtual: the centre of the shot (0..1 across and down the picture).
    pub x: f32,
    pub y: f32,
    /// Virtual: 1 = the whole picture, 2 = twice as close…
    pub zoom: f32,
}

impl Default for PtzPreset {
    fn default() -> Self {
        PtzPreset { name: "Preset".into(), x: 0.5, y: 0.5, zoom: 1.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PtzConfig {
    pub control: PtzControl,
    /// The camera's address: "192.168.1.50" or "192.168.1.50:52381".
    pub address: String,
    /// VISCA camera number (1–7; 1 for a camera on its own).
    pub camera: u8,
    /// How fast moves and preset recalls go (0.1–1).
    pub speed: f32,
    pub invert_pan: bool,
    pub invert_tilt: bool,
    pub presets: Vec<PtzPreset>,
    /// Virtual: how far it can zoom into the picture (1–4).
    pub max_zoom: f32,
    /// Virtual: how long a move to a preset takes.
    pub glide_seconds: f32,
}

impl Default for PtzConfig {
    fn default() -> Self {
        PtzConfig {
            control: PtzControl::None,
            address: String::new(),
            camera: 1,
            speed: 0.6,
            invert_pan: false,
            invert_tilt: false,
            presets: Vec::new(),
            max_zoom: 3.0,
            glide_seconds: 1.5,
        }
    }
}

impl SourceConfig {
    /// The source a virtual PTZ camera is cut from.
    pub fn virtual_ptz_of(&self) -> Option<&str> {
        let d = self.device.trim();
        (d.len() > 5 && d[..5].eq_ignore_ascii_case("vptz:")).then(|| d[5..].trim())
    }

    /// Moved by PTZ controls (a real PTZ camera or a virtual one).
    pub fn has_ptz(&self) -> bool {
        self.virtual_ptz_of().is_some() || self.ptz.control != PtzControl::None
    }
}

impl SourceConfig {
    /// The source an auto-framed source is cut from.
    pub fn framed_from(&self) -> Option<&str> {
        let d = self.device.trim();
        (d.len() > 6 && d[..6].eq_ignore_ascii_case("frame:")).then(|| d[6..].trim())
    }
}

/// Who an auto-framed source follows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FramingFollow {
    /// Everyone in the picture, together.
    #[default]
    Everyone,
    /// The biggest face (usually whoever is closest).
    Largest,
    /// The person furthest left / in the middle / furthest right.
    Left,
    Middle,
    Right,
}

impl FramingFollow {
    pub const ALL: [FramingFollow; 5] = [FramingFollow::Everyone, FramingFollow::Largest, FramingFollow::Left, FramingFollow::Middle, FramingFollow::Right];

    pub fn label(self) -> &'static str {
        match self {
            FramingFollow::Everyone => "Everyone",
            FramingFollow::Largest => "Closest person",
            FramingFollow::Left => "Person on the left",
            FramingFollow::Middle => "Person in the middle",
            FramingFollow::Right => "Person on the right",
        }
    }
}

/// How tight an auto-framed shot is.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum FramingShot {
    /// Head and shoulders.
    #[default]
    CloseUp,
    /// Chest up.
    Medium,
    /// Waist up, with room around.
    Wide,
}

impl FramingShot {
    pub const ALL: [FramingShot; 3] = [FramingShot::CloseUp, FramingShot::Medium, FramingShot::Wide];

    pub fn label(self) -> &'static str {
        match self {
            FramingShot::CloseUp => "Close-up",
            FramingShot::Medium => "Medium",
            FramingShot::Wide => "Wide",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FramingConfig {
    pub follow: FramingFollow,
    pub shot: FramingShot,
    /// How long a move to a new framing takes, in seconds.
    pub reframe_seconds: f32,
    /// Never enlarge the wide picture more than this (1–3); higher allows
    /// tighter shots from a lower-resolution camera, but softer.
    pub max_zoom: f32,
}

impl Default for FramingConfig {
    fn default() -> Self {
        FramingConfig { follow: FramingFollow::Everyone, shot: FramingShot::CloseUp, reframe_seconds: 1.0, max_zoom: 1.5 }
    }
}

/// A playlist for a media player source.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MediaConfig {
    /// Video files (MP4/MOV: H.264, HEVC, ProRes; AAC audio) and images.
    pub files: Vec<PathBuf>,
    /// Start again from the first item after the last.
    pub loop_playlist: bool,
    /// Start playing when the app starts.
    pub autoplay: bool,
    /// Start from the beginning whenever a scene with this source is taken.
    pub restart_on_air: bool,
    /// How long images stay up in a playlist.
    pub image_seconds: f32,
}

impl Default for MediaConfig {
    fn default() -> Self {
        MediaConfig { files: Vec::new(), loop_playlist: true, autoplay: true, restart_on_air: false, image_seconds: 5.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SourceLook {
    /// Percent of the picture trimmed from each edge.
    pub crop_left: f32,
    pub crop_right: f32,
    pub crop_top: f32,
    pub crop_bottom: f32,
    /// Flip left to right.
    pub mirror: bool,
    /// -1..1, 0 = unchanged.
    pub brightness: f32,
    /// 0..2, 1 = unchanged.
    pub contrast: f32,
    /// 0..2, 1 = unchanged (0 = black and white).
    pub saturation: f32,
    /// Remove a green (or blue) screen behind this source.
    pub chroma_key: bool,
    pub key_color: String,
    /// How much of the colour is removed (0..1).
    pub key_similarity: f32,
    /// Softness of the edge (0..1).
    pub key_smoothness: f32,
    /// Removes the screen's colour cast from edges and hair (0..1).
    pub key_spill: f32,
    /// Cut the people out of their surroundings, no green screen needed
    /// (what's behind shows the scene's background or the layers below).
    pub remove_background: bool,
    pub cutout_quality: CutoutQuality,
    /// A colour LUT (.cube), e.g. a camera's log-to-Rec.709 conversion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lut: Option<PathBuf>,
    /// How much of the LUT (0..1).
    pub lut_strength: f32,
}

/// How carefully people are cut out: finer edges cost more time.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CutoutQuality {
    Fast,
    #[default]
    Balanced,
    Best,
}

impl CutoutQuality {
    pub const ALL: [CutoutQuality; 3] = [CutoutQuality::Fast, CutoutQuality::Balanced, CutoutQuality::Best];

    pub fn label(self) -> &'static str {
        match self {
            CutoutQuality::Fast => "Fast",
            CutoutQuality::Balanced => "Balanced",
            CutoutQuality::Best => "Best edges",
        }
    }

    /// For the segmenter: 0 fast, 1 balanced, 2 accurate.
    pub fn level(self) -> u8 {
        match self {
            CutoutQuality::Fast => 0,
            CutoutQuality::Balanced => 1,
            CutoutQuality::Best => 2,
        }
    }
}

impl Default for SourceLook {
    fn default() -> Self {
        SourceLook {
            crop_left: 0.0,
            crop_right: 0.0,
            crop_top: 0.0,
            crop_bottom: 0.0,
            mirror: false,
            brightness: 0.0,
            contrast: 1.0,
            saturation: 1.0,
            chroma_key: false,
            key_color: "#00d040".into(),
            key_similarity: 0.4,
            key_smoothness: 0.08,
            key_spill: 0.1,
            remove_background: false,
            cutout_quality: CutoutQuality::Balanced,
            lut: None,
            lut_strength: 1.0,
        }
    }
}

impl SourceLook {

    /// Visible part of the picture as (left, top, right, bottom) fractions.
    pub fn crop_rect(&self) -> [f32; 4] {
        let c = |v: f32| (v / 100.0).clamp(0.0, 0.45);
        [c(self.crop_left), c(self.crop_top), 1.0 - c(self.crop_right), 1.0 - c(self.crop_bottom)]
    }

    /// Width/height of the visible part, for a source of `aspect`.
    pub fn cropped_aspect(&self, aspect: f32) -> f32 {
        let r = self.crop_rect();
        aspect * (r[2] - r[0]) / (r[3] - r[1]).max(0.01)
    }
}

/// A source's settings without those that change while it runs (its look,
/// playlist, framing, PTZ and whether it's recorded): if these differ, the
/// source is reopened.
pub fn source_key(s: &SourceConfig) -> SourceConfig {
    SourceConfig { look: SourceLook::default(), media: MediaConfig::default(), framing: FramingConfig::default(), ptz: PtzConfig::default(), record: true, ..s.clone() }
}

/// The same for an audio input (level, routing, processing, ducking and
/// recording change while it runs).
pub fn audio_key(a: &AudioConfig) -> AudioConfig {
    AudioConfig {
        gain_db: 0.0,
        in_program: true,
        to_recording: true,
        to_stream: true,
        processing: VoiceProcessing::default(),
        duck: false,
        record: true,
        split_channels: false,
        ..a.clone()
    }
}

/// A new identity for a source or input.
pub fn next_uid() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl Default for SourceConfig {
    fn default() -> Self {
        Self {
            name: "Camera".into(),
            device: "test".into(),
            width: 1920,
            height: 1080,
            fps: 30,
            record: true,
            delay_ms: 0,
            look: SourceLook::default(),
            media: MediaConfig::default(),
            network_latency_ms: 300,
            framing: FramingConfig::default(),
            ptz: PtzConfig::default(),
            uid: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioConfig {
    pub name: String,
    /// Device name (partial match), "#N", "default", or "test".
    pub device: String,
    /// How many channels to capture from the device. Every channel goes into
    /// the WAV file.
    pub channels: u32,
    /// Which input channels (1-based) feed the program's left and right.
    /// Use the same number twice for a mono source.
    pub program_left: u32,
    pub program_right: u32,
    pub gain_db: f64,
    /// Whether this input is heard in the program mix.
    pub in_program: bool,
    /// Record this input to WAV.
    pub record: bool,
    /// Write one mono WAV per channel instead of a single multichannel WAV.
    pub split_channels: bool,
    /// Delay this audio, in milliseconds, to line it up with the video.
    pub delay_ms: u32,
    /// Heard in the recording's mix (Program file and every ISO file).
    pub to_recording: bool,
    /// Heard in the live stream's mix.
    pub to_stream: bool,
    /// Voice processing for the mixes (the WAV stays untouched).
    pub processing: VoiceProcessing,
    /// Turned down automatically while others talk (music, pads, ambience).
    pub duck: bool,
    /// Which input this is while Cutaway runs (never saved), as for sources.
    #[serde(skip)]
    pub uid: u64,
}

/// A channel strip for a microphone. Off by default: a DAW's output is
/// usually already processed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VoiceProcessing {
    pub enabled: bool,
    /// Removes rumble below this frequency (0 = off).
    pub high_pass_hz: f32,
    /// Quiets the mic when nobody is speaking into it.
    pub gate: bool,
    pub gate_threshold_db: f32,
    pub eq: bool,
    pub eq_low_db: f32,
    pub eq_mid_db: f32,
    pub eq_mid_hz: f32,
    pub eq_high_db: f32,
    /// Evens out loud and quiet speech.
    pub compressor: bool,
    pub comp_threshold_db: f32,
    pub comp_ratio: f32,
    pub comp_makeup_db: f32,
    /// Tames harsh "s" sounds.
    pub de_esser: bool,
    pub de_esser_threshold_db: f32,
    /// Turns down everything that isn't a voice (RNNoise): fans, hum,
    /// traffic, keyboards.
    pub noise_reduction: bool,
    /// How much (0–1): less keeps some room sound and is gentler on voices.
    pub noise_amount: f32,
}

impl Default for VoiceProcessing {
    fn default() -> Self {
        VoiceProcessing {
            enabled: false,
            high_pass_hz: 80.0,
            gate: false,
            gate_threshold_db: -50.0,
            eq: false,
            eq_low_db: 0.0,
            eq_mid_db: 0.0,
            eq_mid_hz: 2500.0,
            eq_high_db: 0.0,
            compressor: true,
            comp_threshold_db: -20.0,
            comp_ratio: 3.0,
            comp_makeup_db: 4.0,
            de_esser: false,
            de_esser_threshold_db: -30.0,
            noise_reduction: false,
            noise_amount: 1.0,
        }
    }
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            name: "DAW".into(),
            device: "default".into(),
            channels: 2,
            program_left: 1,
            program_right: 2,
            gain_db: 0.0,
            in_program: true,
            record: true,
            split_channels: false,
            delay_ms: 0,
            to_recording: true,
            to_stream: true,
            processing: VoiceProcessing::default(),
            duck: false,
            uid: 0,
        }
    }
}

impl AudioConfig {
    /// The sound pads' strip.
    pub fn is_pads(&self) -> bool {
        self.device.trim().eq_ignore_ascii_case("pads")
    }
}

/// Where a stream goes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    Youtube,
    Twitch,
    Facebook,
    Kick,
    /// Any RTMP or RTMPS server (Restream, vMix, a media server, ...).
    CustomRtmp,
    /// SRT to a server or another machine (OBS, vMix, a receiver).
    Srt,
}

impl Service {
    pub const ALL: [Service; 6] =
        [Service::Youtube, Service::Twitch, Service::Facebook, Service::Kick, Service::CustomRtmp, Service::Srt];

    pub fn label(self) -> &'static str {
        match self {
            Service::Youtube => "YouTube",
            Service::Twitch => "Twitch",
            Service::Facebook => "Facebook",
            Service::Kick => "Kick",
            Service::CustomRtmp => "Custom RTMP / RTMPS",
            Service::Srt => "SRT",
        }
    }

    /// Server to fill in when the service is picked.
    pub fn default_server(self) -> &'static str {
        match self {
            Service::Youtube => "rtmp://a.rtmp.youtube.com/live2",
            Service::Twitch => "rtmp://live.twitch.tv/app",
            Service::Facebook => "rtmps://live-api-s.facebook.com:443/rtmp",
            Service::Kick => "",
            Service::CustomRtmp => "rtmp://",
            Service::Srt => "srt://host:9000",
        }
    }

    pub fn help(self) -> &'static str {
        match self {
            Service::Youtube => "Stream key from YouTube Studio → Go live → Stream. For RTMPS use rtmps://a.rtmps.youtube.com:443/live2",
            Service::Twitch => "Stream key from Creator Dashboard → Settings → Stream. Most channels should stay at or below 6000 kbps.",
            Service::Facebook => "Stream key from Live Producer → Streaming software. Facebook requires RTMPS.",
            Service::Kick => "Copy both the Stream URL and Stream key from your Kick creator dashboard.",
            Service::CustomRtmp => "Server URL and stream key from your platform or media server.",
            Service::Srt => "srt://host:port of the receiver. The key (optional) is sent as the SRT stream ID.",
        }
    }

    pub fn is_srt(self) -> bool {
        self == Service::Srt
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StreamDestination {
    pub name: String,
    pub service: Service,
    /// rtmp://, rtmps:// or srt:// address, without the key.
    pub server: String,
    /// Stream key (kept in this settings file; treat the file as private).
    pub key: String,
    pub enabled: bool,
    /// Gets the vertical (9:16) version instead of the program.
    pub vertical: bool,
}

impl Default for StreamDestination {
    fn default() -> Self {
        StreamDestination {
            name: "YouTube".into(),
            service: Service::Youtube,
            server: Service::Youtube.default_server().into(),
            key: String::new(),
            enabled: true,
            vertical: false,
        }
    }
}

impl StreamDestination {
    /// Full address handed to the network sink.
    pub fn url(&self) -> String {
        let server = self.server.trim();
        let key = self.key.trim();
        if self.service.is_srt() {
            if key.is_empty() {
                server.to_string()
            } else {
                let sep = if server.contains('?') { '&' } else { '?' };
                format!("{server}{sep}streamid={key}")
            }
        } else if key.is_empty() {
            server.to_string()
        } else {
            format!("{}/{}", server.trim_end_matches('/'), key)
        }
    }

    /// Problem with this destination, if any (shown in settings).
    pub fn problem(&self) -> Option<String> {
        let server = self.server.trim().to_lowercase();
        if self.service.is_srt() {
            if !server.starts_with("srt://") || server.len() <= "srt://".len() {
                return Some("the address must look like srt://host:port".into());
            }
        } else {
            if !(server.starts_with("rtmp://") || server.starts_with("rtmps://")) || server.ends_with("://") {
                return Some("the server must start with rtmp:// or rtmps://".into());
            }
            if self.key.trim().is_empty() {
                // Custom servers may take the stream name in the URL itself
                // (rtmp://host/app/name); the big services always need a key.
                let path_parts = server
                    .split("://")
                    .nth(1)
                    .map(|rest| rest.split('/').skip(1).filter(|p| !p.is_empty()).count())
                    .unwrap_or(0);
                if self.service != Service::CustomRtmp || path_parts < 2 {
                    return Some("add the stream key".into());
                }
            }
        }
        if self.service == Service::Facebook && server.starts_with("rtmp://") {
            return Some("Facebook only accepts rtmps://".into());
        }
        None
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct StreamingConfig {
    /// Stream picture size; the program is scaled to this.
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_bitrate_kbps: u32,
    pub audio_bitrate_kbps: u32,
    /// "auto" or an encoder name (see `encoder` under [output]).
    pub encoder: String,
    /// Reconnect a destination that drops, with increasing waits.
    pub auto_reconnect: bool,
    /// Give up after this many attempts in a row (0 = keep trying).
    pub max_retries: u32,
    /// Start recording automatically when going live.
    pub record_when_live: bool,
    /// Lower the video bitrate while a connection can't keep up (and bring
    /// it back once it can).
    pub adaptive_bitrate: bool,
    pub destinations: Vec<StreamDestination>,
}

impl Default for StreamingConfig {
    fn default() -> Self {
        StreamingConfig {
            width: 1920,
            height: 1080,
            fps: 30,
            video_bitrate_kbps: 6000,
            audio_bitrate_kbps: 160,
            encoder: "auto".into(),
            auto_reconnect: true,
            max_retries: 0,
            record_when_live: false,
            adaptive_bitrate: true,
            destinations: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UiConfig {
    /// Interface zoom (1.0 = normal).
    pub scale: f32,
    pub confirm_stop_recording: bool,
    pub confirm_end_stream: bool,
    /// Name of the show profile these settings belong to ("" = none).
    pub profile: String,
    /// Run the pre-show check before recording or going live.
    pub check_before_start: bool,
    /// Ask GitHub whether there's a newer Cutaway, at start.
    pub check_updates: bool,
    /// Download a newer Cutaway by itself and put it in when Cutaway closes.
    pub auto_update: bool,
    /// A newer version you chose not to hear about again.
    pub skip_version: String,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig { scale: 1.0, confirm_stop_recording: false, confirm_end_stream: true, profile: String::new(), check_before_start: false, check_updates: true, auto_update: false, skip_version: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FrameStyle {
    /// Corner rounding, in pixels of the program picture.
    pub corner_radius: f32,
    /// Border width in pixels (0 = none).
    pub border_width: f32,
    pub border_color: String,
    /// Drop shadow strength, 0..1 (0 = none).
    pub shadow: f32,
}

impl Default for FrameStyle {
    fn default() -> Self {
        FrameStyle { corner_radius: 0.0, border_width: 0.0, border_color: "#ffffff".into(), shadow: 0.0 }
    }
}

/// What's heard where.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MixConfig {
    /// Stops the mixes from clipping.
    pub limiter: bool,
    pub limiter_ceiling_db: f32,
    /// Keep the stream at a steady loudness (what YouTube, Twitch and
    /// Spotify normalise to). The recording is left as mixed.
    pub stream_levelling: bool,
    pub stream_target_lufs: f32,
    /// Headphone / monitor output device ("" = off).
    pub monitor_device: String,
    /// "program", "stream", or an input's name (solo).
    pub monitor_source: String,
    pub monitor_volume_db: f32,
    /// Ducking on: inputs marked to duck go down while others talk.
    pub ducking: bool,
    /// Ducking: inputs marked to duck go down by this much while others talk.
    pub duck_db: f32,
    /// What counts as talking (the loudest other input, after its fader).
    pub duck_threshold_db: f32,
    /// How long after the talking stops before the ducked inputs come back.
    pub duck_release_ms: u32,
    /// Extra outputs: presenters' headphones, mix-minus returns for callers.
    pub outputs: Vec<AudioOutput>,
    /// The producer's talkback mic ("" = none). Never heard in the
    /// recording or the stream; only in outputs with talkback on.
    pub talkback_input: String,
    /// While talking back, what else is in those outputs dips by this much.
    pub talkback_dim_db: f32,
    /// Remote guests hear talkback too.
    pub talkback_to_guests: bool,
}

/// An audio output fed with a mix: presenters' headphones, or a mix-minus
/// return for a caller (everything except their own voice).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AudioOutput {
    pub name: String,
    /// Output device (as listed in Settings; "" or "default" = the system output).
    pub device: String,
    /// Output channels (1-based), e.g. 3 and 4 on an interface.
    pub left: u32,
    pub right: u32,
    /// "recording" or "stream": which mix it starts from.
    pub mix: String,
    /// Inputs left out (a caller's own voice).
    pub minus: Vec<String>,
    /// Talkback is heard here.
    pub talkback: bool,
    pub volume_db: f32,
}

impl Default for AudioOutput {
    fn default() -> Self {
        AudioOutput {
            name: "Headphones".into(),
            device: String::new(),
            left: 1,
            right: 2,
            mix: "recording".into(),
            minus: Vec::new(),
            talkback: true,
            volume_db: 0.0,
        }
    }
}

/// How a sound pad plays.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum PadMode {
    /// Plays to the end; pressing again stops it.
    #[default]
    Once,
    /// Plays round and round until pressed again.
    Loop,
    /// Plays while the pad is held down.
    Hold,
}

impl PadMode {
    pub const ALL: [PadMode; 3] = [PadMode::Once, PadMode::Loop, PadMode::Hold];
    pub fn label(self) -> &'static str {
        match self {
            PadMode::Once => "play once",
            PadMode::Loop => "loop",
            PadMode::Hold => "while held",
        }
    }
}

/// A sound on a pad.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Pad {
    pub name: String,
    /// WAV, AIFF, MP3, AAC/M4A, FLAC, Ogg or CAF.
    pub file: PathBuf,
    pub mode: PadMode,
    pub gain_db: f32,
    /// Fade when stopped early, in milliseconds.
    pub fade_ms: u32,
    /// Button colour.
    pub color: String,
}

impl Default for Pad {
    fn default() -> Self {
        Pad { name: "Pad".into(), file: PathBuf::new(), mode: PadMode::Once, gain_db: 0.0, fade_ms: 300, color: "#3b6fd8".into() }
    }
}

impl Default for MixConfig {
    fn default() -> Self {
        MixConfig {
            limiter: true,
            limiter_ceiling_db: -1.0,
            stream_levelling: false,
            stream_target_lufs: -14.0,
            monitor_device: String::new(),
            monitor_source: "program".into(),
            monitor_volume_db: 0.0,
            ducking: false,
            duck_db: -12.0,
            duck_threshold_db: -40.0,
            duck_release_ms: 800,
            outputs: Vec::new(),
            talkback_input: String::new(),
            talkback_dim_db: -12.0,
            talkback_to_guests: false,
        }
    }
}

/// Cut to whoever is talking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AutoSwitchConfig {
    /// On when the app starts.
    pub enabled: bool,
    pub rules: Vec<AutoRule>,
    /// Level that counts as talking.
    pub threshold_db: f32,
    /// Someone must talk this long before the shot changes.
    pub hold_ms: u32,
    /// Shortest time a shot stays up.
    pub min_shot_s: f32,
    /// Scene for crosstalk and long shots ("" = none).
    pub wide_scene: String,
    /// Go wide when several people talk at once.
    pub wide_on_crosstalk: bool,
    /// After this long on one person, show the wide shot for a moment (0 = never).
    pub max_shot_s: f32,
    /// How long that wide shot stays up.
    pub wide_for_s: f32,
    /// After you switch by hand, automatic switching waits this long.
    pub manual_pause_s: f32,
}

impl Default for AutoSwitchConfig {
    fn default() -> Self {
        AutoSwitchConfig {
            enabled: false,
            rules: Vec::new(),
            threshold_db: -40.0,
            hold_ms: 500,
            min_shot_s: 3.0,
            wide_scene: String::new(),
            wide_on_crosstalk: true,
            max_shot_s: 0.0,
            wide_for_s: 4.0,
            manual_pause_s: 8.0,
        }
    }
}

/// "When this mic talks, show this scene."
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct AutoRule {
    /// Audio input name.
    pub input: String,
    /// Channel of that input (1-based; 0 = all channels).
    pub channel: u32,
    pub scene: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum GraphicKind {
    /// A PNG (with transparency) or JPEG.
    Image,
    /// A name title (lower third) with editable text.
    Title,
    /// A clock, countdown or stopwatch.
    Clock,
    /// A band with scrolling text (`title`: the items, one per line;
    /// `subtitle`: a label at the left, "" for none).
    Ticker,
    /// A web page (`url`: a web address or an HTML file), drawn with its
    /// transparency at `page_size`, like a browser source.
    Web,
}

/// What a clock graphic shows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ClockKind {
    /// The time of day.
    #[default]
    Time,
    /// Counting down to a time of day ("Starting in 4:59").
    Until,
    /// Counting down from a length, started by a button or command.
    Countdown,
    /// Counting up, started by a button or command.
    Stopwatch,
    /// Time left in the rundown's current segment.
    Segment,
}

impl ClockKind {
    pub const ALL: [ClockKind; 5] = [ClockKind::Time, ClockKind::Until, ClockKind::Countdown, ClockKind::Stopwatch, ClockKind::Segment];

    pub fn label(self) -> &'static str {
        match self {
            ClockKind::Time => "Time of day",
            ClockKind::Until => "Countdown to a time",
            ClockKind::Countdown => "Countdown",
            ClockKind::Stopwatch => "Stopwatch",
            ClockKind::Segment => "Rundown segment time left",
        }
    }

    /// Started and stopped by buttons and commands.
    pub fn is_timer(self) -> bool {
        matches!(self, ClockKind::Countdown | ClockKind::Stopwatch)
    }
}

/// How a graphic comes on and goes off.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Motion {
    /// Fades in and out where it is.
    #[default]
    Fade,
    /// Slides in from the nearest side, and back out.
    Slide,
    /// Drawn on from left to right; off the same way back.
    Wipe,
    /// Rises a little into place as it fades in.
    Rise,
    /// Grows into place from slightly smaller.
    Pop,
}

impl Motion {
    pub fn is_fade(&self) -> bool {
        *self == Motion::Fade
    }

    pub const ALL: [Motion; 5] = [Motion::Fade, Motion::Slide, Motion::Wipe, Motion::Rise, Motion::Pop];

    pub fn label(self) -> &'static str {
        match self {
            Motion::Fade => "Fade",
            Motion::Slide => "Slide in from the side",
            Motion::Wipe => "Wipe on",
            Motion::Rise => "Rise into place",
            Motion::Pop => "Pop in",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Graphic {
    pub name: String,
    pub kind: GraphicKind,
    pub image: PathBuf,
    pub title: String,
    pub subtitle: String,
    /// Top-left corner, as fractions of the program picture.
    pub x: f32,
    pub y: f32,
    /// Width as a fraction of the program picture (height follows).
    pub width: f32,
    pub opacity: f32,
    /// How long it takes to come on or go off.
    pub fade_ms: u32,
    /// How it comes on and goes off.
    #[serde(skip_serializing_if = "Motion::is_fade")]
    pub motion: Motion,
    /// Shown when the app starts.
    pub visible: bool,
    pub text_color: String,
    pub box_color: String,
    pub box_opacity: f32,
    /// Font file for titles ("" = built-in).
    pub font: PathBuf,
    /// Clocks: what it shows (`title` is the label above it, "" for none).
    pub clock: ClockKind,
    /// Countdown to this time of day ("19:30").
    pub until: String,
    /// Countdown length in seconds.
    pub seconds: u32,
    pub clock_24h: bool,
    pub show_seconds: bool,
    /// Tickers: how fast the text moves, in picture widths per minute.
    pub speed: f32,
    /// Web pages: the address (or an HTML file).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    /// Web pages: the size the page is laid out at, in CSS pixels.
    #[serde(skip_serializing_if = "is_page_size")]
    pub page_size: [u32; 2],
}

const PAGE_SIZE: [u32; 2] = [1920, 1080];

fn is_page_size(s: &[u32; 2]) -> bool {
    *s == PAGE_SIZE
}

impl Default for Graphic {
    fn default() -> Self {
        Graphic {
            name: "Logo".into(),
            kind: GraphicKind::Image,
            image: PathBuf::new(),
            title: String::new(),
            subtitle: String::new(),
            x: 0.85,
            y: 0.04,
            width: 0.11,
            opacity: 1.0,
            fade_ms: 300,
            motion: Motion::Fade,
            visible: false,
            text_color: "#ffffff".into(),
            box_color: "#101820".into(),
            box_opacity: 0.75,
            font: PathBuf::new(),
            clock: ClockKind::Time,
            until: "19:00".into(),
            seconds: 300,
            clock_24h: false,
            show_seconds: false,
            url: String::new(),
            page_size: PAGE_SIZE,
            speed: 6.0,
        }
    }
}

impl Graphic {
    pub fn title(name: &str, title: &str, subtitle: &str) -> Graphic {
        Graphic { name: name.into(), kind: GraphicKind::Title, title: title.into(), subtitle: subtitle.into(), x: 0.05, y: 0.76, width: 0.36, ..Default::default() }
    }

    pub fn clock(name: &str, clock: ClockKind, label: &str) -> Graphic {
        Graphic { name: name.into(), kind: GraphicKind::Clock, clock, title: label.into(), x: 0.80, y: 0.05, width: 0.15, ..Default::default() }
    }

    pub fn ticker(name: &str, text: &str, label: &str) -> Graphic {
        Graphic { name: name.into(), kind: GraphicKind::Ticker, title: text.into(), subtitle: label.into(), x: 0.0, y: 0.92, width: 1.0, box_opacity: 0.85, ..Default::default() }
    }

    /// A web page over the whole picture.
    pub fn web(name: &str, url: &str) -> Self {
        Graphic { name: name.into(), kind: GraphicKind::Web, url: url.into(), x: 0.0, y: 0.0, width: 1.0, ..Default::default() }
    }
}

/// How guests reach Cutaway's guest page.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum GuestAccess {
    /// A free Cloudflare quick tunnel: an https://….trycloudflare.com address,
    /// nothing to set up.
    #[default]
    Tunnel,
    /// Your own HTTPS address that forwards to the guest port.
    Custom,
}

/// Guests join from their browser: no app, just a link. Each guest has a
/// video source (device "guest:ID") and an audio input ("source:NAME").
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GuestsConfig {
    pub enabled: bool,
    /// Port of the guests' web page (only guest pages are served on it).
    pub port: u16,
    pub access: GuestAccess,
    /// With Custom access: the https:// address that reaches the port.
    pub public_url: String,
    /// Send the program picture back so guests can see the show.
    pub return_video: bool,
    /// Guests' browsers also record them (picture and sound as captured,
    /// before the internet gets at it) while the show records, and upload
    /// it into the take's Guests folder as they go.
    pub backup: bool,
    /// A TURN server for guests behind strict firewalls ("" = none).
    pub turn_url: String,
    pub turn_username: String,
    pub turn_password: String,
}

impl Default for GuestsConfig {
    fn default() -> Self {
        GuestsConfig {
            enabled: false,
            port: 8792,
            access: GuestAccess::Tunnel,
            public_url: String::new(),
            return_video: false,
            backup: false,
            turn_url: String::new(),
            turn_username: String::new(),
            turn_password: String::new(),
        }
    }
}

impl SourceConfig {
    /// A remote guest's link id, for a "guest:ID" source.
    pub fn guest_id(&self) -> Option<&str> {
        self.device.trim().strip_prefix("guest:").map(str::trim).filter(|s| !s.is_empty())
    }

    /// A phone camera's link id, for a "phone:ID" source (a phone joins by
    /// scanning a QR code, and sends its camera only).
    pub fn phone_id(&self) -> Option<&str> {
        self.device.trim().strip_prefix("phone:").map(str::trim).filter(|s| !s.is_empty())
    }

    /// A remote guest's shared screen, for a "guest-screen:ID" source (ID
    /// is the guest's own link: they share from the page they joined on).
    pub fn guest_screen_id(&self) -> Option<&str> {
        self.device.trim().strip_prefix("guest-screen:").map(str::trim).filter(|s| !s.is_empty())
    }

    /// Either kind of link: (id, whether it's a phone camera).
    pub fn link_id(&self) -> Option<(&str, bool)> {
        self.guest_id().map(|i| (i, false)).or_else(|| self.phone_id().map(|i| (i, true)))
    }
}

/// One part of the show's running order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Segment {
    pub name: String,
    /// Planned length in seconds (0 = open-ended).
    pub seconds: u32,
    /// Scene to take when it starts ("" = stay).
    pub scene: String,
    /// Commands to run when it starts (graphics, pads, Syrinscape…).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<String>,
    /// Notes for the producer (talking points, who's on).
    pub notes: String,
}

impl Default for Segment {
    fn default() -> Self {
        Segment { name: "Segment".into(), seconds: 300, scene: String::new(), cues: Vec::new(), notes: String::new() }
    }
}

/// The running order.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RundownConfig {
    pub segments: Vec<Segment>,
    /// Put a marker (and a chapter) in the recording as each segment starts.
    pub markers: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayOutput {
    Off,
    Program,
    Multiview,
    /// The teleprompter's script.
    Prompter,
}

/// A second window for another screen, projector or TV.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct DisplayConfig {
    pub output: DisplayOutput,
    pub fullscreen: bool,
    /// Which display it goes on, by name (as `platform::displays` names
    /// them); empty: another display than Cutaway's, if there is one.
    pub screen: String,
    /// Turn the virtual camera on when Cutaway starts.
    pub virtual_camera: bool,
    /// Show the time and the rundown segment's time left on this screen
    /// (for presenters; not in the recording or stream).
    pub clocks: bool,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        DisplayConfig { output: DisplayOutput::Off, fullscreen: true, screen: String::new(), virtual_camera: false, clocks: false }
    }
}

/// Control from a phone, tablet, Stream Deck or anything that can open a web address.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RemoteConfig {
    pub enabled: bool,
    pub port: u16,
    /// Asked for once by each phone/tablet; empty = no PIN (not recommended).
    pub pin: String,
}

impl Default for RemoteConfig {
    fn default() -> Self {
        RemoteConfig { enabled: false, port: 8790, pin: String::new() }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MidiConfig {
    pub enabled: bool,
    /// Controller name (partial match); "" = every connected controller.
    pub device: String,
    /// Light up pads to show what's live.
    pub feedback: bool,
    pub mappings: Vec<MidiMapping>,
}

impl Default for MidiConfig {
    fn default() -> Self {
        MidiConfig { enabled: false, device: String::new(), feedback: true, mappings: Vec::new() }
    }
}

/// OSC: control from show-control software (QLab, Companion, TouchOSC,
/// lighting desks), with state sent back for buttons that light up.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OscConfig {
    pub enabled: bool,
    /// UDP port Cutaway listens on.
    pub port: u16,
    /// Where to send state ("" = nowhere): a host name or address.
    pub feedback_host: String,
    pub feedback_port: u16,
}

impl Default for OscConfig {
    fn default() -> Self {
        OscConfig { enabled: false, port: 8791, feedback_host: String::new(), feedback_port: 9001 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct MidiMapping {
    /// "note 36", "cc 20" (channel 1), or "note 36 ch 10".
    pub message: String,
    /// What it does: see `crate::control::Action`.
    pub action: String,
}

impl Config {
    /// Settings that need the engine restarted when they change.
    /// Auto-framed sources run at their camera's rate, whatever they say.
    pub fn with_framed_rates(&self) -> Config {
        let mut c = self.clone();
        for i in 0..c.sources.len() {
            let parent = c.sources[i].framed_from().and_then(|from| self.sources.iter().find(|p| p.name.trim().eq_ignore_ascii_case(from)));
            if let Some(p) = parent {
                c.sources[i].fps = p.fps;
            }
        }
        c
    }

    /// Changes that rebuild the whole engine: the program's size and rate.
    /// (Sources and inputs change live: see `lineup_changed`.)
    pub fn needs_restart(&self, other: &Config) -> bool {
        self.program != other.program
    }

    /// Sources or audio inputs added, removed, moved or changed in a way
    /// that reopens them (applied live, source by source).
    pub fn lineup_changed(&self, other: &Config) -> bool {
        let video = |c: &Config| c.sources.iter().map(source_key).collect::<Vec<_>>();
        let audio = |c: &Config| c.audio.iter().map(audio_key).collect::<Vec<_>>();
        video(self) != video(other) || audio(self) != audio(other)
    }

    /// Give every source and input an identity (new ones, and copies that
    /// share one).
    pub fn assign_uids(&mut self) {
        let mut seen = HashSet::new();
        for s in &mut self.sources {
            if s.uid == 0 || !seen.insert(s.uid) {
                s.uid = next_uid();
                seen.insert(s.uid);
            }
        }
        let mut seen = HashSet::new();
        for a in &mut self.audio {
            if a.uid == 0 || !seen.insert(a.uid) {
                a.uid = next_uid();
                seen.insert(a.uid);
            }
        }
    }

    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        let cfg: Config =
            toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }

    pub fn validate(&self) -> Result<()> {
        let p = &self.program;
        anyhow::ensure!(p.width >= 160 && p.height >= 90, "program size is too small");
        anyhow::ensure!(p.width.is_multiple_of(2) && p.height.is_multiple_of(2), "program width and height must be even");
        anyhow::ensure!((1..=120).contains(&p.fps), "program fps must be between 1 and 120");
        for s in &self.sources {
            anyhow::ensure!(s.width % 2 == 0 && s.height % 2 == 0, "source '{}': width and height must be even", s.name);
            anyhow::ensure!((1..=120).contains(&s.fps), "source '{}': fps must be between 1 and 120", s.name);
        }
        for a in &self.audio {
            anyhow::ensure!((1..=64).contains(&a.channels), "audio '{}': channels must be 1-64", a.name);
            anyhow::ensure!(
                (1..=a.channels).contains(&a.program_left) && (1..=a.channels).contains(&a.program_right),
                "audio '{}': program_left/right must be between 1 and {}",
                a.name,
                a.channels
            );
        }
        anyhow::ensure!(matches!(self.output.wav_bit_depth, 16 | 24 | 32), "wav_bit_depth must be 16, 24 or 32");
        let st = &self.streaming;
        anyhow::ensure!(st.width.is_multiple_of(2) && st.height.is_multiple_of(2) && st.width >= 160, "stream width and height must be even");
        anyhow::ensure!((1..=120).contains(&st.fps), "stream fps must be between 1 and 120");
        anyhow::ensure!((200..=100_000).contains(&st.video_bitrate_kbps), "stream video bitrate must be 200-100000 kbps");
        let mut names = std::collections::HashSet::new();
        for s in &self.sources {
            anyhow::ensure!(!s.name.trim().is_empty(), "every source needs a name");
            anyhow::ensure!(names.insert(s.name.trim().to_lowercase()), "two sources are called \"{}\"", s.name);
        }
        for s in &self.sources {
            if let Some(from) = s.framed_from() {
                let parent = self.sources.iter().find(|p| p.name.trim().eq_ignore_ascii_case(from));
                let parent = parent.ok_or_else(|| anyhow::anyhow!("source '{}' follows people in \"{from}\", but there's no source called that", s.name))?;
                anyhow::ensure!(!std::ptr::eq(parent, s), "source '{}' can't be cut from itself", s.name);
                anyhow::ensure!(parent.framed_from().is_none(), "source '{}' is cut from \"{from}\", which is auto-framed itself; pick the wide camera", s.name);
            }
        }
        anyhow::ensure!(self.remote.port >= 1024, "the remote control port must be 1024 or higher");
        Ok(())
    }

    /// Scenes to show: the user's own, or presets built from the sources.
    pub fn effective_scenes(&self) -> Vec<Scene> {
        if self.scenes.is_empty() {
            let names: Vec<String> = self.sources.iter().map(|s| s.name.clone()).collect();
            crate::scene::presets(&names)
        } else {
            self.scenes.clone()
        }
    }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_output_folder() -> PathBuf {
    if cfg!(target_os = "macos") {
        home_dir().join("Movies").join("Cutaway")
    } else {
        home_dir().join("Videos").join("Cutaway")
    }
}

/// Where show profiles are kept (next to the settings file).
pub fn profiles_dir(config_path: &Path) -> PathBuf {
    config_path.parent().map(|p| p.join("profiles")).unwrap_or_else(|| PathBuf::from("profiles"))
}

pub fn default_config_path() -> PathBuf {
    if cfg!(target_os = "macos") {
        home_dir().join("Library/Application Support/Cutaway/config.toml")
    } else if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home_dir)
            .join("Cutaway")
            .join("config.toml")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"))
            .join("cutaway")
            .join("config.toml")
    }
}

/// "#RRGGBB" -> 0xAARRGGBB with full alpha.
pub fn parse_color(s: &str) -> Option<u32> {
    let hex = s.trim().trim_start_matches('#');
    if hex.len() != 6 {
        return None;
    }
    u32::from_str_radix(hex, 16).ok().map(|rgb| 0xff00_0000 | rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_toml() {
        let mut cfg = Config::default();
        cfg.sources.push(SourceConfig::default());
        cfg.audio.push(AudioConfig { channels: 8, program_left: 3, program_right: 4, ..Default::default() });
        cfg.streaming.destinations.push(StreamDestination { key: "k".into(), ..Default::default() });
        cfg.streaming.destinations.push(StreamDestination { service: Service::Srt, server: "srt://h:1".into(), ..Default::default() });
        cfg.scenes = cfg.effective_scenes();
        let text = toml::to_string_pretty(&cfg).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg, back);
        back.validate().unwrap();
    }

    #[test]
    fn example_file_is_valid() {
        let text = include_str!("../cutaway.example.toml");
        let cfg: Config = toml::from_str(text).unwrap();
        cfg.validate().unwrap();
        assert_eq!(cfg.sources.len(), 7);
        assert_eq!(cfg.sources[3].framed_from(), Some("Wide"));
        assert_eq!(cfg.sources[3].framing.follow, FramingFollow::Left);
        assert!(cfg.podcast.target_lufs < 0.0 && cfg.osc.port == 8791 && cfg.captions.transcript);
        assert_eq!(cfg.audio[1].processing.comp_ratio, 3.0);
        assert_eq!(cfg.graphics[1].kind, GraphicKind::Title);
        assert_eq!(cfg.auto_switch.rules.len(), 2);
        assert_eq!(cfg.midi.mappings.len(), 6);
        assert_eq!(cfg.transition.mode, TransitionMode::Mix);
        // 0.6's settings are read as written.
        assert_eq!(cfg.sources[1].look.lut_strength, 1.0);
        assert_eq!(cfg.sources[1].ptz.presets.len(), 1);
        assert_eq!(cfg.graphics[2].kind, GraphicKind::Ticker);
        assert!(cfg.streaming.adaptive_bitrate && cfg.streaming.destinations[3].vertical);
        assert_eq!(cfg.vertical.layouts[0].mode, VerticalMode::Stack);
        assert_eq!(cfg.vertical.framing[0].zoom, 1.2);
        assert_eq!(cfg.upload.drive_files, DriveFiles::Program);
        assert_eq!(cfg.macros[0].commands.len(), 6);
        assert!(cfg.ui.check_updates && !cfg.mix.ducking && !cfg.guests.backup);
        for m in &cfg.macros {
            for c in &m.commands {
                assert!(crate::control::Action::parse(c).is_ok(), "{c}");
            }
        }
        // Every MIDI command in the example is one Cutaway understands.
        for m in &cfg.midi.mappings {
            assert!(crate::control::midi::Msg::parse(&m.message).is_some(), "{}", m.message);
            assert!(m.action.starts_with("fader:") || crate::control::Action::parse(&m.action).is_ok(), "{}", m.action);
        }
    }

    #[test]
    fn partial_file_uses_defaults() {
        let cfg: Config = toml::from_str("[[sources]]\nname = \"Wide\"\ndevice = \"#1\"\n").unwrap();
        assert_eq!(cfg.sources[0].width, 1920);
        assert_eq!(cfg.program.fps, 30);
        assert_eq!(cfg.output.container, Container::Mp4);
    }

    #[test]
    fn old_settings_still_load() {
        // Files from the GStreamer version: MKV and removed keys.
        let cfg: Config = toml::from_str("[output]\ncontainer = \"mkv\"\nmax_take_hours = 3.0\n[program]\ncompositor = \"auto\"\n").unwrap();
        assert_eq!(cfg.output.container, Container::Mp4);
    }

    #[test]
    fn rejects_bad_channel_mapping() {
        let mut cfg = Config::default();
        cfg.audio.push(AudioConfig { channels: 2, program_left: 3, ..Default::default() });
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn stream_urls() {
        let yt = StreamDestination { key: "abcd-1234".into(), ..Default::default() };
        assert_eq!(yt.url(), "rtmp://a.rtmp.youtube.com/live2/abcd-1234");
        assert!(yt.problem().is_none());
        let srt = StreamDestination { service: Service::Srt, server: "srt://10.0.0.5:9000".into(), key: "show".into(), ..Default::default() };
        assert_eq!(srt.url(), "srt://10.0.0.5:9000?streamid=show");
        let fb = StreamDestination { service: Service::Facebook, server: "rtmp://live-api-s.facebook.com/rtmp".into(), key: "k".into(), ..Default::default() };
        assert!(fb.problem().is_some());
        let nokey = StreamDestination::default();
        assert!(nokey.problem().is_some());
        let inline = StreamDestination { service: Service::CustomRtmp, server: "rtmp://host/live/show".into(), ..Default::default() };
        assert!(inline.problem().is_none());
        assert_eq!(inline.url(), "rtmp://host/live/show");
    }

    #[test]
    fn restart_detection() {
        let mut a = Config::default();
        a.audio.push(AudioConfig::default());
        let mut b = a.clone();
        b.audio[0].gain_db = -6.0;
        b.streaming.video_bitrate_kbps = 4500;
        b.output.container = Container::Mov;
        assert!(!a.needs_restart(&b), "gain/stream/output changes apply live");
        assert!(!a.lineup_changed(&b));
        b.program.fps = 60;
        assert!(a.needs_restart(&b));
    }

    #[test]
    fn sources_keep_who_they_are() {
        let mut a = Config::default();
        a.sources = vec![SourceConfig { name: "Wide".into(), ..Default::default() }, SourceConfig { name: "Host".into(), ..Default::default() }];
        a.audio.push(AudioConfig::default());
        a.assign_uids();
        let (wide, host, mic) = (a.sources[0].uid, a.sources[1].uid, a.audio[0].uid);
        assert!(wide != 0 && host != 0 && wide != host && mic != 0);
        // A copy keeps them (settings work on one); a duplicate gets its own.
        let mut b = a.clone();
        assert!(!a.lineup_changed(&b));
        b.sources.push(b.sources[0].clone());
        b.assign_uids();
        assert_eq!((b.sources[0].uid, b.sources[1].uid), (wide, host));
        assert!(b.sources[2].uid != wide && b.sources[2].uid != 0);
        assert!(a.lineup_changed(&b), "a source added");
        // Live changes don't reopen; a new device or a move does.
        let mut c = a.clone();
        c.sources[0].look.mirror = true;
        c.sources[0].record = false;
        c.audio[0].gain_db = -6.0;
        c.audio[0].record = false;
        assert!(!a.lineup_changed(&c));
        c.sources.swap(0, 1);
        assert!(a.lineup_changed(&c), "moved");
        let mut d = a.clone();
        d.sources[1].device = "test:ball".into();
        assert!(a.lineup_changed(&d));
        assert!(!a.needs_restart(&d), "sources change live now");
    }

    #[test]
    fn colors() {
        assert_eq!(parse_color("#10131a"), Some(0xff10131a));
        assert_eq!(parse_color("nope"), None);
    }
}

/// A button that runs a list of commands one after another; "wait:1.5"
/// pauses between them.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Macro {
    pub name: String,
    pub color: String,
    pub commands: Vec<String>,
}

impl Default for Macro {
    fn default() -> Self {
        Macro { name: "Macro".into(), color: "#6a4fc9".into(), commands: Vec::new() }
    }
}

/// Instant replay: the last few seconds of the program again, on air.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReplayConfig {
    /// How much to replay.
    pub seconds: f32,
    /// Slow motion speed (0.25–1).
    pub slow_speed: f32,
    /// Show "REPLAY" in a corner while it plays.
    pub label: bool,
    /// What the Replay button shows: "" for the program, a source's name
    /// (from its own recording), or "quad" for four at once.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub angle: String,
}

impl Default for ReplayConfig {
    fn default() -> Self {
        ReplayConfig { seconds: 8.0, slow_speed: 0.5, label: true, angle: String::new() }
    }
}

/// The teleprompter: a script scrolling for the presenters, on the second
/// screen or a tablet.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PrompterConfig {
    /// The script ("" with `use_rundown`: the rundown's segment notes).
    pub script: String,
    /// Use the rundown's segment names and notes as the script, and jump to
    /// each segment as the rundown moves on.
    pub use_rundown: bool,
    /// Reading speed, in words per minute.
    pub words_per_minute: f32,
    /// Text height as a fraction of the screen's height.
    pub text_size: f32,
    /// Flipped left to right, for prompter glass.
    pub mirror: bool,
    pub text_color: String,
    pub background: String,
}

impl Default for PrompterConfig {
    fn default() -> Self {
        PrompterConfig { script: String::new(), use_rundown: false, words_per_minute: 140.0, text_size: 0.075, mirror: false, text_color: "#ffffff".into(), background: "#000000".into() }
    }
}

/// Viewer comments from a live chat, picked and shown on screen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CommentsConfig {
    /// A Twitch channel to read the chat of ("" = none).
    pub twitch_channel: String,
    /// A YouTube live stream (its address or video ID; "" = none).
    pub youtube: String,
    /// A YouTube Data API key (free, from Google Cloud) to read its chat.
    pub youtube_api_key: String,
    /// Where the comment goes, as fractions of the picture.
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub text_color: String,
    pub box_color: String,
    pub box_opacity: f32,
    pub fade_ms: u32,
    /// Hide a comment after this long (0 = until hidden).
    pub hide_after_s: u32,
}

impl Default for CommentsConfig {
    fn default() -> Self {
        CommentsConfig {
            twitch_channel: String::new(),
            youtube: String::new(),
            youtube_api_key: String::new(),
            x: 0.05,
            y: 0.62,
            width: 0.42,
            text_color: "#ffffff".into(),
            box_color: "#1b1f2a".into(),
            box_opacity: 0.9,
            fade_ms: 300,
            hide_after_s: 0,
        }
    }
}

impl CommentsConfig {
    pub fn enabled(&self) -> bool {
        !self.twitch_channel.trim().is_empty() || (!self.youtube.trim().is_empty() && !self.youtube_api_key.trim().is_empty())
    }
}

/// NDI output: the program, and optionally each source, as NDI sources on
/// the network (for other computers, OBS, vMix, recorders, monitors).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NdiOutConfig {
    /// Send the program (with the recording mix).
    pub program: bool,
    /// Its name on the network (NDI adds the computer's name).
    pub name: String,
    /// Send every source too, as "<name> - <source>".
    pub sources: bool,
}

impl Default for NdiOutConfig {
    fn default() -> Self {
        NdiOutConfig { program: false, name: "Cutaway".into(), sources: false }
    }
}

/// A vertical (9:16) version of the show, made alongside the program from
/// the same sources: each scene has a tall layout of its own (worked out
/// from the scene, or chosen), and each source a framing (which part of
/// its picture fills a tall frame). Recorded into the take and/or streamed
/// to destinations marked vertical.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VerticalConfig {
    pub enabled: bool,
    pub width: u32,
    pub height: u32,
    /// Record it in the take ("Program_Vertical").
    pub record: bool,
    /// The program's graphics (titles, logos), across its width, low down.
    pub graphics: bool,
    /// Scenes' tall layouts by scene name (a scene not listed is automatic).
    pub layouts: Vec<VerticalLayout>,
    /// Sources' framing by source name (not listed: the middle).
    pub framing: Vec<VerticalFraming>,
}

impl Default for VerticalConfig {
    fn default() -> Self {
        VerticalConfig { enabled: false, width: 1080, height: 1920, record: true, graphics: true, layouts: Vec::new(), framing: Vec::new() }
    }
}

impl VerticalConfig {
    /// The tall picture size (even numbers, taller than wide).
    pub fn size(&self) -> (u32, u32) {
        let w = self.width.clamp(270, 2160) & !1;
        let h = self.height.clamp(480, 3840) & !1;
        (w, h.max(w))
    }

    pub fn layout_of(&self, scene: &str) -> VerticalLayout {
        self.layouts.iter().find(|l| l.scene.eq_ignore_ascii_case(scene)).cloned().unwrap_or_else(|| VerticalLayout { scene: scene.to_string(), ..Default::default() })
    }

    pub fn framing_of(&self, source: &str) -> VerticalFraming {
        self.framing.iter().find(|f| f.source.eq_ignore_ascii_case(source)).cloned().unwrap_or_else(|| VerticalFraming { source: source.to_string(), ..Default::default() })
    }
}

/// How a scene is laid out in the tall picture.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VerticalMode {
    /// Worked out from the scene: a full-screen source fills it, picture in
    /// picture keeps its inset (top right), anything else is stacked.
    #[default]
    Auto,
    /// The scene's sources one above another.
    Stack,
    /// One source fills it.
    One,
}

impl VerticalMode {
    pub const ALL: [VerticalMode; 3] = [VerticalMode::Auto, VerticalMode::Stack, VerticalMode::One];

    pub fn label(self) -> &'static str {
        match self {
            VerticalMode::Auto => "Automatic",
            VerticalMode::Stack => "Stacked",
            VerticalMode::One => "One source",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct VerticalLayout {
    pub scene: String,
    pub mode: VerticalMode,
    /// For "one": which source (by name).
    pub source: String,
}

/// Which part of a source's picture fills a tall frame.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct VerticalFraming {
    pub source: String,
    /// Where the window sits across the picture (0 = left edge, 1 = right).
    pub x: f32,
    /// And up and down (0 = top).
    pub y: f32,
    /// 1 = as much as fits; more is closer.
    pub zoom: f32,
}

impl Default for VerticalFraming {
    fn default() -> Self {
        VerticalFraming { source: String::new(), x: 0.5, y: 0.5, zoom: 1.0 }
    }
}

/// Uploading takes to YouTube and Google Drive after the show, with your
/// own Google OAuth client (a "Desktop app" client from Google Cloud).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UploadConfig {
    pub client_id: String,
    pub client_secret: String,
    /// Kept after signing in (treat the settings file as private).
    pub refresh_token: String,
    /// Who's signed in (shown only).
    pub account: String,
    /// What signing in allowed: "youtube", "drive".
    pub granted: Vec<String>,
    /// The program to YouTube.
    pub youtube: bool,
    pub youtube_privacy: YoutubePrivacy,
    /// "" = the show's name and the date.
    pub youtube_title: String,
    pub youtube_description: String,
    /// Markers as YouTube chapters (in the description).
    pub youtube_chapters: bool,
    /// Files to Google Drive.
    pub drive: bool,
    /// The Drive folder takes go in (one folder per take inside it).
    pub drive_folder: String,
    pub drive_files: DriveFiles,
    /// Start as soon as each take is saved (otherwise there's an Upload
    /// button for it).
    pub automatic: bool,
}

impl Default for UploadConfig {
    fn default() -> Self {
        UploadConfig {
            client_id: String::new(),
            client_secret: String::new(),
            refresh_token: String::new(),
            account: String::new(),
            granted: Vec::new(),
            youtube: false,
            youtube_privacy: YoutubePrivacy::Private,
            youtube_title: String::new(),
            youtube_description: String::new(),
            youtube_chapters: true,
            drive: false,
            drive_folder: "Cutaway".into(),
            drive_files: DriveFiles::Program,
            automatic: false,
        }
    }
}

impl UploadConfig {
    pub fn signed_in(&self) -> bool {
        !self.refresh_token.trim().is_empty()
    }

    /// Something to upload to, and signed in for it.
    pub fn ready(&self) -> bool {
        self.signed_in() && ((self.youtube && self.granted.iter().any(|g| g == "youtube")) || (self.drive && self.granted.iter().any(|g| g == "drive")))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum YoutubePrivacy {
    #[default]
    Private,
    Unlisted,
    Public,
}

impl YoutubePrivacy {
    pub const ALL: [YoutubePrivacy; 3] = [YoutubePrivacy::Private, YoutubePrivacy::Unlisted, YoutubePrivacy::Public];

    pub fn api(self) -> &'static str {
        match self {
            YoutubePrivacy::Private => "private",
            YoutubePrivacy::Unlisted => "unlisted",
            YoutubePrivacy::Public => "public",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            YoutubePrivacy::Private => "Private",
            YoutubePrivacy::Unlisted => "Unlisted",
            YoutubePrivacy::Public => "Public",
        }
    }
}

/// What goes to Google Drive.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DriveFiles {
    /// The program file.
    #[default]
    Program,
    /// The program, the vertical version and the mix (WAV).
    ProgramAndMix,
    /// Everything in the take's folder.
    Everything,
}

impl DriveFiles {
    pub const ALL: [DriveFiles; 3] = [DriveFiles::Program, DriveFiles::ProgramAndMix, DriveFiles::Everything];

    pub fn label(self) -> &'static str {
        match self {
            DriveFiles::Program => "the program",
            DriveFiles::ProgramAndMix => "the program, the vertical version and the mix",
            DriveFiles::Everything => "the whole take (every file)",
        }
    }
}
