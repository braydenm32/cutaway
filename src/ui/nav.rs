//! The one place the calm redesign's navigation is written down: the areas
//! down the left rail, each area's tabs (and which sit under More▾), each
//! tab's sections with the routes (routes.rs) that live there, and the
//! parts of the window that aren't an area's page. Order inside every area
//! follows the usage audit's ranking (shots/1-usage-audit.md).
//!
//! The rail and the top-bar tabs are drawn from it (ui/shell.rs), and ⌘K
//! search reads it (ui/search.rs); the "Set up ›" links and the status
//! light's "fix it" links will be too. The tests below check it against every route there is, so
//! nothing gets lost on the way.
// (Parts are still only read by the tests until later redesign PRs draw them.)
#![allow(dead_code)]

use crate::settings::Section as Page;

/// The icon an area has on the rail.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Monitor,
    Camera,
    Layers,
    Wave,
    Text,
    RecordDot,
    Dots,
}

pub struct Area {
    pub id: &'static str,
    pub label: &'static str,
    pub icon: Icon,
    /// ⌘1…⌘7.
    pub key: Option<u8>,
    pub tabs: &'static [Tab],
}

pub struct Tab {
    pub id: &'static str,
    pub label: &'static str,
    /// Under the More▾ at the end of the area's tab row, not a tab of its own.
    pub more: bool,
    pub sections: &'static [Sect],
}

/// A section of a tab. `routes`: route ids that live here. `help`: the old
/// Settings pages whose settings (help.rs) ⌘K search finds here.
pub struct Sect {
    pub id: &'static str,
    pub label: &'static str,
    pub routes: &'static [&'static str],
    pub help: &'static [Page],
}

pub const AREAS: &[Area] = &[
    Area { id: "show", label: "Show", icon: Icon::Monitor, key: Some(1), tabs: &[
        Tab { id: "switch", label: "Switch", more: false, sections: &[
            Sect { id: "program", label: "Program", help: &[], routes: &[
                "mon.program", "mon.preview", "mon.preview.take", "mon.preview.cut", "mon.tall",
                "mon.starting", "mon.engine_error", "mon.waiting.rescue", "main.rec.takecard"] },
            Sect { id: "multiview", label: "Multiview", help: &[], routes: &[
                "mv.tile", "mv.media", "mv.framing", "ctx.mv.framing", "ctx.mv.playlist", "ctx.mv.split", "mv.guest_warn"] },
            Sect { id: "bank", label: "Scene bank", help: &[], routes: &[
                // CUT, MIX and the length, Wipe/Push/Dip/Stinger under ⋯ (usage
                // audit rank 19), Fade to black set apart at the end; under
                // them Preview, AUTO (greyed until it has rules), CC and ⟲ Back.
                "bank.scene", "bank.overflow", "bank.transition", "bank.ms", "bank.preview", "bank.cc",
                "bank.ftb", "bank.auto", "main.rec.replay", "ctx.replay", "key.back"] },
            Sect { id: "column", label: "Panels", help: &[], routes: &[
                "panel.audio", "audio.fader", "audio.onoff", "audio.solo", "audio.phones",
                "panel.graphics", "panel.graphics.edit_title", "dlg.title", "panel.graphics.reload", "panel.graphics.rotation", "ctx.clock",
                "main.stream.panel", "panel.uploads", "panel.rundown", "menu.rundown.goto",
                "panel.guests", "win.guest_qr", "panel.ptz", "ctx.ptz.preset", "panel.comments", "panel.prompter",
                "panel.macros", "panel.sound", "panel.sound.talk",
                "panel.heading.fold", "panel.heading.reorder", "panel.heading.setup", "panel.column.width"] },
        ]},
        Tab { id: "edit", label: "Edit layout", more: false, sections: &[
            Sect { id: "canvas", label: "Layout", help: &[], routes: &["bank.edit", "mon.edit.overlay", "mon.edit.toolbar", "edit.done", "edit.save_scenes", "edit.undo", "edit.redo"] },
            Sect { id: "frame", label: "Frame", help: &[], routes: &["edit.frame.add", "edit.frame.remove", "edit.frame.front", "edit.frame.full", "edit.frame.fitfill", "edit.frame.zoom", "edit.frame.reset"] },
            Sect { id: "scene", label: "Scene", help: &[], routes: &["edit.scene.name", "menu.edit.audio", "menu.edit.cues", "menu.edit.newscene", "edit.scene.duplicate", "edit.scene.delete"] },
        ]},
    ]},
    Area { id: "sources", label: "Sources", icon: Icon::Camera, key: Some(2), tabs: &[
        Tab { id: "cameras", label: "Cameras", more: false, sections: &[
            Sect { id: "list", label: "Your sources", help: &[Page::Sources], routes: &["set.inputs.sources"] } ]},
        // PTZ: rank 11, 5–10 visits before a show
        Tab { id: "closeups", label: "Close-ups & PTZ", more: false, sections: &[
            Sect { id: "split", label: "Close-ups from one camera", help: &[Page::Sources], routes: &[] },
            Sect { id: "ptz", label: "PTZ presets", help: &[Page::Sources], routes: &[] },
            Sect { id: "framing", label: "Auto-framing", help: &[Page::Sources], routes: &[] } ]},
        Tab { id: "people", label: "People & names", more: false, sections: &[
            Sect { id: "who", label: "Who's in each picture", help: &[Page::Sources], routes: &[] } ]},
        // 0 visits for every persona
        Tab { id: "guests", label: "Guests", more: true, sections: &[
            Sect { id: "guests", label: "Remote guests", help: &[Page::Guests], routes: &["set.inputs.guests", "mv.guest_warn"] } ]},
        Tab { id: "network", label: "Network & phone feeds", more: true, sections: &[
            Sect { id: "feeds", label: "SRT, RTMP, NDI", help: &[Page::Sources], routes: &[] } ]},
        Tab { id: "media", label: "Media & playlists", more: true, sections: &[
            Sect { id: "media", label: "Media players", help: &[Page::Sources], routes: &[] } ]},
    ]},
    Area { id: "scenes", label: "Scenes", icon: Icon::Layers, key: Some(3), tabs: &[
        Tab { id: "scenes", label: "Scenes", more: false, sections: &[
            Sect { id: "list", label: "Your scenes", help: &[Page::Scenes], routes: &["set.show.scenes"] } ]},
        Tab { id: "switching", label: "Switching", more: false, sections: &[
            Sect { id: "transition", label: "Transition and Preview", help: &[Page::Switching], routes: &["set.show.switching"] } ]},
        Tab { id: "program", label: "Program picture", more: true, sections: &[
            Sect { id: "picture", label: "Size, frame rate, background", help: &[Page::Program], routes: &["set.look.program"] } ]},
        Tab { id: "auto", label: "Auto-switching", more: true, sections: &[
            Sect { id: "rules", label: "Rules", help: &[Page::Switching], routes: &[] } ]},
        Tab { id: "stinger", label: "Stinger & fade to black", more: true, sections: &[
            Sect { id: "stinger", label: "Stinger and FTB", help: &[Page::Switching], routes: &[] } ]},
    ]},
    Area { id: "sound", label: "Sound", icon: Icon::Wave, key: Some(4), tabs: &[
        Tab { id: "mixer", label: "Mixer", more: false, sections: &[
            Sect { id: "mix", label: "Mix and loudness", help: &[Page::Mixing], routes: &["set.sound.mixing"] } ]},
        Tab { id: "inputs", label: "Inputs", more: false, sections: &[
            Sect { id: "inputs", label: "Mics and inputs", help: &[Page::Audio], routes: &["set.inputs.audio", "dlg.sync"] } ]},
        Tab { id: "pads", label: "Pads", more: false, sections: &[
            Sect { id: "pads", label: "Sound pads", help: &[Page::Pads], routes: &["set.sound.pads"] } ]},
        Tab { id: "phones", label: "Headphones & talkback", more: true, sections: &[
            Sect { id: "phones", label: "Headphones and talkback", help: &[Page::Mixing], routes: &[] } ]},
        Tab { id: "syrinscape", label: "Syrinscape", more: true, sections: &[
            Sect { id: "syrinscape", label: "Syrinscape", help: &[Page::Syrinscape], routes: &["set.sound.syrinscape"] } ]},
    ]},
    Area { id: "graphics", label: "Graphics", icon: Icon::Text, key: Some(5), tabs: &[
        Tab { id: "titles", label: "Titles & overlays", more: false, sections: &[
            Sect { id: "list", label: "Your graphics", help: &[Page::Graphics], routes: &["set.look.graphics"] } ]},
        Tab { id: "timers", label: "Clocks & timers", more: true, sections: &[
            Sect { id: "timers", label: "Clocks and timers", help: &[Page::Graphics], routes: &[] } ]},
        // rank 20, 0 visits
        Tab { id: "captions", label: "Captions", more: true, sections: &[
            Sect { id: "captions", label: "Live captions", help: &[Page::Captions], routes: &["set.look.captions"] } ]},
        Tab { id: "turns", label: "Taking turns", more: true, sections: &[
            Sect { id: "turns", label: "Graphics taking turns", help: &[Page::Graphics], routes: &[] } ]},
        Tab { id: "web", label: "Tickers & web overlays", more: true, sections: &[
            Sect { id: "web", label: "Tickers and web pages", help: &[Page::Graphics], routes: &[] } ]},
    ]},
    Area { id: "record", label: "Record", icon: Icon::RecordDot, key: Some(6), tabs: &[
        Tab { id: "takes", label: "Takes", more: false, sections: &[
            Sect { id: "takes", label: "Your takes", help: &[], routes: &["main.rec.recent"] } ]},
        Tab { id: "recording", label: "Recording", more: false, sections: &[
            Sect { id: "quality", label: "Quality", help: &[Page::Recording], routes: &["set.output.recording"] },
            Sect { id: "editing", label: "Editing", help: &[Page::Recording], routes: &[] },
            Sect { id: "podcast", label: "Podcast", help: &[Page::Recording], routes: &[] },
            Sect { id: "check", label: "Before you start", help: &[Page::General], routes: &["dlg.precheck"] } ]},
        // always a tab; an empty state until a destination exists
        Tab { id: "streaming", label: "Streaming", more: false, sections: &[
            Sect { id: "where", label: "Where to stream", help: &[Page::Streaming], routes: &["set.output.streaming", "main.stream.setup"] } ]},
        // rank 19
        Tab { id: "screens", label: "Screens & webcam", more: true, sections: &[
            Sect { id: "screens", label: "Second screen and webcam", help: &[Page::Outputs], routes: &["set.output.outputs", "win.display", "main.header.screen", "main.header.webcam"] } ]},
        Tab { id: "upload", label: "Upload", more: true, sections: &[
            Sect { id: "upload", label: "Upload after the show", help: &[Page::Upload], routes: &["set.output.upload"] } ]},
        Tab { id: "vertical", label: "Vertical version", more: true, sections: &[
            Sect { id: "vertical", label: "Vertical version", help: &[Page::Vertical], routes: &["set.output.vertical"] } ]},
        Tab { id: "scheduled", label: "Scheduled", more: true, sections: &[
            Sect { id: "scheduled", label: "Scheduled go-live", help: &[Page::Streaming], routes: &["main.stream.scheduled"] } ]},
    ]},
    Area { id: "more", label: "More", icon: Icon::Dots, key: Some(7), tabs: &[
        Tab { id: "control", label: "Control", more: false, sections: &[
            Sect { id: "remote", label: "Phone remote", help: &[Page::Control], routes: &["set.general.control"] },
            Sect { id: "streamdeck", label: "Stream Deck", help: &[Page::Control], routes: &[] },
            Sect { id: "oscmidi", label: "OSC and MIDI", help: &[Page::Control], routes: &["midi.learn"] },
            Sect { id: "shortcuts", label: "Shortcuts", help: &[Page::Shortcuts], routes: &["set.general.shortcuts", "key.custom"] } ]},
        Tab { id: "tools", label: "Show tools", more: false, sections: &[
            Sect { id: "rundown", label: "Rundown", help: &[Page::Rundown], routes: &["set.show.rundown"] },
            Sect { id: "macros", label: "Macros", help: &[Page::Macros], routes: &["set.show.macros"] },
            Sect { id: "prompter", label: "Teleprompter", help: &[Page::Prompter], routes: &["set.show.prompter"] },
            Sect { id: "comments", label: "Viewer comments", help: &[Page::Comments], routes: &["set.show.comments"] },
            Sect { id: "replay", label: "Replay", help: &[Page::Switching], routes: &[] } ]},
        Tab { id: "app", label: "App", more: false, sections: &[
            Sect { id: "shows", label: "Shows", help: &[Page::General], routes: &["menu.show.switch", "menu.show.save_as", "set.modal.reset"] },
            Sect { id: "general", label: "General", help: &[Page::General], routes: &["set.general.general"] },
            Sect { id: "diagnostics", label: "Diagnostics", help: &[Page::Diagnostics], routes: &["set.general.diagnostics"] },
            Sect { id: "about", label: "Updates & about", help: &[Page::General], routes: &["win.update", "win.about"] },
            Sect { id: "help", label: "Help & first-show guide", help: &[], routes: &["main.header.help", "win.guide"] } ]},
    ]},
];

/// The parts of the window that are not an area's page.
pub const SHELL: &[(&str, &[&str])] = &[
    ("rail", &["main.header.settings", "main.header.logo", "main.header.show", "main.header.update"]),
    ("status", &["main.header.health", "main.header.log", "panel.log", "win.health", "main.header.check",
        "set.frame.problem", "main.header.clock", "main.header.typing",
        "main.rec.record", "main.rec.stop", "main.rec.marker", "menu.markers", "dlg.marker_name", "main.rec.clip",
        "main.stream.golive", "main.stream.live"]),
    ("search", &["set.frame.find"]),
    // Every area page has the same footer: Undo / Redo / "Undo changes on
    // this page", and an "Apply (restarts cameras)" bar only on sections
    // that need an engine restart.
    ("page", &["set.frame.revert", "set.frame.apply", "set.frame.ok", "set.frame.cancel", "set.frame.titlebar",
        "set.modal.discard", "set.modal.remove"]),
    ("dialog", &["dlg.confirm.endstream", "dlg.confirm.stoprec", "dlg.confirm.switch", "dlg.confirm.quit"]),
];

/// The phone remote (src/control/remote.html) gets its own tabs.
pub const PHONE: &[(&str, &[&str])] = &[
    ("always", &["rem.status", "rem.header.record", "rem.header.live", "rem.header.marker", "rem.header.clip"]),
    ("scenes", &["rem.monitor", "rem.cut_take", "rem.scenes", "rem.switching", "rem.header.ftb"]),
    ("sound", &["rem.audio", "rem.sound"]),
    ("titles", &["rem.graphics", "rem.timers"]),
    ("more", &["rem.rundown", "rem.macros", "rem.ptz", "rem.prompter", "rem.comments", "rem.media", "rem.header.replay", "rem.tally_links"]),
    // full-screen pages, not tabs
    ("pages", &["rem.pin", "rem.tally", "prompt.page"]),
];

/// Ways in that are not a screen. They don't change; they're listed so the
/// tests see them, and each is tied to a home you can see (VISIBLE, below).
pub const INPUTS: &[(&str, &[&str])] = &[
    // (key.back and key.custom also have visible homes above: ⋯ › Last
    // scene, and the Shortcuts page.)
    ("keys", &["key.settings", "key.undo", "key.redo", "key.record", "key.transition", "key.edit", "key.marker",
        "key.marker_named", "key.auto", "key.take", "key.cut", "key.captions", "key.segment_next",
        "key.segment_prev", "key.clip", "key.talkback", "key.back", "key.number", "key.graphic", "key.pad",
        "key.custom", "key.display", "key.prompter_page"]),
    ("commands", &["cmd.scene", "cmd.source", "cmd.take", "cmd.cut", "cmd.transition", "cmd.ftb", "cmd.record",
        "cmd.stop", "cmd.golive", "cmd.endstream", "cmd.marker", "cmd.graphic", "cmd.title", "cmd.captions",
        "cmd.auto", "cmd.gain", "cmd.mute", "cmd.talkback", "cmd.pad", "cmd.syrinscape", "cmd.timer",
        "cmd.rundown", "cmd.media", "cmd.clip:N", "cmd.replay:N", "cmd.macro", "cmd.wait", "cmd.ptz",
        "cmd.prompter", "cmd.comment", "cmd.guest"]),
    ("streamdeck", &["sd.*"]),
    // (midi.learn is a button, above.)
    ("osc_midi", &["osc.in", "osc.feedback", "midi.in", "midi.lights"]),
    ("http", &["api.state", "api.ws", "api.action", "api.prompter", "api.program_jpg", "api.tally", "api.key_png", "api.icon"]),
    ("cli", &["cli.version", "cli.list_devices", "cli.vcam_install", "cli.vcam_uninstall", "cli.config", "cli.demo",
        "cli.init", "cli.probe_encoders", "cli.stream_to", "cli.measure_sync", "cli.headless",
        "cli.restream_test", "cli.ui_test"]),
];

/// For every key, command, command-line flag, right-click or hover route:
/// the button or page a person can SEE that does the same thing (an
/// "area/tab[/section]", a SHELL part, or "phone/<tab>").
pub const VISIBLE: &[(&str, &str)] = &[
    // keys
    ("key.settings", "rail"), ("key.undo", "show/edit/canvas"), ("key.redo", "show/edit/canvas"),
    ("key.record", "status"), ("key.transition", "show/switch/bank"), ("key.edit", "show/edit"),
    ("key.marker", "status"), ("key.marker_named", "status"), ("key.auto", "show/switch/bank"),
    ("key.take", "show/switch/program"), ("key.cut", "show/switch/program"), ("key.captions", "show/switch/bank"),
    ("key.segment_next", "show/switch/column"), ("key.segment_prev", "show/switch/column"), ("key.clip", "status"),
    ("key.talkback", "show/switch/column"), ("key.back", "show/switch/bank"), ("key.number", "show/switch/bank"),
    ("key.graphic", "show/switch/column"), ("key.pad", "show/switch/column"), ("key.custom", "more/control/shortcuts"),
    ("key.display", "record/screens"), ("key.prompter_page", "phone/pages"),
    // commands
    ("cmd.scene", "show/switch/bank"), ("cmd.source", "show/switch/multiview"), ("cmd.take", "show/switch/program"),
    ("cmd.cut", "show/switch/program"), ("cmd.transition", "show/switch/bank"), ("cmd.ftb", "show/switch/bank"),
    ("cmd.record", "status"), ("cmd.stop", "status"), ("cmd.golive", "status"), ("cmd.endstream", "status"),
    ("cmd.marker", "status"), ("cmd.graphic", "show/switch/column"), ("cmd.title", "show/switch/column"),
    ("cmd.captions", "show/switch/bank"), ("cmd.auto", "show/switch/bank"), ("cmd.gain", "show/switch/column"),
    ("cmd.mute", "show/switch/column"), ("cmd.talkback", "show/switch/column"), ("cmd.pad", "show/switch/column"),
    ("cmd.syrinscape", "sound/syrinscape"), ("cmd.timer", "graphics/timers"), ("cmd.rundown", "show/switch/column"),
    ("cmd.media", "sources/media/media"), ("cmd.clip:N", "status"), ("cmd.replay:N", "show/switch/bank"),
    ("cmd.macro", "show/switch/column"), ("cmd.wait", "more/tools/macros"), ("cmd.ptz", "show/switch/column"),
    ("cmd.prompter", "show/switch/column"), ("cmd.comment", "show/switch/column"), ("cmd.guest", "show/switch/column"),
    // right-click and hover
    ("ctx.replay", "show/switch/bank"), ("ctx.clock", "show/switch/column"), ("ctx.ptz.preset", "show/switch/column"),
    // (the tile's ⋯ offers what its right-click does)
    ("ctx.mv.split", "show/switch/multiview"), ("ctx.mv.framing", "show/switch/multiview"), ("ctx.mv.playlist", "show/switch/multiview"),
    ("mv.guest_warn", "sources/guests/guests"),
    // reachable only one way today
    ("dlg.marker_name", "status"), ("main.header.clock", "more/app/general"),
    // Stream Deck, OSC, MIDI, HTTP
    ("sd.*", "more/control/streamdeck"), ("osc.in", "more/control/oscmidi"), ("osc.feedback", "more/control/oscmidi"),
    ("midi.in", "more/control/oscmidi"), ("midi.lights", "more/control/oscmidi"),
    ("api.state", "more/control/remote"), ("api.ws", "more/control/remote"), ("api.action", "more/control/remote"),
    ("api.prompter", "more/tools/prompter"), ("api.program_jpg", "more/control/remote"), ("api.tally", "more/control/remote"),
    ("api.key_png", "more/control/streamdeck"),
    // the command line
    ("cli.version", "more/app/about"), ("cli.list_devices", "more/app/diagnostics"), ("cli.vcam_install", "record/screens"),
    ("cli.vcam_uninstall", "record/screens"), ("cli.config", "more/app/diagnostics"), ("cli.probe_encoders", "more/app/diagnostics"),
    ("cli.measure_sync", "sound/inputs"), ("cli.init", "more/app/help"), ("cli.stream_to", "record/streaming"),
];

/// Routes opened by another route (the dialog a key opens): they're reached
/// every way that one is.
pub const OPENED_BY: &[(&str, &[&str])] = &[("dlg.marker_name", &["key.marker_named"])];

/// Ways in with no screen twin, on purpose: developer and scripting tools,
/// and the phone's home-screen icon.
pub const NO_SCREEN: &[&str] = &["cli.demo", "cli.headless", "cli.restream_test", "cli.ui_test", "api.icon"];

/// Old routes that became part of another (id, where). Every merged id
/// must ALSO appear in a home above.
pub const MERGED: &[(&str, &str)] = &[
    // the status light
    ("main.header.health", "status"), ("win.health", "status"), ("main.header.log", "status"),
    ("panel.log", "status"), ("main.header.check", "status"), ("set.frame.problem", "status"),
    // the Edit layout tab, the rail
    ("bank.edit", "show/edit"), ("main.header.settings", "rail"),
    // live edits: changes apply as made, Undo changes on this page
    ("set.frame.ok", "page"), ("set.frame.apply", "page"), ("set.frame.cancel", "page"),
    // retired: pages aren't a window, so there's nothing to centre
    ("set.frame.titlebar", "page"),
];

/// Every route id given a home: the areas' sections, the shell, the phone
/// and the ways in that aren't a screen.
pub fn all_home_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = AREAS.iter().flat_map(|a| a.tabs).flat_map(|t| t.sections).flat_map(|s| s.routes.iter().copied()).collect();
    for group in [SHELL, PHONE, INPUTS] {
        ids.extend(group.iter().flat_map(|(_, r)| r.iter().copied()));
    }
    ids
}

pub fn area(id: &str) -> Option<&'static Area> {
    AREAS.iter().find(|a| a.id == id)
}

/// Whether a place named as "area", "area/tab", "area/tab/section", a part
/// of the shell or "phone/<tab>" is there.
pub fn place_exists(path: &str) -> bool {
    let mut parts = path.split('/');
    let first = parts.next().unwrap_or_default();
    let rest: Vec<&str> = parts.collect();
    if first == "phone" {
        return rest.len() == 1 && PHONE.iter().any(|(t, _)| *t == rest[0]);
    }
    if rest.is_empty() && SHELL.iter().any(|(p, _)| *p == first) {
        return true;
    }
    let Some(a) = area(first) else { return false };
    match rest.as_slice() {
        [] => true,
        [tab] => a.tabs.iter().any(|t| t.id == *tab),
        [tab, sect] => a.tabs.iter().find(|t| t.id == *tab).is_some_and(|t| t.sections.iter().any(|s| s.id == *sect)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::routes::{Input, ROUTES};
    use std::collections::HashSet;

    fn route_ids() -> HashSet<&'static str> {
        ROUTES.iter().map(|r| r.id).collect()
    }

    /// The ways in a route has in the new tree, from where it's homed.
    fn inputs_now(id: &str) -> HashSet<Input> {
        use Input::*;
        let mut now = HashSet::new();
        let on_screen = AREAS.iter().flat_map(|a| a.tabs).flat_map(|t| t.sections).any(|s| s.routes.contains(&id)) || SHELL.iter().any(|(_, r)| r.contains(&id));
        if on_screen {
            // Drawn where it can be seen and clicked: that also covers what
            // used to be right-click or hover only.
            now.extend([Mouse, RightClick, Hover, Display]);
        }
        if PHONE.iter().any(|(_, r)| r.contains(&id)) {
            now.insert(Phone);
        }
        for (_, by) in OPENED_BY.iter().filter(|(r, _)| *r == id) {
            for b in by.iter() {
                now.extend(inputs_now(b));
            }
        }
        for (group, ids) in INPUTS {
            if !ids.contains(&id) {
                continue;
            }
            match *group {
                "keys" => now.extend([Key]),
                "commands" => now.extend([Key, StreamDeck, Midi, Osc, Http]),
                "streamdeck" => now.extend([StreamDeck]),
                "osc_midi" => now.extend([Osc, Midi]),
                "http" => now.extend([Http]),
                "cli" => now.extend([Cli]),
                other => panic!("INPUTS group {other} has no ways in"),
            }
        }
        now
    }

    #[test]
    fn every_route_has_a_home() {
        let homes: HashSet<_> = all_home_ids().into_iter().collect();
        let missing: Vec<_> = ROUTES.iter().filter(|r| !homes.contains(r.id)).map(|r| r.id).collect();
        assert!(missing.is_empty(), "routes with no home: {missing:?}");
    }

    #[test]
    fn every_home_points_at_a_real_route() {
        let ids = route_ids();
        let bad: Vec<_> = all_home_ids().into_iter().filter(|h| !ids.contains(h)).collect();
        assert!(bad.is_empty(), "homes naming no route: {bad:?}");
        let bad: Vec<_> = VISIBLE.iter().filter(|(id, _)| !ids.contains(id)).map(|(id, _)| *id).collect();
        assert!(bad.is_empty(), "VISIBLE rows naming no route: {bad:?}");
        let bad: Vec<_> = VISIBLE.iter().filter(|(_, at)| !place_exists(at)).map(|(id, at)| format!("{id} → {at}")).collect();
        assert!(bad.is_empty(), "VISIBLE rows naming no place: {bad:?}");
        let bad: Vec<_> = MERGED.iter().filter(|(id, at)| !ids.contains(id) || !place_exists(at)).map(|(id, at)| format!("{id} → {at}")).collect();
        assert!(bad.is_empty(), "MERGED rows naming no route or place: {bad:?}");
        let bad: Vec<_> = OPENED_BY.iter().flat_map(|(id, by)| std::iter::once(id).chain(by.iter())).filter(|id| !ids.contains(*id)).collect();
        assert!(bad.is_empty(), "OPENED_BY naming no route: {bad:?}");
        let bad: Vec<_> = NO_SCREEN.iter().filter(|id| !ids.contains(*id)).collect();
        assert!(bad.is_empty(), "NO_SCREEN naming no route: {bad:?}");
    }

    #[test]
    fn route_count_is_known() {
        assert_eq!(ROUTES.len(), 251, "a route was added or removed: update shots/4-route-map.md and this number on purpose");
        assert_eq!(route_ids().len(), ROUTES.len(), "duplicate route id");
        assert!(ROUTES.iter().all(|r| !r.was.is_empty() && !r.label.trim().is_empty()), "every route says how it was reached and what it's called");
    }

    /// Each route sits once in a section (a second home is a different
    /// part of the window, such as the phone or a key).
    #[test]
    fn no_route_twice_in_one_part() {
        let mut twice: Vec<&str> = Vec::new();
        let sects = AREAS.iter().flat_map(|a| a.tabs).flat_map(|t| t.sections).map(|s| s.routes);
        for list in sects.chain(SHELL.iter().map(|(_, r)| *r)).chain(PHONE.iter().map(|(_, r)| *r)).chain(INPUTS.iter().map(|(_, r)| *r)) {
            let mut seen = HashSet::new();
            twice.extend(list.iter().filter(|id| !seen.insert(**id)));
        }
        assert!(twice.is_empty(), "listed twice in one place: {twice:?}");
    }

    #[test]
    fn nothing_is_reachable_only_by_hover_right_click_key_or_command() {
        use Input::*;
        let visible: HashSet<_> = VISIBLE.iter().map(|(id, _)| *id).collect();
        let unseen: Vec<_> = ROUTES
            .iter()
            .filter(|r| !r.was.iter().any(|i| matches!(i, Mouse | Phone | Display)))
            .filter(|r| !visible.contains(r.id) && !NO_SCREEN.contains(&r.id))
            .map(|r| r.id)
            .collect();
        assert!(unseen.is_empty(), "only by a key, a command, the command line, right-click or hover, with no visible home: {unseen:?}");
        // And every way in that isn't a screen names its visible twin.
        let unnamed: Vec<_> = INPUTS.iter().flat_map(|(_, r)| r.iter()).filter(|id| !visible.contains(*id) && !NO_SCREEN.contains(*id)).collect();
        assert!(unnamed.is_empty(), "no visible twin named: {unnamed:?}");
    }

    #[test]
    fn no_way_in_was_lost() {
        let mut lost = Vec::new();
        for r in ROUTES {
            let now = inputs_now(r.id);
            for i in r.was {
                if !now.contains(i) {
                    lost.push(format!("{} by {i:?}", r.id));
                }
            }
        }
        assert!(lost.is_empty(), "ways in lost: {lost:?}");
    }

    #[test]
    fn merged_pieces_are_still_reachable() {
        let homes: HashSet<_> = all_home_ids().into_iter().collect();
        for (id, _) in MERGED {
            assert!(homes.contains(id), "{id} merged but has no home");
        }
    }

    #[test]
    fn tabs_are_tidy() {
        let mut areas = HashSet::new();
        let mut keys = HashSet::new();
        for a in AREAS {
            assert!(areas.insert(a.id), "area {} twice", a.id);
            if let Some(k) = a.key {
                assert!(keys.insert(k), "key {k} on two areas");
            }
            let mut tabs = HashSet::new();
            for t in a.tabs {
                assert!(tabs.insert(t.id), "tab {}/{} twice", a.id, t.id);
                assert!(!t.sections.is_empty(), "tab {}/{} has no sections", a.id, t.id);
                let mut sects = HashSet::new();
                for s in t.sections {
                    assert!(sects.insert(s.id), "section {}/{}/{} twice", a.id, t.id, s.id);
                }
            }
            let plain = a.tabs.iter().filter(|t| !t.more).count();
            assert!(plain >= 1, "area {} has no tab of its own", a.id);
            // The 1024-wide top bar fits four.
            assert!(plain <= 4, "area {} has {plain} tabs: more than fit", a.id);
            // More▾ comes after the plain tabs.
            let first_more = a.tabs.iter().position(|t| t.more).unwrap_or(a.tabs.len());
            assert!(a.tabs[first_more..].iter().all(|t| t.more), "area {}: a plain tab after More▾ ones", a.id);
        }
        // The rail's order (approved): the order a show is set up in.
        let order: Vec<_> = AREAS.iter().map(|a| a.id).collect();
        assert_eq!(order, ["show", "sources", "scenes", "sound", "graphics", "record", "more"]);
    }

    /// Every old Settings page's settings land somewhere for ⌘K search.
    #[test]
    fn every_old_settings_page_has_a_section() {
        let helped: HashSet<Page> = AREAS.iter().flat_map(|a| a.tabs).flat_map(|t| t.sections).flat_map(|s| s.help.iter().copied()).collect();
        let lost: Vec<_> = Page::ALL.iter().filter(|p| !helped.contains(p)).collect();
        assert!(lost.is_empty(), "old Settings pages with no new section: {lost:?}");
    }

    /// Where code names a route or a place (`nav::here("…")` on a button,
    /// `nav::go("…")` on a link, `data-route="…"` on the phone page), it's
    /// one that exists. Read from the source files, so it can't fall out of
    /// step.
    #[test]
    fn code_uses_only_known_ids() {
        let ids = route_ids();
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        let mut dirs = vec![src];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x == "rs" || x == "html") && !p.ends_with("ui/nav.rs") {
                    files.push(p);
                }
            }
        }
        assert!(files.len() > 20, "the source files weren't found");
        // (Split so this file's own text doesn't match.)
        let here = concat!("nav::", "here(\"");
        let go = concat!("nav::", "go(\"");
        let data = concat!("data-", "route=\"");
        let mut bad = Vec::new();
        for f in &files {
            let text = std::fs::read_to_string(f).unwrap_or_default();
            for (pat, ok) in [(here, true), (go, false), (data, true)] {
                for piece in text.split(pat).skip(1) {
                    let name = piece.split('"').next().unwrap_or_default();
                    let known = if ok { ids.contains(name) } else { place_exists(name) };
                    if !known {
                        bad.push(format!("{}: {name}", f.display()));
                    }
                }
            }
        }
        assert!(bad.is_empty(), "unknown route ids or places in the code: {bad:?}");
    }

    /// The line each redesign PR quotes.
    #[test]
    fn rehomed_count() {
        let done = ROUTES.iter().filter(|r| !r.legacy).count();
        println!("Rehomed: {done} of {} routes", ROUTES.len());
        // Moved routes have every way in they had (no_way_in_was_lost),
        // and a home in the new tree (every_route_has_a_home).
        assert!(done <= ROUTES.len());
    }
}
