//! Every place a person can go and every action they can take, by the
//! stable ids of the route map (shots/4-route-map.md). `was` is how each
//! could be reached in 0.9.7; the tests in nav.rs make sure none of those
//! ways in is lost as the calm redesign moves things.
//!
//! Keys, commands and the phone remote have ids of their own (`key.*`,
//! `cmd.*`, `rem.*`), so a button's `was` lists only how the button itself
//! is reached: RECORD is `main.rec.record` (a click), ⌘R is `key.record`,
//! the phone's RECORD is `rem.header.record`, OSC/MIDI/Stream Deck
//! `record` is `cmd.record`.
//!
//! `legacy: true`: still drawn in its 0.9.7 place (the old header, the
//! Settings window...). Each redesign PR flips its routes to `false` once
//! the new home in nav.rs is drawn and every `was` still reaches it.
// (Only the tests read it so far; ⌘K search will read the labels.)
#![allow(dead_code)]

/// A way in.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Input {
    /// A click, drag or hold you can see to do.
    Mouse,
    /// Right-click only.
    RightClick,
    /// Shown only while hovering.
    Hover,
    /// A key on the main window (built in or your own).
    Key,
    /// The phone remote page (or the prompter page) in a browser.
    Phone,
    StreamDeck,
    Midi,
    Osc,
    /// `/api/...` on the remote's web server.
    Http,
    /// The command line.
    Cli,
    /// Shown, nothing to press.
    Display,
}
use Input::*;

pub struct Route {
    pub id: &'static str,
    /// What it's called, for search (⌘K) and the cheat sheet.
    pub label: &'static str,
    pub was: &'static [Input],
    pub legacy: bool,
}

/// Not moved yet. (A moved route is written out with `legacy: false`.)
const fn r(id: &'static str, label: &'static str, was: &'static [Input]) -> Route {
    Route { id, label, was, legacy: true }
}

/// Moved to its new home (redesign PR 1 on: the rail, the top bar and the
/// status light).
const fn m(id: &'static str, label: &'static str, was: &'static [Input]) -> Route {
    Route { id, label, was, legacy: false }
}

/// Commands come in through your own keys, Stream Deck "Command", MIDI,
/// OSC `/cutaway/<verb>` and `/api/action?do=` (and macros).
const CMD: &[Input] = &[Key, StreamDeck, Midi, Osc, Http];

pub const ROUTES: &[Route] = &[
    // ---- main window header, right
    m("main.header.help", "Shortcuts cheat sheet", &[Mouse]),
    m("main.header.settings", "Settings", &[Mouse]),
    m("main.header.update", "Update button", &[Mouse]),
    m("win.update", "Update", &[Mouse]),
    m("main.header.typing", "Typing, keys paused", &[Display]),
    m("main.header.log", "Log button", &[Mouse]),
    m("panel.log", "Log", &[Mouse]),
    m("main.header.health", "Health button", &[Mouse]),
    m("win.health", "Health", &[Mouse]),
    m("main.header.check", "Check button", &[Mouse]),
    m("dlg.precheck", "Pre-show check", &[Mouse]),
    m("main.header.screen", "Second screen button", &[Mouse]),
    r("win.display", "Second screen", &[Mouse]),
    m("main.header.webcam", "Webcam (virtual camera)", &[Mouse]),
    m("main.header.clock", "Clock, 12 or 24 hour", &[Mouse]),
    // ---- header, left: show, recording, stream
    m("main.header.logo", "About Cutaway", &[Mouse]),
    m("win.about", "About", &[Mouse]),
    m("main.header.show", "Show menu", &[Mouse]),
    m("menu.show.switch", "Switch to another show", &[Mouse]),
    m("dlg.confirm.switch", "Switch show?", &[Mouse]),
    m("menu.show.save_as", "Save as a show", &[Mouse]),
    m("main.rec.record", "Record", &[Mouse]),
    m("main.rec.stop", "Stop recording", &[Mouse]),
    m("dlg.confirm.stoprec", "Stop recording?", &[Mouse]),
    m("main.rec.marker", "Marker", &[Mouse]),
    m("dlg.marker_name", "Named marker", &[Key]),
    m("menu.markers", "Markers so far", &[Mouse]),
    m("main.rec.clip", "Clip", &[Mouse]),
    r("main.rec.replay", "Replay", &[Mouse]),
    r("ctx.replay", "Replay angle and slow motion", &[RightClick]),
    m("main.rec.recent", "Recent takes", &[Mouse]),
    r("main.rec.takecard", "Last take", &[Mouse]),
    m("main.stream.golive", "Go live", &[Mouse]),
    m("main.stream.setup", "Set up streaming", &[Mouse]),
    m("main.stream.live", "End the stream", &[Mouse]),
    m("dlg.confirm.endstream", "End the stream?", &[Mouse]),
    r("main.stream.panel", "Stream", &[Display]),
    r("main.stream.scheduled", "Scheduled go-live", &[Display]),
    // ---- monitors and Edit layout
    r("mon.engine_error", "Engine problem", &[Mouse]),
    Route { id: "mon.starting", label: "Starting", was: &[Display], legacy: false },
    r("mon.waiting.rescue", "Recovered take", &[Mouse]),
    Route { id: "mon.preview", label: "Preview", was: &[Display], legacy: false },
    Route { id: "mon.preview.take", label: "Take", was: &[Mouse], legacy: false },
    Route { id: "mon.preview.cut", label: "Cut", was: &[Mouse], legacy: false },
    r("mon.tall", "Vertical monitor", &[Mouse]),
    Route { id: "mon.program", label: "Program", was: &[Display], legacy: false },
    r("mon.edit.overlay", "Move and size on the picture", &[Mouse]),
    r("mon.edit.toolbar", "Edit layout toolbar", &[Mouse]),
    r("edit.done", "Done editing", &[Mouse]),
    r("edit.save_scenes", "Save scenes", &[Mouse]),
    r("edit.redo", "Redo", &[Mouse]),
    r("edit.undo", "Undo", &[Mouse]),
    r("edit.frame.add", "Add source", &[Mouse]),
    r("edit.frame.remove", "Remove source", &[Mouse]),
    r("edit.frame.front", "To front", &[Mouse]),
    r("edit.frame.full", "Full frame", &[Mouse]),
    r("edit.frame.fitfill", "Fit or fill", &[Mouse]),
    r("edit.frame.zoom", "Zoom", &[Mouse]),
    r("edit.frame.reset", "Reset picture", &[Mouse]),
    r("edit.scene.name", "Scene name", &[Mouse]),
    r("menu.edit.audio", "Scene audio", &[Mouse]),
    r("menu.edit.cues", "Scene cues", &[Mouse]),
    r("menu.edit.newscene", "New scene", &[Mouse]),
    r("edit.scene.duplicate", "Duplicate scene", &[Mouse]),
    r("edit.scene.delete", "Delete scene", &[Mouse]),
    // ---- multiview
    Route { id: "mv.tile", label: "Multiview tile", was: &[Mouse], legacy: false },
    Route { id: "mv.media", label: "Media play, next, restart", was: &[Mouse], legacy: false },
    Route { id: "mv.framing", label: "Auto-framing", was: &[Mouse], legacy: false },
    Route { id: "ctx.mv.framing", label: "Follow and shot", was: &[RightClick], legacy: false },
    Route { id: "ctx.mv.playlist", label: "Playlist", was: &[RightClick], legacy: false },
    Route { id: "ctx.mv.split", label: "Split into close-ups", was: &[RightClick], legacy: false },
    r("mv.guest_warn", "Guest link warning", &[Hover]),
    // ---- scene bank
    Route { id: "bank.transition", label: "Transition", was: &[Mouse], legacy: false },
    Route { id: "bank.ftb", label: "Fade to black", was: &[Mouse], legacy: false },
    Route { id: "bank.ms", label: "Transition length", was: &[Mouse], legacy: false },
    Route { id: "bank.preview", label: "Preview on or off", was: &[Mouse], legacy: false },
    Route { id: "bank.auto", label: "Auto switching", was: &[Mouse], legacy: false },
    Route { id: "bank.cc", label: "Captions", was: &[Mouse], legacy: false },
    r("bank.edit", "Edit layout", &[Mouse]),
    Route { id: "bank.scene", label: "Scenes", was: &[Mouse], legacy: false },
    Route { id: "bank.overflow", label: "More scenes", was: &[Mouse], legacy: false },
    // ---- right-column panels
    r("panel.heading.fold", "Fold a panel", &[Mouse]),
    r("panel.heading.reorder", "Move a panel", &[Mouse]),
    r("panel.heading.setup", "Set up a panel", &[Mouse]),
    r("panel.column.width", "Panel column width", &[Mouse]),
    r("panel.rundown", "Rundown", &[Mouse]),
    r("menu.rundown.goto", "Go to a segment", &[Mouse]),
    r("panel.guests", "Guests", &[Mouse]),
    r("win.guest_qr", "Guest link QR code", &[Mouse]),
    r("panel.sound", "Sound pads", &[Mouse]),
    r("panel.sound.talk", "Talkback", &[Mouse]),
    r("panel.graphics", "Graphics", &[Mouse]),
    r("panel.graphics.edit_title", "Edit a title", &[Mouse]),
    r("dlg.title", "Title", &[Mouse]),
    r("panel.graphics.reload", "Reload a web graphic", &[Mouse]),
    r("panel.graphics.rotation", "Graphics taking turns", &[Mouse]),
    r("ctx.clock", "Start, pause and reset a timer", &[RightClick]),
    r("panel.macros", "Macros", &[Mouse]),
    r("panel.ptz", "PTZ", &[Mouse]),
    r("ctx.ptz.preset", "PTZ presets", &[Mouse, RightClick]),
    r("panel.comments", "Viewer comments", &[Mouse]),
    r("panel.prompter", "Teleprompter", &[Mouse]),
    r("panel.uploads", "Uploads", &[Mouse]),
    r("panel.audio", "Audio", &[Mouse]),
    r("audio.fader", "Level", &[Mouse]),
    r("audio.onoff", "Mic on or off", &[Mouse]),
    r("audio.solo", "Listen to one input", &[Mouse]),
    r("audio.phones", "Headphone volume", &[Mouse]),
    // ---- windows and dialogs
    r("dlg.sync", "Measure lip sync", &[Mouse]),
    m("dlg.confirm.quit", "Quit while recording?", &[Mouse]),
    r("win.guide", "First-show guide", &[Mouse]),
    // ---- the Settings window
    m("set.frame.titlebar", "Settings title bar", &[Mouse]),
    r("set.frame.find", "Find a setting", &[Mouse]),
    r("set.frame.revert", "Revert", &[Mouse]),
    r("set.frame.problem", "Go to the problem", &[Mouse]),
    r("set.frame.ok", "OK", &[Mouse]),
    r("set.frame.apply", "Apply", &[Mouse]),
    r("set.frame.cancel", "Cancel", &[Mouse]),
    r("set.modal.discard", "Discard changes?", &[Mouse]),
    r("set.modal.reset", "Reset settings", &[Mouse]),
    r("set.modal.remove", "Remove?", &[Mouse]),
    r("set.inputs.sources", "Video sources", &[Mouse]),
    r("set.inputs.audio", "Audio inputs", &[Mouse]),
    r("set.inputs.guests", "Guests", &[Mouse]),
    r("set.sound.mixing", "Mixing", &[Mouse]),
    r("set.sound.pads", "Sound pads", &[Mouse]),
    r("set.sound.syrinscape", "Syrinscape", &[Mouse]),
    r("set.look.program", "Program picture", &[Mouse]),
    r("set.look.graphics", "Graphics", &[Mouse]),
    r("set.look.captions", "Captions", &[Mouse]),
    r("set.show.scenes", "Scenes", &[Mouse]),
    r("set.show.switching", "Switching", &[Mouse]),
    r("set.show.rundown", "Rundown", &[Mouse]),
    r("set.show.macros", "Macros", &[Mouse]),
    r("set.show.prompter", "Teleprompter", &[Mouse]),
    r("set.show.comments", "Viewer comments", &[Mouse]),
    r("set.output.recording", "Recording", &[Mouse]),
    r("set.output.streaming", "Streaming", &[Mouse]),
    r("set.output.vertical", "Vertical version", &[Mouse]),
    r("set.output.upload", "Upload", &[Mouse]),
    r("set.output.outputs", "Screens and webcam", &[Mouse]),
    r("set.general.general", "General", &[Mouse]),
    r("set.general.control", "Control", &[Mouse]),
    r("set.general.shortcuts", "Shortcuts", &[Mouse]),
    r("set.general.diagnostics", "Diagnostics", &[Mouse]),
    // ---- keys
    m("key.settings", "Settings key", &[Key]),
    r("key.undo", "Undo key", &[Key]),
    r("key.redo", "Redo key", &[Key]),
    m("key.record", "Record key", &[Key]),
    r("key.transition", "Transition key", &[Key]),
    r("key.edit", "Edit layout key", &[Key]),
    m("key.marker", "Marker key", &[Key]),
    m("key.marker_named", "Named marker key", &[Key]),
    r("key.auto", "Auto switching key", &[Key]),
    r("key.take", "Take key", &[Key]),
    r("key.cut", "Cut key", &[Key]),
    r("key.captions", "Captions key", &[Key]),
    r("key.segment_next", "Next segment key", &[Key]),
    r("key.segment_prev", "Previous segment key", &[Key]),
    m("key.clip", "Clip key", &[Key]),
    r("key.talkback", "Talkback key", &[Key]),
    Route { id: "key.back", label: "Last scene", was: &[Key], legacy: false },
    r("key.number", "Scene and source number keys", &[Key]),
    r("key.graphic", "Graphic keys", &[Key]),
    r("key.pad", "Sound pad keys", &[Key]),
    r("key.custom", "Your own keys", &[Key]),
    r("key.display", "Second screen keys", &[Key]),
    r("key.prompter_page", "Prompter page keys", &[Key]),
    // ---- commands
    r("cmd.scene", "scene", CMD),
    r("cmd.source", "source", CMD),
    r("cmd.take", "take", CMD),
    r("cmd.cut", "cut", CMD),
    r("cmd.transition", "transition", CMD),
    r("cmd.ftb", "ftb", CMD),
    m("cmd.record", "record", CMD),
    m("cmd.stop", "stop", CMD),
    m("cmd.golive", "golive", CMD),
    m("cmd.endstream", "endstream", CMD),
    m("cmd.marker", "marker", CMD),
    r("cmd.graphic", "graphic", CMD),
    r("cmd.title", "title", CMD),
    r("cmd.captions", "captions", CMD),
    r("cmd.auto", "auto", CMD),
    r("cmd.gain", "gain", CMD),
    r("cmd.mute", "mute", CMD),
    r("cmd.talkback", "talkback", CMD),
    r("cmd.pad", "pad", CMD),
    r("cmd.syrinscape", "syrinscape", CMD),
    r("cmd.timer", "timer", CMD),
    r("cmd.rundown", "rundown", CMD),
    r("cmd.media", "media", CMD),
    r("cmd.clip:N", "clip:N", CMD),
    r("cmd.replay:N", "replay:N", CMD),
    r("cmd.macro", "macro", CMD),
    r("cmd.wait", "wait", CMD),
    r("cmd.ptz", "ptz", CMD),
    r("cmd.prompter", "prompter", CMD),
    r("cmd.comment", "comment", CMD),
    r("cmd.guest", "guest", CMD),
    // ---- the phone remote and the prompter page
    r("rem.pin", "PIN", &[Phone]),
    r("rem.header.record", "Phone: Record", &[Phone]),
    r("rem.header.live", "Phone: Go live", &[Phone]),
    r("rem.header.marker", "Phone: Marker", &[Phone]),
    r("rem.header.clip", "Phone: Clip", &[Phone]),
    r("rem.header.replay", "Phone: Replay", &[Phone]),
    r("rem.header.ftb", "Phone: Fade to black", &[Phone]),
    r("rem.status", "Phone: status", &[Phone]),
    r("rem.monitor", "Phone: program picture", &[Phone]),
    r("rem.cut_take", "Phone: Cut and Take", &[Phone]),
    r("rem.rundown", "Phone: rundown", &[Phone]),
    r("rem.scenes", "Phone: scenes", &[Phone]),
    r("rem.switching", "Phone: switching", &[Phone]),
    r("rem.graphics", "Phone: graphics", &[Phone]),
    r("rem.macros", "Phone: macros", &[Phone]),
    r("rem.ptz", "Phone: PTZ", &[Phone]),
    r("rem.prompter", "Phone: teleprompter", &[Phone]),
    r("rem.comments", "Phone: viewer comments", &[Phone]),
    r("rem.timers", "Phone: timers", &[Phone]),
    r("rem.sound", "Phone: sound pads", &[Phone]),
    r("rem.audio", "Phone: audio", &[Phone]),
    r("rem.media", "Phone: media", &[Phone]),
    r("rem.tally_links", "Phone: tally lights", &[Phone]),
    r("rem.tally", "Tally light page", &[Phone]),
    r("prompt.page", "Prompter page", &[Phone]),
    // ---- HTTP
    r("api.state", "/api/state", &[Http]),
    r("api.ws", "/api/ws", &[Http]),
    r("api.action", "/api/action", &[Http]),
    r("api.prompter", "/api/prompter", &[Http]),
    r("api.program_jpg", "/api/program.jpg", &[Http]),
    r("api.tally", "/api/tally", &[Http]),
    r("api.key_png", "/api/key images", &[Http]),
    r("api.icon", "/icon.png", &[Http]),
    // ---- Stream Deck (its 26 actions), OSC and MIDI
    r("sd.*", "Stream Deck actions", &[StreamDeck]),
    r("osc.in", "OSC commands", &[Osc]),
    r("osc.feedback", "OSC feedback", &[Osc]),
    r("midi.in", "MIDI notes and faders", &[Midi]),
    r("midi.learn", "MIDI learn", &[Mouse]),
    r("midi.lights", "MIDI pad lights", &[Midi]),
    // ---- the command line
    r("cli.version", "--version", &[Cli]),
    r("cli.list_devices", "--list-devices", &[Cli]),
    r("cli.vcam_install", "--vcam-install", &[Cli]),
    r("cli.vcam_uninstall", "--vcam-uninstall", &[Cli]),
    r("cli.config", "--config", &[Cli]),
    r("cli.demo", "--demo", &[Cli]),
    r("cli.init", "--init", &[Cli]),
    r("cli.probe_encoders", "--probe-encoders", &[Cli]),
    r("cli.stream_to", "--stream-to", &[Cli]),
    r("cli.measure_sync", "--measure-sync", &[Cli]),
    r("cli.headless", "--headless", &[Cli]),
    r("cli.restream_test", "--restream-test", &[Cli]),
    r("cli.ui_test", "--ui-test", &[Cli]),
];

/// The route with this id.
pub fn route(id: &str) -> Option<&'static Route> {
    ROUTES.iter().find(|r| r.id == id)
}
