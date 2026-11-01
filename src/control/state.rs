//! A snapshot of the show for controllers: the phone remote and API
//! (as JSON), the Stream Deck plugin (over WebSocket), OSC feedback and
//! MIDI pad lights all read the same thing.

use crate::engine::streamer::{DestState, StreamStatus};
use serde::Serialize;

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct ShowState {
    pub version: String,
    /// The show's name (its profile).
    pub show_name: String,
    pub recording: bool,
    pub rec_seconds: f64,
    pub live: bool,
    pub live_seconds: f64,
    /// The stream as a whole and each destination, so a controller can say
    /// "1 of 2 live" or "Connecting" rather than a plain LIVE.
    pub stream: StreamState,
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
    /// Recording time left on the drive ("1 h 53 min"), the same figure as in `disk`.
    pub disk_left: String,
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
    /// The pre-show check, while it's open on the desk (null otherwise).
    pub precheck: Option<PrecheckState>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct StreamState {
    /// "off", "connecting" (nothing live yet), "live" (every destination
    /// live), "trouble" (some aren't) or "ending".
    pub state: String,
    /// Destinations live, out of those turned on.
    pub live_count: usize,
    pub total: usize,
    pub destinations: Vec<DestinationState>,
    /// The video bitrate, while it's lowered for a struggling connection.
    pub lowered_kbps: Option<u32>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct DestinationState {
    pub name: String,
    /// "connecting", "live", "retrying" or "failed".
    pub state: String,
    /// In words, as the desk's Stream panel says it ("Retrying in 4 s").
    pub text: String,
    /// Megabits a second, to one decimal (while live).
    pub mbps: f64,
    /// The connection can't keep up and frames are being dropped.
    pub congested: bool,
    /// Why it isn't live, when there's a reason.
    pub message: Option<String>,
}

impl StreamState {
    /// From the engine's stream status, counted the way the desk counts it
    /// (destinations turned off don't count).
    pub fn from_status(st: &StreamStatus, streaming: bool) -> StreamState {
        if !streaming && !st.stopping {
            return StreamState { state: "off".into(), ..Default::default() };
        }
        let on: Vec<_> = st.destinations.iter().filter(|d| d.state != DestState::Off).collect();
        let live_count = on.iter().filter(|d| d.state == DestState::Live).count();
        let trying = on.iter().any(|d| matches!(d.state, DestState::Connecting | DestState::Retrying { .. }));
        let state = if st.stopping {
            "ending"
        } else if live_count == 0 && trying {
            "connecting"
        } else if live_count == on.len() && live_count > 0 {
            "live"
        } else {
            "trouble"
        };
        let destinations = on
            .iter()
            .map(|d| {
                let text = crate::engine::streamer::describe_dest(d);
                DestinationState {
                    name: d.name.clone(),
                    state: match d.state {
                        DestState::Connecting => "connecting",
                        DestState::Live => "live",
                        DestState::Retrying { .. } => "retrying",
                        DestState::Failed | DestState::Off => "failed",
                    }
                    .into(),
                    text,
                    mbps: if d.state == DestState::Live { (d.kbps / 100.0).round() / 10.0 } else { 0.0 },
                    congested: d.congested,
                    message: d.message.clone().filter(|_| d.state != DestState::Live),
                }
            })
            .collect();
        StreamState { state: state.into(), live_count, total: on.len(), destinations, lowered_kbps: st.lowered_kbps }
    }
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct PrecheckState {
    /// What starts when it passes: "record", "live", or "" (just a check).
    pub then: String,
    /// The desk's one-line verdict ("1 warning: you can still start").
    pub verdict: String,
    /// The worst result so far: "ok", "warn" or "fail".
    pub level: String,
    /// Stream servers still being asked.
    pub waiting: usize,
    /// What needs a look (results that aren't fine).
    pub items: Vec<CheckLine>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct CheckLine {
    /// "warn" or "fail".
    pub level: String,
    pub what: String,
    pub detail: String,
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
    /// The latest comments, oldest first, so they can be read before one
    /// goes on air (`comment:<id>` shows one).
    pub recent: Vec<CommentInfo>,
}

#[derive(Serialize, Clone, Debug, Default, PartialEq)]
pub struct CommentInfo {
    pub id: u64,
    pub author: String,
    pub text: String,
    /// "Twitch" or "YouTube".
    pub platform: String,
    /// On screen now.
    pub showing: bool,
    /// Not shown yet ("Show next" takes the oldest of these).
    pub new: bool,
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
    /// The same, as the desk says it: "on time", "1:05 over", "0:30 under".
    pub drift_text: String,
}

impl RundownState {
    /// The desk's wording for drift (within 5 s counts as on time).
    pub fn drift_words(drift: f64) -> String {
        let d = drift.round() as i64;
        let span = |s: i64| crate::engine::timers::fmt_span(std::time::Duration::from_secs(s.unsigned_abs()), false);
        if d.abs() < 5 {
            "on time".into()
        } else if d > 0 {
            format!("{} over", span(d))
        } else {
            format!("{} under", span(d))
        }
    }
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
    /// A who's-on-air title (it follows the program).
    pub names: bool,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Service;
    use crate::engine::streamer::DestStatus;
    use std::time::{Duration, Instant};

    fn dest(name: &str, state: DestState) -> DestStatus {
        DestStatus { name: name.into(), service: Service::CustomRtmp, state, kbps: 6043.0, message: Some("refused".into()), attempts: 3, congested: false }
    }

    #[test]
    fn stream_state_counts_destinations_like_the_desk() {
        let mut st = StreamStatus::default();
        assert_eq!(StreamState::from_status(&st, false).state, "off");
        // Both trying: connecting, not live.
        st.destinations = vec![dest("YouTube", DestState::Connecting), dest("Twitch", DestState::Retrying { at: Instant::now() + Duration::from_secs(4) })];
        let s = StreamState::from_status(&st, true);
        assert_eq!((s.state.as_str(), s.live_count, s.total), ("connecting", 0, 2));
        assert!(s.destinations[1].text.starts_with("Retrying in") && s.destinations[1].text.ends_with("(try 4)"), "{}", s.destinations[1].text);
        // One live: trouble, 1 of 2, with its bitrate.
        st.destinations[0].state = DestState::Live;
        let s = StreamState::from_status(&st, true);
        assert_eq!((s.state.as_str(), s.live_count, s.total), ("trouble", 1, 2));
        assert_eq!(s.destinations[0].mbps, 6.0);
        assert_eq!(s.destinations[0].message, None);
        // Both failing is trouble too, never a healthy LIVE.
        st.destinations = vec![dest("YouTube", DestState::Failed), dest("Twitch", DestState::Failed)];
        assert_eq!(StreamState::from_status(&st, true).state, "trouble");
        // Destinations turned off don't count.
        st.destinations = vec![dest("YouTube", DestState::Live), dest("Twitch", DestState::Off)];
        let s = StreamState::from_status(&st, true);
        assert_eq!((s.state.as_str(), s.live_count, s.total), ("live", 1, 1));
        st.stopping = true;
        assert_eq!(StreamState::from_status(&st, true).state, "ending");
    }

    #[test]
    fn drift_reads_like_the_desk() {
        assert_eq!(RundownState::drift_words(65.2), "1:05 over");
        assert_eq!(RundownState::drift_words(-30.0), "0:30 under");
        assert_eq!(RundownState::drift_words(4.0), "on time");
        assert_eq!(RundownState::drift_words(3725.0), "1:02:05 over");
    }
}
