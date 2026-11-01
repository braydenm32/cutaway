# 3 — Numbers audit (Phase 1, read-only)

Scope: every number Cutaway shows, on the desktop app (`src/ui*.rs`), the Settings window (`src/settings/*`), Health, the remote page (`src/control/remote.html`), the prompter page (`src/control/prompter.html`), OSC/MIDI feedback and Stream Deck. Line numbers refer to the tree as read on 2026-10-31. Nothing was built or run, so items marked "needs testing" are inferred from the code.

## Issues, most serious first

1. **Settings Apply reverts desk changes made while Settings was open.** `new_settings` clones `cfg` when the window opens (ui.rs:5173). `apply_settings` writes the whole draft back and keeps only `panel_order` and `folded` from the live config (ui.rs:5292). Anything the operator changed on the desk in the meantime goes back to its old value: fader gain and ON/OFF, transition kind and ms, preview mode, headphone volume, second-screen display, 12/24h clock, framing, titles, PTZ presets. During a show this looks like levels jumping for no reason.
2. **The desktop fader cannot show or hold gains below −40 dB.** The strip fader is an egui `Slider` over −40..=12 (ui.rs:4425). In egui 0.37 the slider clamps by default (slider.rs:590, slider_core.rs:262). It rewrites the bound value every frame and reports it as changed, and the change is sent to the engine (ui.rs:4431, 4521). A MIDI fader at 0 maps to −60 dB, which means "silence" (control/mod.rs:242), and `gain` commands accept −60..12 (control/mod.rs:315). Both get pulled up to −40 dB as soon as the strip is drawn. The Settings gain slider has the same problem and also rounds to 0.5 dB, which marks the page dirty when nothing was touched (settings/mod.rs:1231).
3. **Health shows a false 0 fps for every source after the engine starts or the lineup changes.** In health.rs:276–283 the frame-count baseline is reset and fps is measured in the same tick, so the first reading is 0. For about a second every camera reads "0.0 fps (set to 30)". Health(N) in the header goes up, and the remote and OSC problems list show "camera stalled" warnings that are not real.
4. **The same input shows different levels in different places.** The desktop strip meter is taken after processing and before the fader, on the program pair (audio.rs:873). The remote, OSC, the Settings audio page, the pre-check and the guide use the raw RMS `levels_db` with a 300 ms release (audio.rs:374/384). The Settings switching page reads its level by position in the draft list (switching.rs:146/186), so the bars point at the wrong mic once inputs are added or reordered and not yet applied. It also shows a muted mic as "talking", though auto-switch ignores muted mics (autoswitch.rs:157).
5. **The LIVE clock includes connect time, and the remote shows LIVE too early.** `since` is set when Go Live is pressed, before the connection is made (streamer.rs:262). So LIVE 00:00:07 can appear while nothing has been on air for 7 s. The desktop shows GOING LIVE while it connects (ui.rs:2616). The remote and Stream Deck already show LIVE (remote.html:196).
6. **The REC timer and the files use different zero points.** The header REC time uses `take.started = Instant::now()`, set after the encoders open (take.rs:303). The files, markers and take length use the media clock t0, set at the next frame boundary (take.rs:134, 388, 513). Marker times in the list (ui.rs:4339) and the take length on the card can differ from the REC clock by the encoder start time.
7. **One take, two sizes.** The take card reports size and file count from the take report (takes.rs ~300). The Recent takes menu walks the folder recursively and counts Take.txt and other extras (takes.rs ~233). Both are on screen together when the menu is open over the card.
8. **12/24h is ignored in three places.** The second-screen presenter clocks hard-code `%-I:%M:%S %p` (panels.rs:485). The "● REC" time there uses `fmt_span` m:ss, while the header uses HH:MM:SS (panels.rs:487 vs ui.rs:2457). The scheduled go-live line uses `%H:%M` (ui.rs:2020).
9. **Two segment clocks can disagree.** The rundown panel times the segment from rundown status. The Segment timer graphic gets its planned length copied once in `set_segment` (timers.rs:106), so editing the rundown after that leaves the graphic with the old length.
10. **Rundown drift counts revisited segments twice.** `go` pushes to `done` on every visit (control/rundown.rs:89), so going back and forward adds the segment's over/under again. The "on time / N over" figure (panels.rs:118) is then wrong.
11. **The Until clock breaks after its time.** `clock_text` for Until never moves to the next day and shows 0:00 once the time has passed (timers.rs:171). A countdown to 00:30 started at 23:50 shows 0:00 straight away.
12. **"Recording problem" never reaches the remote.** `poll_status` takes `rec_failed` (ui.rs:2225). `show_state` adds "recording problem" only if that flag is still set (ui.rs:1559–1562), but it was already consumed. The remote, OSC and Stream Deck never see it.
13. **The Settings "writes about X/hour" hint mixes measured and configured values.** It uses the measured disk rate of the running engine with the draft's channel settings (settings/mod.rs:1407; rate from ui.rs:5221). Changing quality in the draft does not change the estimate until Apply.
14. **The transition ms field is shown for Stinger but has no effect.** The bank DragValue (ui.rs:3761) stays live in Stinger mode. The stinger uses its own clip length (switcher.rs:182/200).
15. **The low-disk warnings use different limits.** Header amber at 1800 s (ui.rs:2555), Health amber at 1800 s (ui.rs:4619), the pre-check at 60 min, and the backup warnings at 30 and 10 min (health.rs:323). When idle, the "Last take … free" line turns amber but does not say how much time is left (ui.rs:2590).
16. **The disk figure is worded three ways.** Header: "X free · Y left" (ui.rs:2555). Health: "Recording: {describe} of {total} (rate measured/estimated)", which nests describe()'s "X free · about Y of recording" in a second sentence, so it reads badly (ui.rs:4619, health.rs:41). Remote: its own status text. `fmt_left` rounds minutes down (health.rs:62), so "0 min" shows while seconds are still left.
17. **Health Repeated/Dropped turn amber on totals for the whole session.** The amber limit is ≥30 frames since the engine started (ui.rs:4641–4653), so one bad moment early on leaves the row amber for the rest of the show.
18. **Remote state stops when the desktop stops repainting (needs testing).** `publish_state` runs only inside the UI frame and is throttled to 150 ms (ui.rs:1707). If the window is hidden or minimised and egui stops repainting, the remote, OSC and MIDI lights stop updating.
19. **The retry countdown reads "Retrying in 0 s", and the first retry has no try number.** `describe` truncates the seconds (streamer.rs:76). "(try N)" is shown only when N > 1 (ui.rs:4093–4096).
20. **The stream fps shown is not the fps sent.** The Streaming page "Quality" summary shows the configured fps (settings/mod.rs:1804). The streamer caps it at the program fps (streamer.rs:248).
21. **The guest "ms behind" is round-trip time, and its units differ.** The guests panel labels RTT as delay. Guest bitrate is in kbit/s, while stream rows use Mbps (panels.rs ~200; ui.rs:4091).
22. **Key hints are hard-coded and go wrong when keys are rebound or on another platform.** TALK "(hold, or B)" (panels.rs:419), RECORD "(Ctrl/Cmd+R)" (ui.rs:2574), the Undo/Redo "⌘Z" hints (ui.rs:3227–3236), the guide's "Press Record (R)" (guide.rs:193, while the default is Cmd/Ctrl+R), and "Alt+N" on graphics buttons, which is Option on Mac (ui.rs:4171). None of them read `shortcuts.rs`.
23. **"TAKE n" on the starting screen counts engine starts, not takes.** `starts_count` goes up on every restart (starting.rs:54, 179). After three Settings applies it says TAKE 4 with no recording made.
24. **The integrated loudness "I" has no clear starting point.** It shows a value when idle, and nothing says when it started counting (ui.rs:4485).
25. **Upload rows show "0% of 0 KB" before the first progress update** (uploads.rs:557).
26. **GB precision differs.** The guide shows "{:.0} GB" (guide.rs:160), the header and Health use describe()'s formatting, and take sizes use their own formatting (takes.rs).
27. **The remote shows rundown drift as "01:05"** (remote.html:234), while the desktop shows "1:05 over" (panels.rs:118).
28. **Rotation countdown "04:59" vs rundown "4:59".** Rotation trims "00:" from fmt_duration (ui.rs:4201). The rundown uses fmt_span.
29. **Three definitions of "offline".** The dropped-source banner uses `status.offline && !alive` (ui.rs:2257), the remote problems list uses offline alone (ui.rs:1588), and the pre-check has its own test. The same camera can be offline in one place and fine in another.

## Numbers printed twice on the same screen

| Screen | Number | Where | Note |
|---|---|---|---|
| Main | Time of day | header clock ui.rs:2654 + clock graphic button ui.rs:4155 | Same 12/24h setting (OK), but each formats it separately. |
| Main | Segment time left | rundown panel panels.rs:100 + Segment graphic button ui.rs:4155 | Can disagree (issue 9). |
| Main | Marker count | "◆ Marker (n)" ui.rs:2492 + the ▾ list length ui.rs:4327 | Agree. |
| Main + Health | Disk free / time left | header ui.rs:2555 + Health ui.rs:4619 | Different wording (issue 16). |
| Main | Problem count | Health(n) ui.rs:2393, Log(n) ui.rs:2390, Check, chips on program | Four signals for overlapping problem lists. |
| Main | Destinations live | "{live} of {total} destinations live" ~ui.rs:2630 + rows in stream_panel ui.rs:4091 | Agree. |
| Main | Bitrate | "bitrate lowered to X Mbps" ui.rs:4072 + per-row "{:.1} Mbps" ui.rs:4091 | The first is the target, the rows are measured, and both say Mbps. |
| Audio | Gain | "{:+.1} dB" label ui.rs:4432 + fader tooltip | Same value shown twice, side by side. |
| Main | Index numbers | scene buttons "1…" ui.rs:3889 + multiview tiles "1 name" ui.rs:3531 | Both start at 1 but count different things, while keys 1–9 pick scenes or sources depending on mode. |
| Main | AUTO state | program chip "AUTO (paused Ns / n talking)" ui.rs:2990 + bank AUTO button ui.rs:3777 | OK. |
| Main + Settings | Transition ms | bank DragValue ui.rs:3761 + Settings › Switching | Apply can revert the bank value (issue 1). |
| Main + 2nd screen | REC time | header HH:MM:SS ui.rs:2457 + display "● REC" m:ss panels.rs:487 | Different formats (issue 8). |

## Full table

Legend: R = refresh. "Frame" means every UI repaint, which is usually 60 Hz while active and slower when idle.

### Header and recording

| # | Number | Where | Source | Units / format | R | Consistent? | Mark |
|---|---|---|---|---|---|---|---|
| H1 | REC elapsed | ui.rs:2457 | RecState since (take.rs:323), `fmt_clock` ui.rs:5810 | HH:MM:SS | frame | remote/OSC from rec_seconds ui.rs:1550; 2nd screen m:ss | ISSUE (6, 8) |
| H2 | REC hold-to-stop progress | ui.rs:2457 | REC_HOLD_SECS | ring | frame | remote has its own hold | OK |
| H3 | Marker count | ui.rs:2492 | take markers | integer | frame | matches the list | OK |
| H4 | Marker times | ui.rs:4339 | clock::now() − 50 ms − t0 (take.rs:388) | m:ss | frame | does not match the REC clock zero | ISSUE (6) |
| H5 | Clip length | ui.rs:2508 | clip seconds config | "Ns" | static | remote "✂ Clip Ns" | OK |
| H6 | Disk free · time left (recording) | ui.rs:2555 | DiskStatus describe health.rs:41, seconds_left :36 | GB · h/min, amber <1800 s | ~1 s poll | Health/remote word it differently; fmt_left rounds down | ISSUE (15, 16) |
| H7 | Saving files N% | ui.rs:2561 | finalize progress | % | frame | remote "Saving N%" remote.html:193; show_state ui.rs:1554 | OK |
| H8 | Last take: N files, size · free | ui.rs:2590 | take report + disk | count, GB | poll | Recent takes menu differs; amber without time left | ISSUE (7, 15) |
| H9 | Background job N% | ui.rs:2595 | background task | % | frame | — | OK |
| H10 | LIVE elapsed | ui.rs:2624 | streamer since (streamer.rs:262) | HH:MM:SS | frame | includes connect time; remote shows LIVE early | ISSUE (5) |
| H11 | Destinations live | ~ui.rs:2630 | stream status | "a of b" | frame | matches the rows | OK |
| H12 | Scheduled go-live | ui.rs:2005/2020 | scheduled.rs | %H:%M | frame | ignores 12h | ISSUE (8) |
| H13 | Header clock | ui.rs:2654–2667 | local time, clock_text ui.rs:5806 | 12/24h | 1 s | 2nd screen ignores the setting | ISSUE (8) |
| H14 | Log (n) | ui.rs:2390 | warning count | integer | frame | — | OK |
| H15 | Health (n) | ui.rs:2393 | health problems health.rs:184 | integer | 1 s | false 0 fps raises it | ISSUE (3) |
| H16 | Update version | ui.rs:2351 | update check | semver | once | — | OK |

### Monitors, bank and multiview

| # | Number | Where | Source | Units | R | Consistent? | Mark |
|---|---|---|---|---|---|---|---|
| M1 | AUTO paused Ns / n talking | ui.rs:2990 | autoswitch state | s, count | frame | Settings switching level is off (issue 4) | OK |
| M2 | FILL · 1.0× zoom tag | ui.rs:3138 | layout crop | × to 1 dp | frame | toolbar Zoom slider | OK |
| M3 | Zoom slider | edit_toolbar ~ui.rs:3280 | layout | × | frame | — | OK |
| M4 | Audio (n), Cues (n) | ui.rs:3331, 3365 | scene config | count | frame | — | OK |
| M5 | Tile index "i+1" | ui.rs:3531 | source order | 1-based | frame | collides with scene numbers | ISSUE (printed twice) |
| M6 | Transition ms | ui.rs:3761 | cfg transition ms 0–3000 | ms | frame | ignored in Stinger; Settings Apply reverts it | ISSUE (1, 14) |
| M7 | Scene number | ui.rs:3889 | scene order | 1-based | frame | remote matches | OK |
| M8 | "+n ›" hidden scenes | ui.rs:3836/3854 | overflow | count | frame | — | OK |

### Stream panel

| # | Number | Where | Source | Units | R | Consistent? | Mark |
|---|---|---|---|---|---|---|---|
| S1 | Lowered bitrate | ui.rs:4072 | adaptive target | Mbps | frame | — | OK |
| S2 | Per-destination bitrate | ui.rs:4091 | kbps EMA streamer.rs:780 | "{:.1} Mbps" | frame | guests use kbit/s | ISSUE (21) |
| S3 | Retrying in N s (try N) | ui.rs:4093–4096, streamer.rs:76 | backoff | s | frame | shows 0 s; first try unlabelled | ISSUE (19) |
| S4 | Stream fps (Settings) | settings/mod.rs:1804 | config | fps | static | capped at program fps | ISSUE (20) |

### Graphics, titles, markers

| # | Number | Where | Source | Units | R | Consistent? | Mark |
|---|---|---|---|---|---|---|---|
| G1 | Clock graphic text | ui.rs:4155 | timers.rs clock_text | per type | frame | Until breaks (11); Segment stale (9) | ISSUE |
| G2 | On-screen time | ui.rs:4166 | fmt_duration | HH:MM:SS | frame | — | OK |
| G3 | "Alt+N" | ui.rs:4171 | hard-coded | — | — | wrong on Mac | ISSUE (22) |
| G4 | Next rotation in | ui.rs:4201 | rotation timer | "04:59" | frame | rundown uses "4:59" | ISSUE (28) |
| G5 | Marker name dialog time | ui.rs:4296 | marker at | m:ss | once | same zero as H4 | ISSUE (6) |

### Audio panel

| # | Number | Where | Source | Units | R | Consistent? | Mark |
|---|---|---|---|---|---|---|---|
| A1 | Strip meter peak/RMS | draw_meter ui.rs:5636, st.meters audio.rs:873 | post-FX, pre-fader | dBFS bar | frame | remote/OSC/Settings use raw levels_db | ISSUE (4) |
| A2 | Peak hold | ui.rs:4540 | local | 12 dB/s fall | frame | remote has none | OK |
| A3 | Gain reduction | ui.rs:4419, draw_reduction 5682 | compressor | dB | frame | — | OK |
| A4 | Fader | ui.rs:4425 | gain −40..=12 slider | dB | frame | clamps −60 → −40 | ISSUE (2) |
| A5 | Gain label | ui.rs:4432 | gain | "{:+.1} dB" | frame | Settings rounds to 0.5 | ISSUE (2) |
| A6 | Automix ↓N, ducking ↓N | ui.rs:4465, 4468 | mixer | dB | frame | — | OK |
| A7 | REC MIX / STREAM meters | ui.rs:4475 | bus meters | dBFS | frame | — | OK |
| A8 | LUFS short-term | ui.rs:4483 | loudness | LUFS | frame | — | OK |
| A9 | Integrated "I" | ui.rs:4485 | loudness | LUFS | frame | label unclear when idle | ISSUE (24) |
| A10 | lim / lev | ui.rs:4488, 4492 | limiter/leveller | dB | frame | — | OK |
| A11 | Headphone volume | ui.rs:4511 | monitor gain | % / dB | frame | Settings Apply reverts it | ISSUE (1) |

### Health window (repaints every 500 ms)

| # | Number | Where | Source | Units | Consistent? | Mark |
|---|---|---|---|---|---|---|
| X1 | Drives / recording time left | ui.rs:4619 | describe health.rs:41, pick_rate :55, estimate_rate :108 | GB, h/min | wording garbled; threshold same as header | ISSUE (16) |
| X2 | Backup drive | ui.rs ~4625 | uses main_rate health.rs:323 | min | warns at 30/10 | ISSUE (15) |
| X3 | Source fps | ui.rs:4631 | health.rs:276–283 | "{:.1} fps (set to N)" | 0 after start | ISSUE (3) |
| X4 | Frames / Repeated / Dropped / Waiting | ui.rs:4641–4653 | file writer counters | counts, amber ≥30 | cumulative | ISSUE (17) |
| X5 | Audio resyncs | ui.rs:4665 | audio | count | — | OK |
| X6 | Compositor skips | ui.rs:4670 | compositor | count | — | OK |

### Rundown, guests, sound, production panels

| # | Number | Where | Source | Units | R | Mark |
|---|---|---|---|---|---|---|
| P1 | Segment i/n | panels.rs:100 | rundown status rundown.rs:108 | count | frame | OK |
| P2 | Segment time left (big) | panels.rs:100–115 | status | m:ss | frame | ISSUE (9) |
| P3 | On time / over / under | panels.rs:118 | done list | m:ss | frame | ISSUE (10, 27) |
| P4 | Guest "on" time | panels.rs:172 | fmt_span | m:ss | frame | OK |
| P5 | Guest "ms behind" | panels.rs ~200 | RTT | ms | frame | ISSUE (21) |
| P6 | Guest backup MB | panels.rs:225 | backup file | MB | frame | OK |
| P7 | Pad remaining Ns | panels.rs:331 | pad player | s | frame | OK |
| P8 | Pad F-key corner | panels.rs:331 | index | F1–F12 | static | OK (keys can't be rebound) |
| P9 | TALK "(hold, or B)" | panels.rs:419 | hard-coded | — | — | ISSUE (22) |
| P10 | 2nd-screen clocks | panels.rs:485 | local time | %-I:%M:%S %p | frame | ISSUE (8) |
| P11 | 2nd-screen REC | panels.rs:487 | fmt_span | m:ss | frame | ISSUE (8) |
| P12 | Prompter wpm | production.rs:649 | prompter speed | wpm | frame | OK; matches the prompter page |
| P13 | Prompter position | production.rs:655 | scroll | slider | frame | OK |
| P14 | Comments queue | production.rs:412 | comment list | count | frame | OK |

### Takes, uploads, starting, guide

| # | Number | Where | Source | Mark |
|---|---|---|---|---|
| T1 | Take card "N long · size · files" | takes.rs ~300, fmt_length "1h 02m" | ISSUE (7) |
| T2 | Recent takes size | takes.rs ~233 (recursive walk) | ISSUE (7) |
| T3 | Upload "N% of size" | uploads.rs:557 | ISSUE (25) |
| T4 | Starting "TAKE n" | starting.rs:179 | ISSUE (23) |
| T5 | Starting "Starting N cameras" | starting.rs:183 (cfg sources count) | OK |
| T6 | Guide free space | guide.rs:160 "{:.0} GB" | ISSUE (26) |
| T7 | Guide "Press Record (R)" | guide.rs:193 | ISSUE (22) |

### Settings

| # | Number | Where | Mark |
|---|---|---|---|
| C1 | Input gain slider | settings/mod.rs:1231 | ISSUE (2): rounds to 0.5 dB and marks the page dirty |
| C2 | Input level bar | settings/mod.rs:1188 (levels_db) | ISSUE (4): does not match the strip |
| C3 | Switching rule levels | settings/switching.rs:186–195 | ISSUE (4): indexed by draft position; muted mic shown as talking |
| C4 | "Writes about X/hour" | settings/mod.rs:1407 | ISSUE (13) |
| C5 | Stream quality summary | settings/mod.rs:1804 | ISSUE (20) |
| C6 | Transition ms | settings/switching.rs | ISSUE (1) with the bank |
| C7 | Remote port / PIN, OSC port, MIDI channel | settings/control.rs:41/124/173 | OK |

### Remote page, prompter page, OSC, MIDI, Stream Deck

| # | Number | Where | Source | Mark |
|---|---|---|---|---|
| R1 | REC / LIVE times | remote.html:72–77 from show_state rec_seconds/live_seconds ui.rs:1550/1558 | ISSUE (5): LIVE while connecting remote.html:196 |
| R2 | Saving N% | remote.html:193 | OK |
| R3 | Audio meter | remote.html:278, (level+60)/60; level from levels_db rounded to 0.5 at ui.rs:1599 | ISSUE (4) |
| R4 | Rundown drift | remote.html:234 "01:05" | ISSUE (27) |
| R5 | Problems list | show_state ui.rs:1559 | ISSUE (12, 29) |
| R6 | Refresh | 150 ms publish (ui.rs:1707), 500 ms polling fallback, 700 ms images | ISSUE (18) |
| R7 | Prompter page wpm | prompter.html:28 | OK |
| R8 | OSC rec_time / live_time / level | control/osc.rs (level 0..1 in 2% steps) | Same as R1 and R3 |
| R9 | MIDI fader in | control/mod.rs:242 (0 → −60, then −40…+12) | ISSUE (2) |
| R10 | Stream Deck key text (scene names, REC, timers) | control/keys.rs; gets the same LIVE-while-connecting state | ISSUE (5) |
