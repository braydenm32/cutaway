# 5 — Information architecture (Phase 3, plan only)

For Brayden. This is the map of where everything in Cutaway goes in the calm redesign
(direction A: Cutaway's own look, trimmed). Nothing has been built or changed. Line numbers
refer to the 0.9.7 tree as read on 2026-10-31 (`/home/claude/cutaway`, commit 7d051ef).

Evidence used: `1-usage-audit.md` (what people use, ranked), `2-page-forensics.md` (why each
screen is hard), `3-numbers-audit.md` (numbers that disagree), `4-route-map.md` (all 251 places
and actions, each with an id) and the README.

**The idea in one paragraph.** The main window gets a slim rail of areas down the left:
**Show, Sources, Scenes, Sound, Graphics, Record**, with **More** at the bottom. Clicking an area
turns the top bar into that area's tabs. **Show** is the live desk you know today (program,
multiview, scene bank, and the column of panels on the right). Each of the other areas is a full
page that replaces part of today's Settings window, so the Settings window goes away. On the right
of the top bar there's one status cluster that never moves: a single **All good / 2 to check**
light (Health and Log together), one **time left** number, the clock, the **REC** pill (hold to
stop) and, once you have somewhere to stream to, the **LIVE** pill.

```
┌──────┬─────────────────────────────────────────────────────────────────────────────┐
│ ▣    │  Switch   Edit layout           ● All good   2h 40m left   21:04   ● REC 00:12:41 ◆ ✂ │
│Brayd.│─────────────────────────────────────────────────────────────────────────────│
│▶ Show│  ┌───────────── Program ─────────────┐   ┌ right column ───────────┐        │
│◉ Src │  │                                   │   │ Graphics  [Maya][Sam]   │        │
│▦ Scn │  └───────────────────────────────────┘   │ ...folded panels...     │        │
│♪ Snd │  [multiview tiles .......................] │ AUDIO strip             │        │
│T Gfx │  [1 Wide][2 Maya][3 Sam][4 Two][5 Screen][6 Couch][7 PiP]  CUT MIX ⋯ ms Preview │
│● Rec │                                                                             │
│⋯ More│                                                                             │
└──────┴─────────────────────────────────────────────────────────────────────────────┘
```

How to read the rest:

* **Area › Tab › Section** is the new address of a thing. "More▾" means the tab is in the
  dropdown at the end of the area's tab row, not a tab of its own.
* "Ctl" means a thing is also reached through Cutaway's **commands**: Stream Deck, OSC
  `/cutaway/<verb>`, MIDI mappings, macros, your own keys and `/api/action?do=`. None of these
  change. A command keeps its name and does what it did.
* **NEW home** means it gets a visible place it never had. **CHANGED** means it behaves
  differently, and says how. **merged** means two old things became one, and says where the
  pieces went.

---

## 1. The area/tab tree, as one config file

All of the navigation lives in one file, `src/ui/nav.rs`, as plain Rust constants. I chose a
Rust file over TOML because the compiler then checks every name, the orphan test (part 3) reads
it directly, and nobody edits it as a user setting. It is the only place a tab, its label, its
icon, its order or its More▾ flag is written down. The left rail, the top-bar tabs, the ⌘K search,
the "Set up ›" links and the status light's "fix it" links are all drawn from it.

Order inside every area follows the usage audit's ranking (1-usage-audit.md, "Screens ranked by
use"). Anything ranked 19 or 20 there, or used 0 times by every persona, sits under More▾.

```rust
// src/ui/nav.rs: the one place the navigation is written down.
pub struct Area { pub id: &'static str, pub label: &'static str, pub icon: Icon, pub key: Option<u8>, pub tabs: &'static [Tab] }
pub struct Tab { pub id: &'static str, pub label: &'static str, pub more: bool, pub sections: &'static [Sect] }
/// A section of a tab. `routes`: route ids that live here. `help`: the old Settings page whose
/// "Find a setting" entries (help.rs) now point here (see part 3).
pub struct Sect { pub id: &'static str, pub label: &'static str, pub routes: &'static [&'static str], pub help: &'static [Page] }

pub const AREAS: &[Area] = &[
  Area { id: "show", label: "Show", icon: Icon::Monitor, key: Some(1), tabs: &[
    Tab { id: "switch", label: "Switch", more: false, sections: &[
      Sect { id: "program", label: "Program", help: &[], routes: &[
        "mon.program", "mon.preview", "mon.preview.take", "mon.preview.cut", "mon.tall",
        "mon.starting", "mon.engine_error", "mon.waiting.rescue", "main.rec.takecard"] },
      Sect { id: "multiview", label: "Multiview", help: &[], routes: &[
        "mv.tile", "mv.media", "mv.framing", "ctx.mv.framing", "ctx.mv.playlist", "ctx.mv.split", "mv.guest_warn"] },
      Sect { id: "bank", label: "Scene bank", help: &[], routes: &[
        "bank.scene", "bank.overflow", "bank.transition", "bank.ms", "bank.preview", "bank.cc",
        // the ⋯ beside CUT / MIX (usage audit rank 19):
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
    Tab { id: "closeups", label: "Close-ups & PTZ", more: false, sections: &[   // PTZ: rank 11, 5–10 visits before a show
      Sect { id: "split", label: "Close-ups from one camera", help: &[Page::Sources], routes: &[] },
      Sect { id: "ptz", label: "PTZ presets", help: &[Page::Sources], routes: &[] },
      Sect { id: "framing", label: "Auto-framing", help: &[Page::Sources], routes: &[] } ]},
    Tab { id: "people", label: "People & names", more: false, sections: &[
      Sect { id: "who", label: "Who's in each picture", help: &[Page::Sources], routes: &[] } ]},
    Tab { id: "guests", label: "Guests", more: true, sections: &[         // 0 visits for every persona
      Sect { id: "guests", label: "Remote guests", help: &[Page::Guests], routes: &["set.inputs.guests"] } ]},
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
    Tab { id: "captions", label: "Captions", more: true, sections: &[      // rank 20, 0 visits
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
    Tab { id: "streaming", label: "Streaming", more: false, sections: &[ // always a tab; empty state until a destination exists
      Sect { id: "where", label: "Where to stream", help: &[Page::Streaming], routes: &["set.output.streaming", "main.stream.setup"] } ]},
    Tab { id: "screens", label: "Screens & webcam", more: true, sections: &[ // rank 19
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
  ("rail",   &["main.header.settings", "main.header.logo", "main.header.show", "main.header.update"]),
  ("status", &["main.header.health", "main.header.log", "panel.log", "win.health", "main.header.check",
               "set.frame.problem", "main.header.clock", "main.header.typing",
               "main.rec.record", "main.rec.stop", "main.rec.marker", "menu.markers", "dlg.marker_name", "main.rec.clip",
               "main.stream.golive", "main.stream.live"]),
  ("search", &["set.frame.find"]),
  // every area page has the same footer: Undo / Redo / "Undo changes on this page", and an
  // "Apply (restarts cameras)" bar only on sections that need an engine restart.
  ("page",   &["set.frame.revert", "set.frame.apply", "set.frame.ok", "set.frame.cancel", "set.frame.titlebar",
               "set.modal.discard", "set.modal.remove"]),
  ("dialog", &["dlg.confirm.endstream", "dlg.confirm.stoprec", "dlg.confirm.switch", "dlg.confirm.quit"]),
];

/// The phone remote (src/control/remote.html) gets its own tabs.
pub const PHONE: &[(&str, &[&str])] = &[
  ("always", &["rem.status", "rem.header.record", "rem.header.live", "rem.header.marker", "rem.header.clip"]),
  ("scenes", &["rem.monitor", "rem.cut_take", "rem.scenes", "rem.switching", "rem.header.ftb"]),
  ("sound",  &["rem.audio", "rem.sound"]),
  ("titles", &["rem.graphics", "rem.timers"]),
  ("more",   &["rem.rundown", "rem.macros", "rem.ptz", "rem.prompter", "rem.comments", "rem.media", "rem.header.replay", "rem.tally_links"]),
  ("pages",  &["rem.pin", "rem.tally", "prompt.page"]),   // full-screen pages, not tabs
];

/// Ways in that are not a screen. They do not change; they're listed so the orphan test
/// sees them and so each one is tied to a visible home (VISIBLE, below).
pub const INPUTS: &[(&str, &[&str])] = &[
  ("keys", &["key.settings", "key.undo", "key.redo", "key.record", "key.transition", "key.edit", "key.marker",
             "key.marker_named", "key.auto", "key.take", "key.cut", "key.captions", "key.segment_next",
             "key.segment_prev", "key.clip", "key.talkback", "key.number", "key.graphic", "key.pad",
             "key.display", "key.prompter_page"]),     // key.back and key.custom have homes above
  ("commands", &["cmd.scene", "cmd.source", "cmd.take", "cmd.cut", "cmd.transition", "cmd.ftb", "cmd.record",
             "cmd.stop", "cmd.golive", "cmd.endstream", "cmd.marker", "cmd.graphic", "cmd.title", "cmd.captions",
             "cmd.auto", "cmd.gain", "cmd.mute", "cmd.talkback", "cmd.pad", "cmd.syrinscape", "cmd.timer",
             "cmd.rundown", "cmd.media", "cmd.clip:N", "cmd.replay:N", "cmd.macro", "cmd.wait", "cmd.ptz",
             "cmd.prompter", "cmd.comment", "cmd.guest"]),
  ("streamdeck", &["sd.*"]),
  ("osc_midi", &["osc.in", "osc.feedback", "midi.in", "midi.lights"]),   // midi.learn is a button, above
  ("http", &["api.state", "api.ws", "api.action", "api.prompter", "api.program_jpg", "api.tally", "api.key_png", "api.icon"]),
  ("cli", &["cli.version", "cli.list_devices", "cli.vcam_install", "cli.vcam_uninstall", "cli.config", "cli.demo",
            "cli.init", "cli.probe_encoders", "cli.stream_to", "cli.measure_sync", "cli.headless",
            "cli.restream_test", "cli.ui_test"]),
];

/// For every key, command, CLI flag or hover/right-click route: the button or page a person can
/// SEE that does the same thing. Checked by the test. (Excerpt; the full list is the "Visible
/// home" column of the table in part 2.)
pub const VISIBLE: &[(&str, &str)] = &[
  ("key.back", "show/switch/bank"),          ("key.marker_named", "status"),   ("ctx.replay", "show/switch/bank"),
  ("ctx.clock", "graphics/timers"),          ("ctx.ptz.preset", "sources/closeups"), ("ctx.mv.split", "sources/closeups"),
  ("ctx.mv.framing", "sources/closeups"),    ("ctx.mv.playlist", "sources/media"),   ("mv.guest_warn", "status"),
  ("cmd.clip:N", "status"),                  ("cmd.replay:N", "show/switch/bank"),   ("cmd.gain", "show/switch/column"),
  ("cmd.syrinscape", "sound/syrinscape"),    ("cmd.media", "sources/media"),         ("cmd.wait", "more/tools"),
  ("cli.vcam_install", "record/screens"),    ("cli.vcam_uninstall", "record/screens"),
  ("cli.list_devices", "more/app"),          ("cli.probe_encoders", "more/app"),     ("cli.measure_sync", "sound/inputs"),
  ("cli.version", "more/app"),               ("main.header.clock", "more/app"),      ("api.tally", "more/control"),
  // ... one row for every route in INPUTS, every ctx.*, and every route in "reachable only one way"
];

/// Old routes that became part of another. Every merged id must ALSO appear in a home above.
pub const MERGED: &[(&str, &str)] = &[
  ("main.header.health", "status light"), ("win.health", "status light"), ("main.header.log", "status light"),
  ("panel.log", "status light"),          ("main.header.check", "status light"), ("set.frame.problem", "status light"),
  ("bank.edit", "show/edit tab"),         ("main.header.settings", "rail"),
  ("set.frame.ok", "live edits"),         ("set.frame.apply", "live edits"), ("set.frame.cancel", "live edits"),
  ("set.frame.titlebar", "no window to centre"),
];
```

Why the tabs differ from the first proposal (each change follows the usage audit):

| Proposed | Now | Evidence |
|---|---|---|
| Show: Switch \| Rundown \| Edit layout | Show: **Switch \| Edit layout**. Rundown moves to More › Show tools | Edit layout is rank 16; Rundown is rank 20 with 0 visits for all four personas. The rundown *panel* still shows on the Switch desk as soon as a rundown has segments, and N / Shift+N still work. (Question 3.) |
| Sources: Cameras \| People & names \| Guests \| More▾ | Sources: **Cameras \| Close-ups & PTZ \| People & names \| More▾ (Guests, Network & phone feeds, Media & playlists)** | PTZ close-ups are rank 11 with 5–10 visits before every show (Maya and Sam are cut from the Couch camera). Guests had 0 visits for every persona. Today Split, PTZ and auto-framing are buried on the Sources page or on right-click only (route map #6, forensics multiview #2). |
| Scenes: Scenes \| Switching \| More▾ | Same. More▾ = Program picture, Auto-switching, Stinger & fade to black | Transitions beyond Cut/Mix, FTB and AUTO are rank 19. The Program page (size, frame rate, background) is set once. |
| Sound: Mixer \| Inputs \| Pads \| More▾ | Same. More▾ = Headphones & talkback, Syrinscape | Audio strip rank 4; Pads rank 9 (20 visits for the streamer); Syrinscape rank 20. |
| Graphics: Titles & overlays \| Captions \| More▾ | Graphics: **Titles & overlays \| More▾ (Clocks & timers, Captions, Taking turns, Tickers & web overlays)** | Graphics is rank 7. Captions is rank 20 with 0 visits, so it goes under More▾. The CC button and C key stay on the desk once captions are set up. One tab plus More▾ is fine: the area opens straight onto the list you need. |
| Record: Takes \| Recording \| Streaming \| Screens & webcam \| More▾ | Record: **Takes \| Recording \| Streaming \| More▾ (Screens & webcam, Upload, Vertical version, Scheduled)** | Screen and Webcam are rank 19. Streaming stays a tab and shows an empty state ("Add where to stream") until a destination exists, so a record-only show isn't nagged in three places (forensics header #6). |
| More: Control, Show tools, App | Same three tabs; Rundown joins Show tools | Usage audit "Under a themed More". |

The rail keeps the approved order (Show, Sources, Scenes, Sound, Graphics, Record). It follows
the order you set a show up in (cameras, then scenes, then sound, then names, then record), which
is also the order of the first-show guide. During a show you stay in **Show** and don't need the
rail at all.

---

## 2. Every old route and its new home

Columns: **Old id** · **New home** · **How you reach it** (click path · key · phone · Ctl) ·
**Change**. A dash means there was no such way in before either. "Desk" is Show › Switch. "Column"
is the right-hand column of panels on the desk. "⋯" is the new menu beside CUT / MIX on the bank.
"Light" is the status light in the top bar.

**Rule:** every way in that a route has today is still there. Where this table says CHANGED, the
change is to what a click *does* or where the button is drawn, never to whether a key, the phone,
the Stream Deck, MIDI or OSC can reach it.

### 2a. Main window header → rail and status cluster (11)

| Old id | New home | How you reach it | Change |
|---|---|---|---|
| `main.header.help` | More › App › Help & first-show guide | Click rail More › App › Help. ⌘K "shortcuts" | moved. The cheat sheet becomes a page, drawn from shortcuts.rs so it shows your real keys (numbers audit #22) |
| `main.header.settings` | Rail (every area) | Click any rail area. ⌘, | **CHANGED:** no Settings window. ⌘, opens the area you last used other than Show (Sources the first time) |
| `main.header.update` | Rail: a dot on More; More › App › Updates & about | Click More › App › Updates & about › Update now | moved. A small dot instead of a header button |
| `win.update` | More › App › Updates & about | As above | moved from a window to a section |
| `main.header.typing` | Status cluster | Display only: "Typing, keys paused" chip left of the light | same |
| `main.header.log` | Light › Log | Click the light › Log | **merged** into the light (forensics header #4) |
| `main.header.health` | Light › Now | Click the light | **merged** into the light |
| `main.header.check` | Light › Run pre-show check; Record › Recording › Before you start | Click the light › Run check | **merged** (the button moves into the light's panel) |
| `main.header.screen` | Record › More▾ › Screens & webcam; also Desk › ⋯ › Second screen | Click ⋯ › Second screen | moved (rank 19) |
| `main.header.webcam` | Record › More▾ › Screens & webcam; also ⋯ › Webcam | Click ⋯ › Webcam [installed] | moved. **NEW:** an Install button there (was CLI only) |
| `main.header.clock` | Status cluster › clock; More › App › General › Clock | Click the clock (12/24 h), or the setting | **NEW visible home** for 12/24 h. The setting now applies to the second screen and scheduled line too (numbers audit #8) |

### 2b. Header left, recording and stream (19)

| Old id | New home | How you reach it | Change |
|---|---|---|---|
| `main.header.logo` | Rail top (logo) | Click → More › App › Updates & about | same click, opens a page instead of a window |
| `main.header.show` | Rail top (show name under the logo) | Click → Show menu | moved from header to rail |
| `menu.show.switch` | Show menu; More › App › Shows | Click show name › pick | same |
| `menu.show.save_as` | Show menu; More › App › Shows | Click show name › Save as a show… | same |
| `main.rec.record` | Status cluster › REC pill (idle: "● Record") | Click. ⌘R. Phone: header RECORD. Ctl `record` | moved inside the top bar |
| `main.rec.stop` | Status cluster › REC pill, hold | Hold. ⌘R asks first. Phone: hold REC. Ctl `stop` | **CHANGED:** "hold to stop" is written under the timer all the time; a click no longer swaps the timer for the hint (forensics header #3) |
| `main.rec.marker` | Status cluster › ◆ (while recording) | Click. M. Phone ＋Marker. Ctl `marker` | same |
| `menu.markers` | Status cluster › ◆ ▾ (recording); Record › Takes › markers (after) | Click ▾ | **CHANGED:** the ▾ is only ever markers, and they stay visible after the take (forensics after-show #2) |
| `main.rec.clip` | Status cluster › ✂, and a ✂ ▾ with lengths | Click. K. Phone ✂ Clip. Ctl `clip` | **NEW:** ✂ ▾ offers 10 / 30 / 60 s (was `clip:N` command only) |
| `main.rec.replay` | Desk › ⋯ › Replay; More › Show tools › Replay | Click ⋯ › Replay. Phone More › Replay. Ctl `replay` | moved (rank 19) |
| `ctx.replay` | Desk › ⋯ › Replay ▸ (Slow motion, Program, each camera, Quad, Stop, Last N s) | Click ⋯ › Replay ▸ | **NEW visible home** (was right-click only) |
| `main.rec.recent` | Record › Takes | Click Record › Takes | **CHANGED:** no longer the ▾ beside Marker (forensics header #5) |
| `main.rec.takecard` | Desk › under the program after a take; Record › Takes (top row) | Shown after Stop; Record › Takes | **CHANGED:** ✕ only hides the card; the take stays first in Takes. "Open in Resolve" no longer opens Settings the first time |
| `main.stream.golive` | Status cluster › LIVE pill ("◉ Go live") [destination exists] | Click. No key unless you bind one. Phone GO LIVE. Ctl `golive` | moved inside the top bar |
| `main.stream.setup` | Record › Streaming (empty state "Add where to stream") | Click Record › Streaming | **CHANGED:** the header link is gone; nothing about streaming shows on a record-only desk |
| `main.stream.live` | Status cluster › LIVE pill | Click → confirm. Phone: hold LIVE. Ctl `endstream` | same. Shows CONNECTING until really live (numbers audit #5) |
| `main.stream.panel` | Column › Stream (while live); Record › Streaming › Now | Visible while live | same, plus a copy on the Streaming tab |
| `main.stream.scheduled` | Record › More▾ › Scheduled; a line in the Stream panel | Click Record › More▾ › Scheduled | moved |
| `dlg.confirm.endstream` | Dialog | From LIVE pill / `endstream` | same |

### 2c. Monitors and Edit layout (32)

| Old id | New home | How you reach it | Change |
|---|---|---|---|
| `mon.engine_error` | Desk › program | Shown when the engine fails. Buttons: Go to the area at fault (was "Open settings"), Try again, Save a problem report | button now opens the right area page |
| `mon.starting` | Desk › program | Shown while starting | same. "TAKE n" counts takes (numbers audit #23) |
| `mon.waiting.rescue` | Desk › program; Record › Takes ("Recovered") | Shown after a rescue | **NEW:** the rescued take also appears in Takes |
| `mon.preview` | Desk | Preview on | same |
| `mon.preview.take` | Desk | Click TAKE. Space. Phone Scenes › TAKE. Ctl `take` | same |
| `mon.preview.cut` | Desk | Click CUT. Enter. Phone Scenes › CUT. Ctl `cut` | same |
| `mon.tall` | Desk | Click → Record › More▾ › Vertical version | click target moved |
| `mon.program` | Desk (also shown in Edit layout) | Always. Phone Scenes tab | same |
| `mon.edit.overlay` | Show › Edit layout | Drag, corner, Shift, scroll, double-click, Alt-drag | **NEW visible:** toolbar gets a "Move picture" tool (pan without Alt) and a hint row naming scroll-zoom and double-click (route map #5) |
| `mon.edit.toolbar` | Show › Edit layout | Click the tab. E | moved into a tab |
| `edit.done` | Show › Edit layout › Done | Click. Esc (new) | goes back to Switch |
| `edit.save_scenes` | Show › Edit layout | Click | same |
| `edit.redo` | Show › Edit layout; and the footer of every area page | Click. ⌘⇧Z / Ctrl+Y | **NEW visible home** outside Edit layout (route map #4) |
| `edit.undo` | Show › Edit layout; and every area page footer | Click. ⌘Z | **NEW visible home** outside Edit layout |
| `edit.frame.add` | Edit layout › Frame | Click | same |
| `edit.frame.remove` | Edit layout › Frame | Click | same |
| `edit.frame.front` | Edit layout › Frame | Click | same |
| `edit.frame.full` | Edit layout › Frame | Click | same |
| `edit.frame.fitfill` | Edit layout › Frame | Click; double-click frame | same |
| `edit.frame.zoom` | Edit layout › Frame | Slider; scroll | same |
| `edit.frame.reset` | Edit layout › Frame | Click | same |
| `edit.scene.name` | Edit layout › Scene; Scenes › Scenes | Type | same |
| `menu.edit.audio` | Edit layout › Scene › Audio (n) | Click | same |
| `menu.edit.cues` | Edit layout › Scene › Cues (n) | Click | same |
| `menu.edit.newscene` | Edit layout › Scene › ＋ New scene; Scenes › Scenes | Click | same |
| `edit.scene.duplicate` | Edit layout › Scene | Click | same |
| `edit.scene.delete` | Edit layout › Scene | Click → confirm | **CHANGED:** asks first, like Scenes › Remove (forensics edit #1) |
| `bank.edit` | Show › Edit layout (the tab) | Click the tab. E | **merged** into the tab; the bank button goes |
| `dlg.confirm.stoprec` | Dialog | ⌘R while recording; `stop` | same |
| `dlg.confirm.switch` | Dialog | From the Show menu | same |
| `dlg.confirm.quit` | Dialog | Closing mid-show | same |
| `dlg.title` | Dialog from ✎ | Click ✎ in Column › Graphics or Graphics › Titles. Ctl `title` | same |

### 2d. Multiview and scene bank (13)

| Old id | New home | How you reach it | Change |
|---|---|---|---|
| `mv.tile` | Desk › multiview | Click, Shift-click. Keys 1–9. Ctl `source` | **CHANGED (question 1):** with Preview off, a click *selects* the tile and shows a "Put on air" button; Shift-click still cuts. With Preview on, nothing changes (forensics multiview #1) |
| `mv.media` | Tile; Sources › More▾ › Media & playlists | Click ⏭ ⏯ ⟲. Phone More › Media. Ctl `media` | same, plus a page |
| `mv.framing` | Tile chip | Click | same |
| `ctx.mv.framing` | Tile ⋯ › Follow / Shot; Sources › Close-ups & PTZ › Auto-framing | Click the tile's ⋯ (always drawn, not on hover). Right-click still works | **NEW visible home** |
| `ctx.mv.playlist` | Tile ⋯ › Playlist; Sources › More▾ › Media & playlists | Click the tile's ⋯ | **NEW visible home** |
| `ctx.mv.split` | Tile ⋯ › Split into close-ups…; Sources › Close-ups & PTZ | Click the tile's ⋯ | **NEW visible home** |
| `mv.guest_warn` | ⚠ chip on the tile; Light item; Sources › More▾ › Guests | Visible chip; click it | **NEW visible home** (was hover only) |
| `bank.transition` | Desk › bank: CUT and MIX shown; WIPE, PUSH, DIP, STINGER in ⋯ | Click. T. Phone Scenes › Switching. Ctl `transition` | **CHANGED:** four styles move into ⋯ (rank 19) |
| `bank.ftb` | Desk › ⋯ › Fade to black; a FTB chip on the program while black | Click. Phone Scenes (hold). Ctl `ftb` | **CHANGED:** moves into ⋯ and is drawn as a live control (forensics bank #3) |
| `bank.ms` | Desk › bank, beside MIX | Drag | **CHANGED:** hidden for Cut and Stinger, where it does nothing (numbers audit #14) |
| `bank.preview` | Desk › bank | Click | same |
| `bank.auto` | Desk › ⋯ › Auto switching | Click. A. Phone Scenes › Auto. Ctl `auto` | **CHANGED:** greyed with "Set up rules ›" until rules exist (forensics bank #5) |
| `bank.cc` | Desk › bank [captions set up] | Click. C. Phone Scenes › CC. Ctl `captions` | same |

### 2e. Scene bank buttons and panels (30)

| Old id | New home | How you reach it | Change |
|---|---|---|---|
| `bank.scene` | Desk › bank | Click. 1–9, Shift+1–9. Phone Scenes. Ctl `scene` | same. Reorder in Edit layout or Scenes › Scenes |
| `bank.overflow` | Desk › bank | Scroll / click "+n ›" | **CHANGED:** buttons shrink or take a second row so 9 scenes fit at 1280 wide; "+n ›" only past that (forensics bank #1) |
| `panel.heading.fold` | Column headings; Column ⋯ › Arrange panels | Click heading. **NEW** Arrange panels list | **NEW visible home** with Reset (route map #7) |
| `panel.heading.reorder` | Column headings; Arrange panels (↑ ↓) | Drag heading, or ↑ ↓ | **NEW visible home** |
| `panel.heading.setup` | Column heading "Set up ›" | Click → that panel's area tab | **CHANGED:** one quiet link style instead of a button per panel (forensics column #5) |
| `panel.column.width` | Column edge; Arrange panels › Reset width | Drag; Reset | **NEW** reset |
| `panel.rundown` | Column (when the rundown has segments); More › Show tools › Rundown | ◀ Next ■. N, Shift+N. Phone More › Rundown. Ctl `rundown` | same on the desk |
| `menu.rundown.goto` | Column › Rundown › Go to ▾ | Click | same |
| `panel.guests` | Column (when guests exist); Sources › More▾ › Guests | Click ✕ / Copy link. Ctl `guest` | same |
| `win.guest_qr` | From Guests panel or page | Click QR | same |
| `panel.sound` | Column › Sound pads; Sound › Pads | Click a pad. F1–F12. Phone Sound. Ctl `pad`, `syrinscape` | same |
| `panel.sound.talk` | Column › Sound pads › TALK; Sound › More▾ › Headphones & talkback | Hold. Hold B. Phone Sound › TALK. Ctl `talkback` | same; hint reads your real key (numbers audit #22) |
| `panel.graphics` | Column › Graphics (always, even when empty); Graphics › Titles & overlays | Click. Alt/⌥+1–9. Phone Titles. Ctl `graphic` | **CHANGED:** never hidden; empty state "Make a name title ›" (forensics column #4) |
| `panel.graphics.edit_title` | Column › Graphics ✎; Graphics › Titles & overlays | Click ✎. Ctl `title` | same |
| `panel.graphics.reload` | Column › Graphics ⟳; Graphics › More▾ › Tickers & web overlays | Click | same |
| `panel.graphics.rotation` | Column › Graphics; Graphics › More▾ › Taking turns | Click | same |
| `ctx.clock` | Column › Graphics: ⏯ and ↺ drawn on each timer's button; Graphics › More▾ › Clocks & timers | Click. Phone Titles › Timers. Ctl `timer`. Right-click still works | **NEW visible home** (was right-click only) |
| `panel.macros` | Column; More › Show tools › Macros | Click. Phone More › Macros. Ctl `macro` | same |
| `panel.ptz` | Column; Sources › Close-ups & PTZ | Joystick, zoom, Home, ＋. Phone More › PTZ. Ctl `ptz` | **CHANGED:** ⌂ Home moves away from Zoom out and needs a hold while that shot is on air (forensics column #3) |
| `ctx.ptz.preset` | PTZ panel: **Save here** button beside the presets | Click a preset to recall; Save here to store. Right-click still works | **NEW visible home** |
| `panel.comments` | Column (when set up); More › Show tools › Viewer comments | Click. Phone More › Comments (now readable). Ctl `comment` | same on the desk |
| `panel.prompter` | Column; More › Show tools › Teleprompter | Buttons. Phone More › Teleprompter; /prompter page. Ctl `prompter` | same |
| `panel.uploads` | Column (while uploads exist); Record › More▾ › Upload | Buttons | same |
| `panel.audio` | Column › AUDIO (always, pinned last); Sound › Mixer | Always visible | **CHANGED:** drawn as a fixed heading so it no longer looks foldable (forensics column #2) |
| `audio.fader` | Column › AUDIO; Sound › Mixer | Drag; double-click resets. Phone Sound (**NEW** fader). Ctl `gain` | **CHANGED:** range −60…+12 dB so MIDI and commands aren't pulled up to −40 (numbers audit #2) |
| `audio.onoff` | Column › AUDIO | Click. Phone Sound. Ctl `mute` | same; phone needs a hold to turn a mic OFF |
| `audio.solo` | Column › AUDIO; Sound › More▾ › Headphones & talkback | Click 🎧 | same |
| `audio.phones` | Column › AUDIO; Sound › More▾ › Headphones & talkback | Drag | same |
| `panel.log` | Light › Log | Click the light › Log: Warnings only, Copy | **merged** |
| `dlg.marker_name` | Status cluster › ◆ ▾ › Named marker… | Click. Shift+M | **NEW visible home** (was Shift+M only) |

### 2f. Windows and dialogs (5)

| Old id | New home | How you reach it | Change |
|---|---|---|---|
| `win.about` | More › App › Updates & about | Click the logo, or More › App | moved from a window |
| `win.health` | Light › Now | Click the light | **merged**. Amber only for problems in the last minute, no false 0 fps at start (numbers audit #3, #17) |
| `dlg.precheck` | Light › Run pre-show check; Record › Recording › Before you start; runs by itself before REC / GO LIVE if turned on | Click | **CHANGED:** when started from the phone, Stream Deck, MIDI or OSC, the results go to the light and the phone, not a modal on the desk (route map #13). Each ⚠ row links to its fix |
| `dlg.sync` | Sound › Inputs › Lip sync › Measure…; from the pre-check | Click | moved |
| `win.guide` | More › App › Help & first-show guide | Shown on first run; **NEW:** reopen any time | **NEW** reopen |

### 2g. Second screen (1)

| Old id | New home | How you reach it | Change |
|---|---|---|---|
| `win.display` | Record › More▾ › Screens & webcam; Desk › ⋯ › Second screen | Click. On the window: F, double-click, Esc, ⌘W | **NEW visible:** Full screen and Close buttons on the Screens page; the window's own corner buttons show for 3 s when it opens, not just on hover (route map #9) |

### 2h. Settings window → areas (34)

| Old id | New home | How you reach it | Change |
|---|---|---|---|
| `set.frame.titlebar` | none needed | n/a | **retired:** pages aren't a window, so there's nothing to centre. What it was for (getting Settings off the program monitor) is now Esc or Show |
| `set.frame.find` | ⌘K search in the top bar (🔍) | ⌘K, or click 🔍 | **CHANGED:** finds everything Find did, plus desk actions (part 3) |
| `set.frame.revert` | Page footer › "Undo changes on this page" | Click; ⌘Z steps back one at a time | same idea, per page |
| `set.frame.problem` | Light items, and a mark on the rail icon and tab | Click the item → jumps to the field | **merged** |
| `set.frame.ok` | None: changes apply as you make them | n/a | **CHANGED (question 2):** no OK. Fixes Apply undoing desk changes (numbers audit #1) |
| `set.frame.apply` | "Apply (restarts cameras)" bar, only on sections that restart the engine | Click | **CHANGED:** only where a restart is needed (codec, program size, devices) |
| `set.frame.cancel` | Page footer › Undo changes on this page | Click | **CHANGED:** same job as Revert |
| `set.modal.discard` | Asked only when you leave a section with an unapplied restart bar | Leaving the section | **CHANGED:** rarely shown |
| `set.modal.reset` | More › App › Shows › Reset… | Click → confirm | moved |
| `set.modal.remove` | Every Remove button | Click → confirm | same |
| `set.inputs.sources` | Sources › Cameras (Close-ups & PTZ, People & names, Network & phone feeds, Media & playlists hold its parts) | Click Sources | **CHANGED:** remembers the selected source; "If it drops while on air" moves to the top of each source; never covers the program (forensics settings #2, #3, #5) |
| `set.inputs.audio` | Sound › Inputs | Click Sound › Inputs | moved |
| `set.inputs.guests` | Sources › More▾ › Guests | Click Sources › More▾ › Guests | moved |
| `set.sound.mixing` | Sound › Mixer (headphones and talkback parts: Sound › More▾ › Headphones & talkback) | Click Sound | moved |
| `set.sound.pads` | Sound › Pads | Click | moved |
| `set.sound.syrinscape` | Sound › More▾ › Syrinscape | Click | moved. **NEW:** volume slider and play buttons (were `syrinscape` command only) |
| `set.look.program` | Scenes › More▾ › Program picture (monitor quality: More › App › General) | Click | moved |
| `set.look.graphics` | Graphics › Titles & overlays (Taking turns, Clocks & timers, Tickers & web are filtered views) | Click Graphics | **CHANGED:** remembers the selected graphic |
| `set.look.captions` | Graphics › More▾ › Captions | Click | moved |
| `set.show.scenes` | Scenes › Scenes | Click Scenes | **CHANGED:** remembers the selected scene |
| `set.show.switching` | Scenes › Switching (stinger, FTB → More▾ › Stinger & fade to black; auto → More▾ › Auto-switching; replay → More › Show tools › Replay) | Click | moved; split up |
| `set.show.rundown` | More › Show tools › Rundown | Click | moved |
| `set.show.macros` | More › Show tools › Macros | Click | moved |
| `set.show.prompter` | More › Show tools › Teleprompter | Click | moved |
| `set.show.comments` | More › Show tools › Viewer comments | Click | **CHANGED:** always there, so you can prepare comments before setting up streaming (route map #15) |
| `set.output.recording` | Record › Recording (Quality, Editing, Podcast as sections) | Click | moved |
| `set.output.streaming` | Record › Streaming (schedule: More▾ › Scheduled) | Click | moved |
| `set.output.vertical` | Record › More▾ › Vertical version | Click | moved |
| `set.output.upload` | Record › More▾ › Upload | Click | moved |
| `set.output.outputs` | Record › More▾ › Screens & webcam | Click | moved |
| `set.general.general` | More › App › General (Shows, Export / Import / Reset: More › App › Shows) | Click | moved; split |
| `set.general.control` | More › Control (Phone remote, Stream Deck, OSC and MIDI sections) | Click | moved; split |
| `set.general.shortcuts` | More › Control › Shortcuts | Click | moved |
| `set.general.diagnostics` | More › App › Diagnostics | Click | moved. **NEW:** List devices and Test encoders buttons (were CLI only) |

### 2i. Keys (23)

Every key keeps working exactly as today, except the two marked. Each key's visible twin is
named so you can see what it does.

| Old id | New home (visible twin) | How you reach it | Change |
|---|---|---|---|
| `key.settings` | Rail | ⌘/Ctrl+, | **CHANGED:** opens the last-used area |
| `key.undo` | Edit layout and page footers › Undo | ⌘/Ctrl+Z | same key; now also undoes area-page edits |
| `key.redo` | Edit layout and page footers › Redo | ⌘⇧Z / Ctrl+Y | same |
| `key.record` | REC pill | ⌘/Ctrl+R | same |
| `key.transition` | Bank CUT / MIX | T | same |
| `key.edit` | Show › Edit layout tab | E | **CHANGED:** switches to the tab (E again goes back to Switch) |
| `key.marker` | ◆ | M | same |
| `key.marker_named` | ◆ ▾ › Named marker… | Shift+M | same, plus the new button |
| `key.auto` | ⋯ › Auto switching | A | same |
| `key.take` | TAKE | Space | same |
| `key.cut` | CUT | Enter | same |
| `key.captions` | CC | C | same |
| `key.segment_next` | Rundown panel Next ▶ | N | same |
| `key.segment_prev` | Rundown panel ◀ | Shift+N | same |
| `key.clip` | ✂ | K | same |
| `key.talkback` | TALK | Hold B | same |
| `key.back` | **NEW** ⋯ › ↩ Last scene | Backspace; Ctl `back` | **NEW visible home** (route map #2). Not the same as Back in the app (part 4) |
| `key.number` | Bank / tiles | 1–9, Shift+1–9 | same |
| `key.graphic` | Column › Graphics | Alt/⌥+1–9 | same; hint shows ⌥ on a Mac |
| `key.pad` | Column › Sound pads | F1–F12 | same |
| `key.custom` | More › Control › Shortcuts | Your own keys | same |
| `key.display` | Second screen window | F, Esc, ⌘W | same |
| `key.prompter_page` | /prompter page bar | Space, arrows, Home | **NEW:** the bar shows these keys (route map #10) |

### 2j. Commands (31)

Commands are reached through Stream Deck "Command", OSC, MIDI, macros, your own keys and
`/api/action?do=`. None changes. The visible twin is the button that does the same thing.

| Old id | Visible twin | Phone | Change |
|---|---|---|---|
| `cmd.scene` | Bank scene buttons | Scenes | same |
| `cmd.source` | Multiview tile | — | same |
| `cmd.take` | TAKE | Scenes | same |
| `cmd.cut` | CUT | Scenes | same |
| `cmd.transition` | CUT / MIX / ⋯ | Scenes | same |
| `cmd.ftb` | ⋯ › Fade to black | Scenes | same |
| `cmd.record` | REC pill | always | same |
| `cmd.stop` | REC pill hold | always | same |
| `cmd.golive` | LIVE pill | always | same |
| `cmd.endstream` | LIVE pill | always | same |
| `cmd.marker` | ◆ | always | same |
| `cmd.graphic` | Column › Graphics | Titles | same |
| `cmd.title` | ✎ | — | same |
| `cmd.captions` | CC | Scenes | same |
| `cmd.auto` | ⋯ › Auto switching | Scenes | same |
| `cmd.gain` | AUDIO fader | Sound (**NEW** fader) | fader range now matches −60…+12 |
| `cmd.mute` | AUDIO ON | Sound | same |
| `cmd.talkback` | TALK | Sound | same |
| `cmd.pad` | Sound pads | Sound | same |
| `cmd.syrinscape` | **NEW** Sound › More▾ › Syrinscape volume and play | Sound | **NEW visible home** |
| `cmd.timer` | **NEW** ⏯ ↺ on timer buttons | Titles › Timers | **NEW visible home** |
| `cmd.rundown` | Rundown panel | More › Rundown | same |
| `cmd.media` | Tile transport; **NEW** Sources › More▾ › Media & playlists (next / restart for any media source) | More › Media | **NEW visible home** for non-tile media |
| `cmd.clip:N` | **NEW** ✂ ▾ 10 / 30 / 60 s | — | **NEW visible home** |
| `cmd.replay:N` | **NEW** ⋯ › Replay ▸ Last N s | More › Replay | **NEW visible home** |
| `cmd.macro` | Macros panel | More › Macros | same |
| `cmd.wait` | More › Show tools › Macros › step "Wait" | — | a macro step; same |
| `cmd.ptz` | PTZ panel | More › PTZ | same |
| `cmd.prompter` | Teleprompter panel | More › Teleprompter | same |
| `cmd.comment` | Comments panel | More › Comments | same |
| `cmd.guest` | Guests panel | — | same |

### 2k. Phone remote and prompter page (25)

The phone gets four tabs: **Scenes | Sound | Titles | More**. The top strip (status, RECORD,
GO LIVE, ＋Marker, ✂ Clip) stays on every tab, because REC and markers are used on the phone half
the time (usage audit rank 3 and 6). The last tab you used is remembered in the phone's own
browser storage.

| Old id | New home on the phone | Change |
|---|---|---|
| `rem.pin` | PIN page (before the tabs) | same |
| `rem.header.record` | Top strip | same (hold to stop) |
| `rem.header.live` | Top strip | **CHANGED:** hold to go live as well as to end (forensics phone #3); shows CONNECTING and each destination's state (forensics phone #2, numbers audit #5) |
| `rem.header.marker` | Top strip | same |
| `rem.header.clip` | Top strip | same |
| `rem.header.replay` | More › Replay | moved (rank 19) |
| `rem.header.ftb` | Scenes › switching row, hold | **CHANGED:** needs a hold (forensics phone #3) |
| `rem.status` | Top strip | same; now includes "recording problem" (numbers audit #12) |
| `rem.monitor` | Scenes (top) | other tabs show a slim "On air: Wide" line |
| `rem.cut_take` | Scenes | same |
| `rem.rundown` | More › Rundown | moved |
| `rem.scenes` | Scenes | **CHANGED:** buttons are updated in place, not rebuilt, so taps aren't lost (forensics phone #1, *measured*) |
| `rem.switching` | Scenes: Cut, Mix, then "More transitions ▾", Auto, CC | Wipe / Push / Dip / Stinger go behind ▾ |
| `rem.graphics` | Titles | same |
| `rem.macros` | More › Macros | moved |
| `rem.ptz` | More › PTZ | moved |
| `rem.prompter` | More › Teleprompter | moved |
| `rem.comments` | More › Comments | **CHANGED:** you can read a comment before putting it on air |
| `rem.timers` | Titles › Timers | moved |
| `rem.sound` | Sound (pads, Syrinscape, TALK) | same |
| `rem.audio` | Sound (ON / OFF and **NEW** faders) | OFF needs a hold |
| `rem.media` | More › Media | moved |
| `rem.tally_links` | More › Tally lights | moved |
| `rem.tally` | Tally page (`?tally=N`) | same; also linked from More › Control › Phone remote on the desk |
| `prompt.page` | /prompter page | same; the bar shows its keys |

### 2l. HTTP, Stream Deck, OSC/MIDI and command line (27)

These have no screen of their own. Each keeps working and each now has a page that explains it.

| Old id | Where it's explained / controlled | Change |
|---|---|---|
| `api.state` | More › Control › Phone remote | same |
| `api.ws` | More › Control › Phone remote | same |
| `api.action` | More › Control › Phone remote (example links) | same |
| `api.prompter` | More › Show tools › Teleprompter (link to the page) | same |
| `api.program_jpg` | More › Control › Phone remote | same |
| `api.tally` | More › Control › Phone remote › Tally lights | **NEW visible home** (route map #11) |
| `api.key_png` | More › Control › Stream Deck | same |
| `api.icon` | none (the phone's home-screen icon) | same; listed so the test sees it |
| `sd.*` (26 actions) | More › Control › Stream Deck: install the plugin, list of all 26 actions | same |
| `osc.in` | More › Control › OSC and MIDI | same |
| `osc.feedback` | More › Control › OSC and MIDI | same |
| `midi.in` | More › Control › OSC and MIDI | same (fader 0 = −60 dB now shows as −60) |
| `midi.learn` | More › Control › OSC and MIDI › Learn | same |
| `midi.lights` | More › Control › OSC and MIDI › Light up pads | same |
| `cli.version` | More › App › Updates & about shows the version | same |
| `cli.list_devices` | **NEW** More › App › Diagnostics › List devices | **NEW visible home** |
| `cli.vcam_install` | **NEW** Record › More▾ › Screens & webcam › Install virtual camera | **NEW visible home** (route map #14) |
| `cli.vcam_uninstall` | **NEW** same page › Remove virtual camera | **NEW visible home** |
| `cli.config` | More › App › Diagnostics › Settings file (Show in folder) | same |
| `cli.demo` | command line only (developer) | same |
| `cli.init` | command line only; the first-show guide does this in the app | same |
| `cli.probe_encoders` | **NEW** More › App › Diagnostics › Test encoders | **NEW visible home** |
| `cli.stream_to` | command line only; Record › Streaming in the app | same |
| `cli.measure_sync` | Sound › Inputs › Lip sync › Measure… (already in the app as `dlg.sync`) | same |
| `cli.headless` | command line only (with --seconds, --switch-every, --stream, --no-record, --markers) | same |
| `cli.restream_test` | command line only (developer) | same |
| `cli.ui_test` | command line only (developer) | same |

### Count

| Group | Rows |
|---|---|
| 2a header right | 11 |
| 2b header left, recording, stream | 19 |
| 2c monitors and Edit layout | 32 |
| 2d multiview and bank | 13 |
| 2e bank buttons and panels | 30 |
| 2f windows and dialogs | 5 |
| 2g second screen | 1 |
| 2h Settings | 34 |
| 2i keys | 23 |
| 2j commands | 31 |
| 2k phone remote and prompter page | 25 |
| 2l HTTP, Stream Deck, OSC/MIDI, CLI | 27 |
| **Total** | **251** |

**251 of 251 route ids have a home**, matching the route map's count (4-route-map.md "Count";
re-checked by extracting every backticked id from that file: 251 unique). Stream Deck's 26 actions
stay under the one id `sd.*`, as in the route map.

**Merged, and where the pieces are still reachable:**

| Merged into | Old pieces | Each piece is still reachable at |
|---|---|---|
| The status light | `main.header.health`, `win.health`, `main.header.log`, `panel.log`, `main.header.check`, `set.frame.problem` | Light › **Now** (every Health row: drives, sources, frames, audio, compositor, plus a new stream row) · Light › **Log** (Warnings only, Copy) · Light › **Run pre-show check** · each item's **Fix ›** link. The light is the same click as Health was. Remote, OSC and Stream Deck keep their problems list. |
| Show › Edit layout tab | `bank.edit` | The tab, and E |
| Rail | `main.header.settings` | Any area, and ⌘, |
| Live edits | `set.frame.ok`, `set.frame.apply`, `set.frame.cancel` | Changes apply as made; "Apply (restarts cameras)" where needed; Undo changes on this page |
| Retired | `set.frame.titlebar` | Nothing to centre; Esc or Show |

**Routes that today are reachable only one way, and their new visible home** (route map
"Routes reachable only one obscure way", forensics "Things that look clickable…"):

| Today only by | Route | Visible home now |
|---|---|---|
| Clicking the header clock | `main.header.clock` | More › App › General › Clock |
| Backspace / command | `key.back` | ⋯ › ↩ Last scene |
| Shift+M | `dlg.marker_name`, `key.marker_named` | ◆ ▾ › Named marker… |
| ⌘Z / ⌘⇧Z outside Edit layout | `key.undo`, `key.redo` | Undo / Redo in every page footer |
| Alt-drag, scroll, double-click | `mon.edit.overlay` | Move picture tool and hint row |
| Right-click | `ctx.replay`, `ctx.clock`, `ctx.ptz.preset`, `ctx.mv.framing`, `ctx.mv.split`, `ctx.mv.playlist` | ⋯ › Replay ▸; ⏯ ↺ on timers; Save here; tile ⋯; Sources pages |
| Click/drag headings | `panel.heading.fold`, `panel.heading.reorder`, `panel.column.width` | Column ⋯ › Arrange panels (with Reset) |
| Hover | `mv.guest_warn`, `win.display` corner buttons | ⚠ chip; Screens page buttons |
| Arrow keys, tap | `key.prompter_page` | Keys printed on the prompter bar |
| Typing `?tally=N` | `rem.tally`, `api.tally` | More › Control › Phone remote › Tally lights; phone More › Tally lights |
| Commands only | `cmd.clip:N`, `cmd.replay:N`, `cmd.gain` (below −40), `cmd.syrinscape`, `cmd.media` | ✂ ▾; Replay ▸; fader to −60; Syrinscape page; Media page |
| CLI only | `cli.vcam_install`, `cli.vcam_uninstall`, `cli.probe_encoders`, `cli.list_devices` | Screens & webcam; Diagnostics |
| Hidden until streaming exists | `set.show.comments` | Always in More › Show tools |

`cmd.wait` is only ever a macro step and stays one. `cli.demo`, `cli.restream_test`,
`cli.ui_test`, `cli.headless`, `cli.init` and `cli.stream_to` are developer or scripting tools and
stay on the command line, with the in-app equivalent named where one exists.

---

## 3. The orphan test, and ⌘K search

### 3a. A list of every route, in the code

New file `src/ui/routes.rs`:

```rust
/// Every place a person can go and every action they can take. Ids are stable (4-route-map.md).
/// `was` is how it could be reached in 0.9.7; the test makes sure none of those are lost.
pub struct Route { pub id: &'static str, pub was: &'static [Input], pub legacy: bool }
#[derive(Clone, Copy, PartialEq)] pub enum Input { Mouse, Key, Phone, StreamDeck, Midi, Osc, Http, Cli, Display }

pub const ROUTES: &[Route] = &[
  Route { id: "main.rec.record", was: &[Mouse, Key, Phone, StreamDeck, Midi, Osc, Http], legacy: false },
  Route { id: "ctx.replay",      was: &[Mouse], legacy: false },
  // ... all 251
];
```

`legacy: true` marks a route that's still drawn in its 0.9.7 place (for example inside the old
Settings window) while Phase 4 is under way. Each Phase 4 PR flips its routes to `false`.

And in code, every place that draws or runs a route says which one: `nav::here("ctx.clock")` on a
button, `nav::go("record/streaming")` on a link. That makes the ids greppable.

### 3b. The tests (in `src/ui/nav.rs`, `#[cfg(test)]`)

They work the same way as the existing `every_setting_is_explained` test (settings/help.rs:507),
which reads the source with `include_str!` so it can't fall out of step.

```rust
#[test] fn every_route_has_a_home() {
    let homes = all_home_ids();  // AREAS sections + SHELL + PHONE + INPUTS
    let missing: Vec<_> = ROUTES.iter().filter(|r| !homes.contains(r.id)).map(|r| r.id).collect();
    assert!(missing.is_empty(), "routes with no home: {missing:?}");
}
#[test] fn every_home_points_at_a_real_route() {
    let ids: HashSet<_> = ROUTES.iter().map(|r| r.id).collect();
    let bad: Vec<_> = all_home_ids().into_iter().filter(|h| !ids.contains(h)).collect();
    assert!(bad.is_empty(), "homes naming no route: {bad:?}");
    // the same for VISIBLE, MERGED, and every "area/tab" string in VISIBLE and nav::go(...)
}
#[test] fn route_count_is_known() {
    assert_eq!(ROUTES.len(), 251, "a route was added or removed: update 4-route-map.md and this number on purpose");
    assert_eq!(ROUTES.iter().map(|r| r.id).collect::<HashSet<_>>().len(), ROUTES.len(), "duplicate id");
}
#[test] fn nothing_is_reachable_only_by_hover_right_click_key_or_command() {
    // Every route whose only `was` is a key, a command, the CLI, right-click (ctx.*) or hover
    // has a VISIBLE row naming a real area/tab, unless it's on the short developer list.
}
#[test] fn no_way_in_was_lost() {
    // For each route, the inputs it has now (from its homes: an area tab = Mouse, INPUTS keys =
    // Key, PHONE = Phone, a command = StreamDeck+Midi+Osc+Http) include everything in `was`.
}
#[test] fn merged_pieces_are_still_reachable() {
    for (id, _) in MERGED { assert!(all_home_ids().contains(id), "{id} merged but has no home"); }
}
#[test] fn tabs_are_tidy() {
    // unique area ids and tab ids within an area; More▾ tabs come after the plain ones;
    // every area has at least one plain tab; at most 4 plain tabs (the 1024-wide top bar fits 4).
}
#[test] fn code_uses_only_known_ids() {
    // grep include_str!(ui.rs, ui/*.rs, settings/*.rs, control/remote.html) for here("…") / go("…")
    // and data-route="…": each must be in ROUTES or a real area/tab.
}
#[test] fn rehomed_count() {
    let done = ROUTES.iter().filter(|r| !r.legacy).count();
    println!("Rehomed: {done} of {} routes", ROUTES.len());   // the PR checklist line (part 5)
}
```

### 3c. "Find a setting" becomes ⌘K

**What Find indexes today.** `help::search` (settings/help.rs:116) looks through three things,
best first, and every typed word must match:

1. **Setting names**: every `(page, "name")` row in the help table `for_page`
   (help.rs:158–496), read out of the file itself by `entries()` / `parse_entries`
   (help.rs:81, 86). That's **289 entries across 23 pages**. (Shortcuts has none.) The biggest are
   Video sources 47, Graphics 29, Recording 27, Switching 23, Streaming 22, Mixing 21, Audio 20.
2. **Page names**: the 24 labels in `Section::label` (settings/mod.rs:104–129), such as "Video
   sources", "Remote & MIDI", "Screens & webcam".
3. **Help text**: the sentences in each entry, so "gate" finds Audio › Noise gate.

The box (settings/mod.rs:1626) shows up to 8 hits as "Name — Page"; a click or Enter goes to the
page, lights the setting for 2.5 s and scrolls to it (`find_target` help.rs:48, `lit` help.rs:54),
and unfolds the part it's in (`open_part` help.rs:22). Esc clears it (mod.rs:585). It doesn't index
anything outside Settings, and the query is lost when Settings closes (forensics settings #2).

**How ⌘K covers it.** ⌘K (or the 🔍 in the top bar) opens one search box over whatever area you're
in. It searches, best first:

1. The same 289 setting names. Each entry's old page is mapped to its new section by the `help:`
   field in `nav.rs`. Where one old page is split across tabs (Sources is split five ways, Switching
   four), a small table names the tab for each entry (for example `(Sources, "Follow")` →
   Sources › Close-ups & PTZ), and the test below checks it.
2. Tab and section labels from `nav.rs`, **plus the 24 old page names and the 6 old group names
   and summaries** (`Group::label` / `summary`, settings/mod.rs:152, 163) as aliases, so "Remote &
   MIDI" or "Output" still finds the right place.
3. The same help text.
4. **New:** every route's label from `ROUTES` (for example "Fade to black", "Named marker",
   "12-hour clock", "Last scene", "Install virtual camera"), so desk actions can be found too. A
   hit on an action **goes to it and lights it; it never presses it**. Nothing goes on air from
   search.

Going to a hit works as it does now: switch area and tab, unfold the section, scroll, light for
2.5 s. The same `find_target` / `lit` / `open_part` code, keyed by section instead of page. The
last query stays in the box for the session.

The existing test `find_a_setting_by_name_page_or_help` (help.rs:541) is kept as it is, with
`Hit.page` replaced by the section, plus:

```rust
#[test] fn search_finds_everything_find_found() {
    for e in help::entries() {
        let hit = search(&e.label, 50).into_iter().find(|h| h.label.as_deref() == Some(&e.label));
        let hit = hit.unwrap_or_else(|| panic!("{} on {:?} not found", e.label, e.page));
        assert!(nav::section(hit.section).help.contains(&e.page), "{} lands on the wrong tab", e.label);
    }
    for p in Page::ALL { assert!(!search(p.label(), 8).is_empty(), "old page name {} finds nothing", p.label()); }
}
```

⌘K, ⌘[ and ⌘] are not taken by any built-in key today (control/shortcuts.rs:229–253). They are
added as built-ins, so the Shortcuts page warns if you've given one of them to a command of your
own.

---

## 4. What's remembered

| State | Remembered | Where | Why |
|---|---|---|---|
| Which area is open | No: Cutaway always opens on **Show › Switch** | n/a | It's the safe place to start a show. Nobody should open the app into a settings page. |
| Each area's last tab and last More▾ item | Across restarts | This computer | It's about how you work, not about the show. |
| Selected source, scene, graphic, input, destination, macro, segment | Across restarts, by **name or id**, never by position | This computer | Today they reset to the first one on every open (settings/mod.rs:411–448, `sel_source: 0` etc.; forensics settings #2). Saving by position would point at the wrong thing after a reorder, the same bug as numbers audit #4. If the thing is gone, fall back to the first. |
| Folded sections inside an area page | Across restarts | This computer | Same reason as tabs. |
| Scroll position in a page | This run only | Memory | Coming back to a page mid-scroll after a restart is disorienting. |
| Filters (Warnings only in the Log, a filter in Takes) | This run only | Memory | A filter left on silently hides things next time. |
| ⌘K query | This run only | Memory | |
| Right-column panel order, folds, width | Across restarts, **per show** (as today) | Show settings `[ui]`: `panel_order`, `folded`, `column_width` (config.rs:1428–1454) | The README promises "each show remembers its own", and a show's panels depend on what it has. No change. |
| 12/24 h clock | Per show (as today) | `[ui] clock_12h` (config.rs:1456) | No change, but the second screen and scheduled line now obey it. |
| Phone remote tab | On that phone | Phone browser storage (wrapped so a private window just starts on Scenes) | It's per phone. |

**"This computer"** means a small `nav` record saved by eframe next to the window size, in
`Main window.ron` (ui.rs:133–138), by giving `App` a `save()` that writes it with
`eframe::set_value`. I'd keep `persist_egui_memory` off (ui.rs:5460), because turning it on would
save every bit of egui's own widget state, which is noisy and can break when egui is updated. The
show config isn't the place either: these are habits of the person at this computer, they'd
otherwise travel with a show you send to someone, and writing them would mark the show as changed.

**Back and forward.** In a desktop app "Back" means only "the page I was on before", never "undo".

* **⌘[** goes back and **⌘]** goes forward (Ctrl+[ and Ctrl+] on Windows; also the mouse's back
  and forward buttons). They move between area/tab/section visits from this run, up to 20.
  Restarting clears them.
* **Esc** (no text field focused, no dialog open) goes straight to Show › Switch.
* Back never changes anything on air, never undoes an edit, and never opens Edit layout while
  you're recording.
* **Backspace stays "back to the last scene"** (key.back). It's a different thing, so the app's
  Back deliberately doesn't use it. The cheat sheet says so.

---

## 5. Phase 4 build order

Each PR is one area or one layer, small enough for one builder, in the order the usage evidence
says matters. Each PR description ends with the line the `rehomed_count` test prints:

> **Rehomed: N of 251 routes** (this PR: +n — `id`, `id`, …)

PR 0 starts at 0 of 251, with every route `legacy: true` and homed in its old place, so the orphan
test passes from day one and each PR can only move the number up.

| PR | What | Routes rehomed (running total) | Fixes that go with it |
|---|---|---|---|
| **0. Route registry and test** | `routes.rs` with 251 ids, `nav.rs` with the tree, all tests in part 3. No visible change. Also the one-line Apply fix: Settings writes back only fields you changed on the page, not the whole copy | 0 → 0 of 251 | numbers audit #1 (Apply reverts desk changes): the H fix shouldn't wait for the redesign |
| **1. Shell: rail, top bar, status cluster** | Left rail, top-bar tabs (areas still open the old Settings pages inside the page frame), REC / LIVE pills, time left, clock, the status light with Now / Log / Run check | +38 → 38 (2a, 2b header and stream rows, status-light merges, win.about, win.update, win.health, panel.log, dlg.precheck, confirm dialogs) | forensics header #1 (overlap at 1024), #2 (time left never truncated), #3 (hold hint), #4; numbers #3 (false 0 fps), #5 (LIVE while connecting), #15 and #16 (one low-disk limit, one wording), #17 (amber from the last minute only), #22 (hints read shortcuts.rs), #26 (one GB format), #29 (one "offline") |
| **2. ⌘K search** | Search over `nav.rs`, help entries and route labels; the Find box goes | +1 → 39 (`set.frame.find`) | forensics settings #2 (Find cleared) |
| **3. Phone remote tabs** | Scenes / Sound / Titles / More; buttons updated in place | +25 → 64 (2k) | forensics phone #1 (lost taps, *measured*), #2 (per-destination LIVE), #3 (holds for FTB, mute, GO LIVE; pre-check result on the phone), #4 (read comments, faders), #5; numbers #12, #18 (publish state outside the UI frame), #27 |
| **4. Show › Switch: bank and multiview** | CUT / MIX / ⋯, ms, AUTO greyed, scenes fit, tile click selects, tile ⋯ | +24 → 88 (2d, bank.scene, bank.overflow, mon.*, key.back, ctx.replay, main.rec.replay) | forensics bank #1–#5, multiview #1–#2; numbers #14, #23, M5 (tile vs scene numbers: tiles show the source name, not a number) |
| **5. Show › Switch: right column and audio strip** | Arrange panels, one Set up link, Graphics always shown, AUDIO fixed heading, PTZ Home moved, timer buttons, PTZ Save here | +28 → 116 (2e panels, audio.*, ctx.clock, ctx.ptz.preset, dlg.marker_name) | forensics column #1 (height − 260 px, ui.rs:5521), #2–#6; numbers #2 (fader −60), #9, #21, #24, #28 |
| **6. Sources** | Cameras, Close-ups & PTZ, People & names, More▾. First area to use live edits and the restart bar | +6 → 122 | forensics settings #2, #3, #4 (names in one place), #5 ("If it drops" first); guide #1 (People & names offers "Make a name title"), guide #3 (test patterns aren't "cameras"), guide #5 (close-up per person from one webcam) |
| **7. Sound** | Mixer, Inputs, Pads, More▾ | +6 → 128 | numbers #2 (Settings gain rounding), #4 (one meter tap; switching levels by input id, muted ≠ talking); guide #2 (pick the USB mic, not the first input) |
| **8. Record** | Takes, Recording, Streaming (empty state), More▾ | +14 → 142 | forensics after-show #1 (Open in Resolve), #2 (markers after the take), #3, #4; numbers #6 (REC clock zero), #7 (one size), #13, #19, #20, #25 |
| **9. Graphics** | Titles & overlays, More▾ | +3 → 145 | forensics column #4; numbers #8 (12 h everywhere), #11 (Until after midnight), #28 |
| **10. Scenes and Show › Edit layout** | Scenes, Switching, More▾; Edit layout as a tab with Undo / Redo, Move picture tool, Delete confirm, ON AIR banner when Preview is off | +27 → 172 | forensics edit #1, #2; settings #6 (transition style lives in one place) |
| **11. More: Control, Show tools, App; remove the Settings window** | The last pages move; the Settings window code is deleted; CLI-only tools get buttons | +79 → 251 (whatever is left: More pages, `sd.*`, OSC/MIDI, `api.*`, `cli.*`, and the keys and commands whose visible twin lives on these pages; a key or command flips in whichever PR draws its twin) | numbers #9, #10 (rundown drift), route map #14, #15; forensics guide #4 (guide reopenable, progress saved) |

The running totals are the plan, not a promise. The test prints the real figure. A PR may only
flip a route to `legacy: false` once its new home is drawn and every `was` input still reaches it.

---

## 6. Questions for Brayden (each with my recommended answer)

1. **When Preview is off, should clicking a multiview tile still put that camera on air?**
   *Recommended: no.* A click selects it (so you can look at Maya's framing) and shows a "Put on air"
   button; Shift-click and the number keys still cut straight away. The forensics found this is
   exactly where people click to look closer (forensics multiview #1).

2. **Should changes in the area pages take effect as you make them, with no OK / Apply?**
   *Recommended: yes*, with Undo on every page and an "Apply (restarts cameras)" bar only for the
   few settings that restart the engine. It removes the bug where Apply undid your fader moves
   mid-show (numbers audit #1).

3. **Do you use the rundown during a show?** *Recommended: put it under More › Show tools* (no
   persona used it). The rundown panel still appears on the desk as soon as a rundown has segments,
   and N / Shift+N still work. If you do use it, it becomes Show's third tab.

4. **Should Guests and Captions sit under More▾ rather than as tabs?** *Recommended: yes for both*
   (0 visits for every persona). If you plan remote-guest shows soon, Guests can become a Sources
   tab with no other changes.

5. **Should Cutaway always open on Show › Switch, even if you closed it in another area?**
   *Recommended: yes.* Each area still remembers its own last tab and selection for when you click
   it.
