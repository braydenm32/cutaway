# 4 — Route map (Phase 1, read-only)

Every place a user can go and every action they can take, each with a stable id. Format: `id`: what it is (file:line). How to reach it. [condition]. Line numbers refer to the tree as read on 2026-10-31.

## Main window: header, right side (ui.rs:2337)

- `main.header.help`: ? cheat sheet (ui.rs:2338). Click. Shows shortcuts.rs cheat_sheet.
- `main.header.settings`: Settings (ui.rs:2341). Click or Cmd/Ctrl+, .
- `main.header.update`: Update button (ui.rs:2351). Click. [an update is available]
- `win.update`: Update window (ui.rs:2362, update_window ui.rs:597). From main.header.update.
- `main.header.typing`: Typing indicator (ui.rs:2386). Display only. [a text field has focus, so shortcuts are off]
- `main.header.log`: Log (n) (ui.rs:2390). Click, opens `panel.log`.
- `main.header.health`: Health (n) (ui.rs:2393). Click, opens `win.health`.
- `main.header.check`: Check (ui.rs:2396). Click, opens `dlg.precheck`.
- `main.header.screen`: Screen / second display (ui.rs:2400). Click, opens `win.display`.
- `main.header.webcam`: Webcam / virtual camera toggle (ui.rs:2406). Click. [the virtual camera is installed]
- `main.header.clock`: Clock (ui.rs:2654–2667). Click to switch 12/24h. This is the only way to change it.

## Main window: header, left side (ui.rs:2416)

- `main.header.logo`: Logo (ui.rs:2424). Click, opens `win.about` (about_window ui.rs:572).
- `main.header.show`: Show menu (show_menu ui.rs:2701). Click on the show name.
- `menu.show.switch`: Switch to another show (ui.rs:2701+). Pick from the menu, then `dlg.confirm.switch`.
- `menu.show.save_as`: Save as a show… (ui.rs:~2718).
- `main.rec.record`: RECORD (ui.rs:2571). Click or Cmd/Ctrl+R. [idle] Can open `dlg.precheck` first.
- `main.rec.stop`: REC pill, hold to stop (ui.rs:2457). Hold for REC_HOLD_SECS, or confirm `dlg.confirm.stoprec`. [recording]
- `main.rec.marker`: ◆ Marker (ui.rs:2492). Click or M. Shift+M opens `dlg.marker_name`. [recording]
- `menu.markers`: ▾ marker list (ui.rs:2498, marker_list ui.rs:4327). Click. [recording, markers exist]
- `main.rec.clip`: ✂ Clip (ui.rs:2508). Click or K. [recording]
- `main.rec.replay`: Replay (ui.rs:2516). Click. [recording]
- `ctx.replay`: Replay right-click menu (ui.rs:2525): Slow motion, Program, per-camera, Quad, Stop.
- `main.rec.recent`: Recent takes ▾ (ui.rs:2578, takes.rs ~233): Upload, Open in Resolve, Show in Finder. [idle]
- `main.rec.takecard`: Last take card (takes.rs ~300): Show in Finder, Open in Resolve, Upload, ✕. [after a take]
- `main.stream.golive`: GO LIVE (ui.rs:2642). Click. [destinations set up] Can open `dlg.precheck`.
- `main.stream.setup`: Set up streaming (ui.rs:2635). Opens Settings › Streaming. [no destinations]
- `main.stream.live`: LIVE pill; click to end (ui.rs:2624). Then `dlg.confirm.endstream`. [live]
- `main.stream.panel`: Stream detail panel (stream_panel ui.rs:4042). [live]
- `main.stream.scheduled`: Scheduled go-live line (ui.rs:1972/2005). [a schedule is set]

## Monitors (ui.rs:2801)

- `mon.engine_error`: Engine error screen (ui.rs:2813–2821): Open settings / Try again / Save a problem report. [the engine failed]
- `mon.starting`: Starting slate (starting.rs:143). [the engine is starting]
- `mon.waiting.rescue`: Rescue card (waiting.rs:163): OK, Show the folder. [a crashed take was recovered]
- `mon.preview`: Preview monitor (ui.rs:2892). [preview mode on]
- `mon.preview.take`: TAKE (ui.rs:2922). Click or Space.
- `mon.preview.cut`: CUT (ui.rs:2930). Click or Enter.
- `mon.tall`: 9:16 monitor (ui.rs:2937). Click, opens Settings › Vertical. [vertical output on]
- `mon.program`: Program monitor (ui.rs:2957), with chips: no signal, AUTO, WEBCAM, SCREEN, "CC · stream only" (ui.rs:3018).
- `mon.edit.overlay`: Edit layout overlay (edit_overlay ui.rs:3028). Drag to move, drag a corner to resize, Shift keeps the shape, scroll to zoom, double-click to fill/fit, Alt-drag to pan. [edit mode]
- `mon.edit.toolbar`: Edit toolbar (edit_toolbar ui.rs:3190). [edit mode]
- `edit.done`: Done (ui.rs:3208).
- `edit.save_scenes`: Save scenes (ui.rs:3219).
- `edit.redo`: Redo (ui.rs:3227). Also Cmd+Shift+Z or Ctrl+Y.
- `edit.undo`: Undo (ui.rs:3233). Also Cmd/Ctrl+Z.
- `edit.frame.add`: Add source (ui.rs:3250).
- `edit.frame.remove`: Remove source (ui.rs:3250+).
- `edit.frame.front`: To front.
- `edit.frame.full`: Full frame.
- `edit.frame.fitfill`: Fit / Fill.
- `edit.frame.zoom`: Zoom slider.
- `edit.frame.reset`: Reset picture (ui.rs:3297).
- `edit.scene.name`: Scene name field.
- `menu.edit.audio`: Audio (n) menu (ui.rs:3331).
- `menu.edit.cues`: Cues (n) menu with ＋Add (ui.rs:3365).
- `menu.edit.newscene`: New scene menu.
- `edit.scene.duplicate`: Duplicate (ui.rs:3401).
- `edit.scene.delete`: Delete (ui.rs:3406).

## Multiview (ui.rs:3478)

- `mv.tile`: Click a tile to preview, Shift-click to cut (ui.rs:3531). Also keys 1–9 (Shift = cut).
- `mv.media`: Media transport ⏭ ⏯ ⟲ (ui.rs:3565). [media source]
- `mv.framing`: Auto-framing chip (ui.rs:3584).
- `ctx.mv.framing`: Right-click: Follow / Shot (ui.rs:3597).
- `ctx.mv.playlist`: Playlist right-click (ui.rs:3633). [playlist source]
- `ctx.mv.split`: Split right-click (ui.rs:3649). [split source]
- `mv.guest_warn`: Guest link warning tooltip. Hover only.

## Scene bank (ui.rs:3722)

- `bank.transition`: TRANSITION picker: Cut/Mix/Wipe/Push/Dip/Stinger (ui.rs:3731). Also T.
- `bank.ftb`: FTB (ui.rs:3749).
- `bank.ms`: Transition ms (ui.rs:3761). Drag.
- `bank.preview`: Preview toggle (ui.rs:3766).
- `bank.auto`: AUTO (ui.rs:3777). Also A.
- `bank.cc`: CC captions (ui.rs:3786). Also C. [captions set up]
- `bank.edit`: Edit layout (ui.rs:3792). Also E.
- `bank.scene`: Scene buttons (ui.rs:3889). Click; drag to reorder in edit mode. Keys 1–9.
- `bank.overflow`: Edge arrows "+n ›" (ui.rs:3836/3854). [more scenes than fit]

## Right-column panels (main layout ui.rs:5464; section_heading panels.rs:38)

- `panel.heading.fold`: Click a heading to fold it (panels.rs:38).
- `panel.heading.reorder`: Drag a heading to reorder (panels.rs:38).
- `panel.heading.setup`: "Set up" link (panels.rs:54). [panel empty]
- `panel.column.width`: Drag the column edge (ui.rs:5464).
- `panel.rundown`: Rundown (panels.rs:83): ◀ (142), Next/Start (145) also N / Shift+N, ■ (150), Go to ▾ (153).
- `menu.rundown.goto`: Go to ▾ menu (panels.rs:153).
- `panel.guests`: Guests (panels.rs:172): ✕ (244), Copy link (254).
- `win.guest_qr`: Guest QR window (panels.rs:251/270).
- `panel.sound`: Sound pads (panels.rs:296): click a pad, F1–F12, ■ Stop (355), Syrinscape buttons.
- `panel.sound.talk`: TALK, hold, or B (panels.rs:419).
- `panel.graphics`: Graphics (graphics_panel ui.rs:4116): click to toggle, Alt+1–9.
- `panel.graphics.edit_title`: ✎ opens `dlg.title` (ui.rs:4181, title_dialog ui.rs:4233).
- `panel.graphics.reload`: ⟳ reload web graphic (ui.rs:4185).
- `panel.graphics.rotation`: Rotation buttons (ui.rs:4201).
- `ctx.clock`: Clock right-click: Start/Pause, Back to start/zero, Stop and reset (clock_menu panels.rs:433).
- `panel.macros`: Macros (production.rs:124).
- `panel.ptz`: PTZ (production.rs:219): joystick (274), zoom (277), Home (288), ＋ new preset (308).
- `ctx.ptz.preset`: Click a preset to recall; right-click to save (production.rs:302).
- `panel.comments`: Comments (production.rs:412): click to show/hide, Show next (451), Hide. [destinations or comments set up]
- `panel.prompter`: Prompter (production.rs:621): ⏯ ⏮ ◀ ▶▶, − wpm ＋ (649), position slider (655).
- `panel.uploads`: Uploads (uploads.rs:468): Carry on / Forget them (488–491), Upload / Not this one (521–525), ✕ (547/558), Open (565), Retry (573), Clear finished (600).
- `panel.audio`: Audio strips (audio_panel ui.rs:4379).
- `audio.fader`: Fader (ui.rs:4425).
- `audio.onoff`: ON/OFF (ui.rs:4433).
- `audio.solo`: 🎧 solo (ui.rs:4442).
- `audio.phones`: Headphone volume (ui.rs:4511).
- `panel.log`: Log panel: Warnings only, Copy (log_panel ui.rs:4552).

## Windows and dialogs

- `win.about`: About (ui.rs:572).
- `win.health`: Health (ui.rs:4580).
- `dlg.precheck`: Pre-check (ui.rs:4680/4693): Start, Start anyway, Cancel, Close, Check again, Measure lip sync…. 20 s timeout.
- `dlg.sync`: Lip-sync measurement (sync_dialog ui.rs:4816): Start, Try again, Apply, Close. From dlg.precheck.
- `dlg.confirm.endstream`: End stream (confirm_dialog ui.rs:3989).
- `dlg.confirm.stoprec`: Stop recording (ui.rs:3989).
- `dlg.confirm.switch`: Switch show (ui.rs:3989).
- `dlg.confirm.quit`: Quit while recording or live (ui.rs:3989).
- `dlg.title`: Title edit (ui.rs:4233).
- `dlg.marker_name`: Named marker (ui.rs:4289). Shift+M only.
- `win.guide`: First-run guide (guide.rs:20). [first run]
- `win.display`: Second screen (display_window ui.rs:4942): Program / Multiview / Prompter view, presenter clocks (5003), double-click or F for full screen, Esc / Cmd+W to close, corner buttons (5083–5084).

## Settings window (settings/mod.rs; frame show_window :573)

Frame:
- `set.frame.titlebar`: Title bar Centre / ✕; double-click the title to centre it.
- `set.frame.find`: Find box (mod.rs:1626).
- `set.frame.revert`: Revert.
- `set.frame.problem`: Problem link (jumps to the field).
- `set.frame.ok`: OK.
- `set.frame.apply`: Apply.
- `set.frame.cancel`: Cancel.
- `set.modal.discard`: Discard changes? modal.
- `set.modal.reset`: Reset modal.
- `set.modal.remove`: Confirm remove modal (mod.rs:937).

Groups and pages (mod.rs:152–181):
- `set.inputs.sources`: Sources (sources.rs:111): Split (157), PTZ (549), Look (551), Playlist (734), preview (preview.rs:40), capture picker (85).
- `set.inputs.audio`: Audio (mod.rs:1088).
- `set.inputs.guests`: Guests (guests.rs:61).
- `set.sound.mixing`: Mixing (mixing.rs:113; parts at 140/177/202/235/260).
- `set.sound.pads`: Pads (sound.rs:23).
- `set.sound.syrinscape`: Syrinscape (sound.rs:104).
- `set.look.program`: Program (mod.rs:1279).
- `set.look.graphics`: Graphics (graphics.rs:14; rotations.rs:12).
- `set.look.captions`: Captions (captions.rs:8).
- `set.show.scenes`: Scenes (scenes.rs:88).
- `set.show.switching`: Switching (switching.rs:7): stinger (51), FTB (101), replay (115), auto (157).
- `set.show.rundown`: Rundown (rundown.rs:66).
- `set.show.macros`: Macros (macros.rs:9).
- `set.show.prompter`: Prompter (prompter.rs:6).
- `set.show.comments`: Comments (comments.rs:6). [destinations or comments set up]
- `set.output.recording`: Recording (mod.rs:1351). Tabs: Quality (1431), Editing (1519), Podcast (1582).
- `set.output.streaming`: Streaming (mod.rs:1674): Quality (1804), Going live (1840), schedule (scheduled.rs:75).
- `set.output.vertical`: Vertical (vertical.rs:9).
- `set.output.upload`: Upload (upload.rs:28).
- `set.output.outputs`: Outputs (outputs.rs:9).
- `set.general.general`: General (mod.rs:1028): Shows (control.rs:244), Export / Import / Reset (1061–1080).
- `set.general.control`: Control (control.rs:37): Remote (41), Stream Deck (96), OSC (124), MIDI with Learn (173).
- `set.general.shortcuts`: Shortcuts (shortcuts.rs:16), "Always" list (148).
- `set.general.diagnostics`: Diagnostics (mod.rs:1894).

## Keys (control/shortcuts.rs; handle_keys ui.rs:1789, run_key_command 1848, numbered_key 1890, run_builtin 1908)

- `key.settings`: Cmd/Ctrl+,
- `key.undo`: Cmd/Ctrl+Z
- `key.redo`: Cmd/Ctrl+Shift+Z, or Ctrl+Y on Windows
- `key.record`: Cmd/Ctrl+R
- `key.transition`: T
- `key.edit`: E
- `key.marker`: M
- `key.marker_named`: Shift+M
- `key.auto`: A
- `key.take`: Space
- `key.cut`: Enter
- `key.captions`: C
- `key.segment_next`: N
- `key.segment_prev`: Shift+N
- `key.clip`: K
- `key.talkback`: B (hold)
- `key.back`: Backspace (back to the last scene)
- `key.number`: 1–9 preview, Shift+1–9 cut (numbered_key ui.rs:1890)
- `key.graphic`: Alt+1–9
- `key.pad`: F1–F12
- `key.custom`: User-bound commands (shortcuts.rs custom keys, release_of)
- `key.display`: Second screen: F, Esc, Cmd+W
- `key.prompter_page`: Prompter page: Space, arrows, Home (prompter.html:145)

## Commands (control/mod.rs Action enum :192, parse :263; run_action ui.rs:1228)

These are reached through OSC `/cutaway/<verb>`, MIDI mappings, Stream Deck "Command", custom keys, macros and `/api/action?do=`.

- `cmd.scene`, `cmd.source`, `cmd.take`, `cmd.cut`, `cmd.transition`, `cmd.ftb`
- `cmd.record`, `cmd.stop`, `cmd.golive`, `cmd.endstream`, `cmd.marker`
- `cmd.graphic`, `cmd.title`, `cmd.captions`, `cmd.auto`
- `cmd.gain` (−60..12, mod.rs:315), `cmd.mute`, `cmd.talkback`
- `cmd.pad`, `cmd.syrinscape` (volume, play by id)
- `cmd.timer` (start/pause/reset), `cmd.rundown` (next/prev/go N)
- `cmd.media` (play/pause/next/restart), `cmd.clip:N`, `cmd.replay:N`
- `cmd.macro`, `cmd.wait`, `cmd.ptz`, `cmd.prompter`, `cmd.comment`, `cmd.guest`

## Remote page (control/remote.rs, remote.html)

- `rem.pin`: PIN screen with lockout (remote.html:106).
- `rem.header.record`: RECORD / REC, hold to stop (remote.html:72–77).
- `rem.header.live`: GO LIVE / LIVE, hold to end.
- `rem.header.marker`: ＋Marker.
- `rem.header.clip`: ✂ Clip Ns.
- `rem.header.replay`: Replay.
- `rem.header.ftb`: FTB.
- `rem.status`: Status line (79).
- `rem.monitor`: Program image (80).
- `rem.cut_take`: CUT / TAKE row (81).
- `rem.rundown`: Rundown (85).
- `rem.scenes`: Scenes (87).
- `rem.switching`: Switching, including stinger (89).
- `rem.graphics`: Graphics (91).
- `rem.macros`: Macros (92).
- `rem.ptz`: PTZ (93).
- `rem.prompter`: Prompter (94).
- `rem.comments`: Comments (95).
- `rem.timers`: Timers (96).
- `rem.sound`: Sound (97).
- `rem.audio`: Audio (100).
- `rem.media`: Media (101).
- `rem.tally_links`: Tally links (102).
- `rem.tally`: Tally view, `?tally=N` (108/353).
- `prompt.page`: /prompter page (prompter.html): bar ⏮ ◀ ⏯ ▶▶ − wpm ＋ (28), tap to toggle (141).

HTTP endpoints: `api.state` /api/state, `api.ws` /api/ws, `api.action` /api/action?do=, `api.prompter` /api/prompter, `api.program_jpg` /api/program.jpg, `api.tally` /api/tally/N, `api.key_png` /api/key/{scene|source}/N.png, `api.icon` /icon.png.

## Stream Deck (streamdeck/app.cutaway.streamdeck.sdPlugin, keys.rs)

- `sd.*`: 26 actions: Scene, Source, Take/Cut, Transition, Record, Go Live, Marker, Graphic, Audio Input, Auto, Captions, Media, Pad, Syrinscape, Talkback, Clip, Timer, Rundown, Guest, FTB, Replay, Macro, PTZ, Prompter, Viewer Comment, Command.

## OSC / MIDI

- `osc.in`: `/cutaway/<verb>`, which maps onto cmd.* (control/osc.rs).
- `osc.feedback`: rec_time, live_time, level and state.
- `midi.in`: Mapped notes and CCs, which map onto cmd.*; fader 0 = −60 dB (mod.rs:242).
- `midi.learn`: Learn (settings/control.rs:173).
- `midi.lights`: Feedback LEDs (ui.rs:~1740).

## CLI only (main.rs)

`cli.version`, `cli.list_devices`, `cli.vcam_install`, `cli.vcam_uninstall`, `cli.config`, `cli.demo`, `cli.init`, `cli.probe_encoders`, `cli.stream_to`, `cli.measure_sync`, `cli.headless` (with --seconds, --switch-every, --stream, --no-record, --markers), `cli.restream_test`, `cli.ui_test`.

## Count

Unique route ids in this file, counted with grep:

- Desktop UI (header, recording, stream, monitors, edit, multiview, bank, panels, audio, windows and dialogs): 111
- Settings (frame, modals, 24 pages): 34
- Keys: 23
- Commands: 31
- Remote page 24 + prompter page 1 + HTTP endpoints 8: 33
- Stream Deck: 1 id (`sd.*`) covering 26 actions
- OSC/MIDI: 5
- CLI: 13 (plus 5 headless sub-flags)

**Total: 251 route ids, or 276 if each Stream Deck action counts separately.** Some desktop bullets bundle several small buttons (for example the uploads panel and the pre-check), so there are about 300 clickable actions in all.

## Routes reachable only one obscure way

1. **12/24h clock:** only by clicking the header clock (ui.rs:2654). It is not in Settings › General. The second screen and the scheduled line ignore the setting anyway.
2. **Back to the last scene:** only Backspace or a command (shortcuts.rs; run_builtin ui.rs:1908). There is no button.
3. **Named marker:** only Shift+M (dlg.marker_name ui.rs:4289). The ◆ button never offers it.
4. **Undo/Redo of Settings and title changes outside Edit layout:** only Cmd+Z / Cmd+Shift+Z. The buttons exist only in the edit toolbar (ui.rs:3227–3233).
5. **Picture pan:** only Alt-drag (edit_overlay ui.rs:3028). Scroll zoom and double-click fill/fit have no labels either.
6. **Replay angle and slow motion, clock timers, PTZ preset save, auto-framing Follow/Shot, split and playlist controls:** only by right-click (ui.rs:2525, panels.rs:433, production.rs:302, ui.rs:3597/3633/3649). There is nothing on touch or the remote for some of these, and no hint that the menus exist.
7. **Panel fold, reorder and column width:** only by clicking or dragging headings and the column edge (panels.rs:38). There is no reset, and Settings Apply keeps them (ui.rs:5292).
8. **Settings window Centre:** double-click the title, or the small Centre button.
9. **Second screen full screen and close:** F, Esc, double-click (ui.rs:4942–5084). The corner buttons show only on hover.
10. **Prompter page controls:** tap to toggle, arrow keys (prompter.html:141/145). The keys are not shown anywhere.
11. **Tally view:** only by typing `?tally=N` or following the remote's Tally links (remote.html:102/353). /api/key and /api/tally are used only by the Stream Deck and lights.
12. **Commands only:** clip:N, replay:N, gain, wait, Syrinscape volume / play by id, and media next/restart on non-tile media have no button. They are reached through OSC, MIDI, Stream Deck Command, macros or custom keys.
13. **Remote RECORD / GO LIVE can open the pre-check on the desk.** The phone user cannot see or dismiss it (dlg.precheck ui.rs:4680), and the desk operator gets a modal they did not ask for.
14. **CLI-only features:** --measure-sync, --probe-encoders, --vcam-install/uninstall, --restream-test, --list-devices. The virtual camera install has no in-app route (the Webcam button shows only once it is installed, ui.rs:2406).
15. **Comments settings page:** hidden until a stream destination exists or comments are set up (mod.rs:152–181), so you cannot prepare comments before setting up streaming.
