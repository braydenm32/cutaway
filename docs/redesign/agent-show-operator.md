# Show operator walkthrough: Brayden, 13" MacBook, record-only show

Persona: owner and main operator. Records (never streams). 3 cameras (Wide, Host cam, 4K Couch), with two virtual PTZ close-ups (Maya, Sam) cut from Couch, plus Screen. Opens takes in DaVinci Resolve.
Evidence: Cutaway 0.9.7 code at `/home/claude/cutaway` and the screenshots in `/home/claude/shots/`. New shots: `a-drop.png`, `a-set-maya.png`, `a-find.png` (all 1280x800). `a-takecard.png` was meant to show the Take saved card, but the `record off` command fired before recording had started, so it shows the main window still recording at 1280x800. Everything below about the Take saved card comes from the code (`src/ui/takes.rs`). `takes.png` is the same Stop confirm as `stop.png`, not a takes view.

Window sizes: 1480x900 = `main.png`, 1280x800 = `air.png` / `a-*.png`, 1024x700 = `compact.png`.

---

## Walkthrough log (clicks, keys and hesitations)

### 1. Before the show
| Step | Path taken | Clicks/keys | Hesitation |
|---|---|---|---|
| Open the show | "Tuesday Pod ▾" → pick the show → "Switch" confirm (ui.rs:3996-4002) | 3 | The show picker is a small dim pill beside the logo and doesn't look like the main way to choose a show. |
| Check Maya/Sam framing | PTZ panel: Maya/Sam toggle, joystick, Zoom (main.png). The panel has no presets yet, only a bare "+". For an exact box: Settings → Inputs → 4 Maya → drag the red box → Apply, then the same for Sam (a-set-maya.png) | 2 for a look; about 8 plus drags for exact framing | Framing happens in two places that look different: the joystick in the panel and the drag box in Settings. My instinct was to click the Maya tile in the multiview to look closer, but **that click puts Maya on air** (ui.rs:3628, 3658). "+" saves "Preset N" right away, with no name (production.rs:308). Saving over a preset is right-click only (production.rs:297). |
| Names/roles | "?" → Your first show → step 2 table (guide.png), 6 rows × 2 fields. Or Settings → Video sources → pick each source → Name title → Apply (a-set-maya.png) | About 7 with the guide, about 11 in Settings | "?" opens a first-run guide, not help. The guide updates live, while Settings needs Apply, so the same fields behave differently in the two places. |
| Six scenes in order | Settings → Show → Scenes (set-scenes.png): 9 scenes exist (Wide, Host cam, Couch, Maya, Sam, Screen, Side by side, PiP, Grid). Select Side by side, ↑ once to put it above Screen; select Couch → Remove → confirm; Grid the same; OK | About 11 | ↑↓ moves one place per click. Settings calls it "Remove" and asks first. Edit layout calls it "Delete" and doesn't ask (ui.rs:3406). |
| Pre-show Check | Header "Check" → read → Close (check.png) | 2 | It isn't automatic: "before recording" is off by default (config.rs:1513, set-general.png). The rows are text only, so you can't click the ⚠ Recording drive row to fix it. |

### 2. Live
| Step | Path | Clicks/keys | Hesitation |
|---|---|---|---|
| Record | RECORD pill (or ⌘R). No check runs first by default. | 1 | none |
| wide → host → Maya → Sam → SbS → screen → PiP | Number keys 1,2,4,5,7,6,8 (before reordering) | 7 keys | At 1280 the bank shows scenes 1–6 and "+3 ›" (air.png). At 1480 it shows 1–7 and "+2 ›" (main.png). **PiP (key 8) is cut off at both sizes**, so clicking it takes an extra scroll click. The multiview labels "4 Maya" (source number) and the bank's "4 Maya" (scene key) look identical but mean different things. |
| Name title | Names graphic set to follow whoever's on air for 6 s (show.toml; a-drop.png shows "Brayden / Host" by itself) | 0 (Alt+1 by hand) | No ✎ for the follow title, so it's unclear how to fix a typo live (ui.rs:4180). |
| Named marker | Shift+M → type → Enter (ui.rs:4289-4324). With the mouse: ◆ Marker → ▾ → click the name field → type → click away | 1 chord + typing, or 3 clicks + typing | The ▾ beside Marker is in the same place as the ▾ "Recent takes" shown when idle (ui.rs:2498 vs 2578): same glyph, different function. |
| Save a clip | K or ✂ Clip | 1 | none |
| Camera drops | A red toast "Wide dropped while on air: no signal" and ⚠ on scene 1. It cut to Host cam only because DROP_TO was set (a-drop.png). | 0 if set up, else 1 key | The per-camera "If it drops while on air" sits at the bottom of Settings → Video sources (set-sources.png). It's off ("Don't switch") by default, so most shows have no safety net. |
| Glance at time/disk/levels | REC timer, clock, disk line, meters | 0 clicks, 3 places to look | **At 1280x800 the disk line truncates to "…04:40"** (air.png, a-drop.png, a-set-maya.png). Free space and time left are only on hover (ui.rs:2658). The audio column's ON buttons are cut off at 800 px tall (a-drop.png). |

### 3. After
| Step | Path | Clicks | Hesitation |
|---|---|---|---|
| Stop | Hold REC for 1 s (ui.rs:2455-2490). A quick click just swaps the timer for "Hold to stop" for 1.5 s (ui.rs:2486). ⌘R → "Stop recording?" modal (stop.png). | 1 long press, or 2 | **Rage-click risk**: a click does nothing visible except replace the timer text, so the natural reaction is to click again, harder. The modal puts the red "Stop recording" on the left, where Mac users expect Cancel (ui.rs:4014). |
| Find the take | The "Take saved" card under REC (takes.rs:289-375). It doesn't draw while Settings is open (ui.rs:2582-2583), and once dismissed with ✕ it's gone. Otherwise ▾ beside RECORD → Recent takes (ui.rs:2578). | 0–1 | The ▾ for Recent takes only exists when idle. During recording that spot is the marker ▾. |
| Open in Resolve | "Open in Resolve". The first time it opens Settings → Recording → For editing with a toast (takes.rs:193-201), then Install the Resolve script → OK. After that the button **only copies the folder path** (takes.rs:186). You then switch to Resolve → Workspace → Scripts → Cutaway → pick the take. | About 7–9 | The label promises "Open", but it doesn't launch Resolve. The Recent takes menu calls the same action "Resolve" (takes.rs:257). |
| Find markers | Only inside Resolve's timeline | — | After the stop, Cutaway can't show the take's markers: the ▾ list exists only while recording ("Names can be changed until the take stops", markers.png), and neither the card nor Recent takes shows a marker count. |

### 4. Between shows
| Step | Path | Clicks | Hesitation |
|---|---|---|---|
| Change a scene layout | E / Edit layout → drag frames → Done (edit.png) | 2 + drags | With Preview off, it **edits the on-air scene live** ("Picture in picture on air: changes show at once", edit.png). The bottom toggle reads "Editing" while the exit is "Done" at the top: two exits in two places. |
| Rename a person | Settings (opens on the last page, Sources) → pick the source → Name title → OK | 3–4 | Each time Settings opens, the selected source goes back to "1 Wide" (settings/mod.rs:411). |
| Move a panel | Drag the dim "▾ GRAPHICS" heading onto "▾ PTZ" (panels.rs:38-73) | 1 drag | The only way to learn this is hovering the tiny heading. AUDIO looks like a heading but can't be folded or moved (ui.rs:4381). |
| Find a setting | Settings → type "resolve" → click "DaVinci Resolve — Recording" (a-find.png) | 2 + typing | Works well. The box clears after a jump and isn't kept when Settings is closed and reopened (settings/mod.rs:414, ~1660). |

---

## (a) Screens and panels touched

Visits are per show. "Fold" means visible without scrolling at 1280x800 / 1024x700.

| Screen / panel | Visits | Time | Rage-click risk | Dead clicks (or inverse) | Fold 1280x800 | Fold 1024x700 | Frustration 0–5 |
|---|---|---|---|---|---|---|---|
| Header: REC pill / timer | 1 start + about 30 glances + 1 stop | whole show | **High**: a quick click only shows "Hold to stop" | — | yes | yes | 4 |
| Header: status line (disk / last take) + clock | about 10 glances | 1 s each | — | Truncated "…" hides the data, which only shows on hover | **truncated** (air.png) | the header itself overlaps (compact.png) | 4 |
| Header: Marker / ▾ / Clip / Replay | 3–10 | seconds | low | ▾ means two different things; Replay looks dim | yes | yes | 2 |
| Header: Screen / Check / Health / Log / Settings / ? | 2–6 | — | Health count wanders: 1, 4, 5, 6 between shots | "?" is a guide, not help | yes | **text overlaps "Set up streaming"** (compact.png) | 3 |
| Program monitor | constant | — | — | — | yes | yes | 1 |
| Multiview | constant glance, 0 intended clicks | — | — | **Inverse dead click**: looks like a monitor, but a click goes to air (ui.rs:3628) | yes | yes, smaller | 4 |
| Scene bank | 20–40 (mostly keys) | — | "+N ›" scroll | Number style matches the multiview | 6 of 9 shown, PiP hidden | 4 of 9 ("+5 ›") | 3 |
| Transition row (CUT/MIX…/FTB/ms/Preview/AUTO/Edit layout) | 0–2 | — | **AUTO** lights up with no rules and does nothing (engine/mod.rs:1752-1756) | FTB looks disabled but is live (ui.rs:3749) | yes | yes | 2 |
| PTZ panel | 2–4 (pre-show) | 2–5 min | low | "+" saves an unnamed preset; ⌂ Home snaps the close-up out | yes | yes | 3 |
| Graphics panel (Names, Logo, ✎) | 0–3 | seconds | — | ✎ unlabeled, next to Logo only | yes | **scrolled out of view** (compact.png; ui.rs:5521) | 3 |
| Stream panel | 0 (record-only) | — | — | "No destinations yet." | takes space | takes space | 2 |
| Audio column (meters, faders, ON, LUFS, STREAM meter) | constant glance | — | — | AUDIO heading isn't foldable; a STREAM meter shows with no stream | **ON buttons clipped** (a-drop.png) | faders clipped (compact.png) | 3 |
| Marker list popup | 1–2 | 30 s | — | ✕ deletes at once, no undo | popup | popup | 2 |
| Named-marker dialog | 3–8 | 5 s | — | — | modal | modal | 1 |
| Stop confirm (⌘R) | 0–1 | 2 s | — | — | yes | yes | 2 |
| Take saved card | 1 | 30 s | — | Filename is a label, not a link | under REC | under REC | 2 |
| Recent takes menu | 0–1 | 20 s | — | — | popup | popup | 2 |
| Resolve hand-off | 1 | 1–3 min | "Open in Resolve" clicked twice, expecting Resolve to launch | Label overpromises | — | — | 4 |
| Pre-show Check dialog | 1 | 30 s | — | Rows can't be clicked to fix | yes | probably clipped (not shot) | 2 |
| Health window | 0–2 | 30 s | — | — | covers the REC/Marker header (health.png) | — | 3 |
| Log panel | 0–1 | 20 s | — | — | squeezes the audio column, faders cut (log.png) | — | 2 |
| First-show guide | 0–1 | 1–2 min | — | — | covers the program (guide.png) | — | 2 |
| Settings → Inputs → Video sources | 1–2 | 3–5 min | — | Tooltip covers the Name field (a-set-maya.png) | **covers the whole program** (a-set-maya.png) | not shot | 3 |
| Settings → Show → Scenes | 0–1 | 2 min | ↑/↓ one step per click | — | yes | not shot | 3 |
| Settings → Output → Recording | 0–1 | 1 min | — | "For editing" is folded away | yes | not shot | 3 |
| Settings Find | 0–1 | 20 s | — | — | yes | not shot | 1 |
| Edit layout | 0–1 between shows | 3–5 min | — | — | yes | not shot | 3 |
| Show picker | 1 | 5 s | — | — | yes | yes | 1 |
| Drop toast + ⚠ on scene | 0–1 | 5 s | — | — | yes | yes | 2 |

## (b) Screens ranked by importance to Brayden
1. Header REC pill and timer (start, hold-to-stop, recording proof)
2. Scene bank and number keys
3. Program monitor
4. Audio meters and ON buttons
5. Multiview (as a monitor; for him it is never a control)
6. Status line: disk and time left, clock
7. Marker / Clip
8. Graphics: Names
9. Take saved card → Open in Resolve
10. PTZ panel (pre-show framing only)
11. Pre-show Check
12. Settings → Video sources (names, framing, drop behaviour)
13. Edit layout / Settings → Scenes (between shows)
14. Health / Log (only when something's amber)
15. Guide, Find, show picker
16. Stream panel, STREAM meter, Set up streaming, Replay, Screen, AUTO, FTB (irrelevant to a record-only show)

## (c) Friction findings (severity: H / M / L)

1. **H: Multiview tiles put cameras on air.** With Preview off, clicking a tile takes that source full screen with the transition (ui.rs:3628, tooltip ui.rs:3658, show_source ui.rs:3702-3718). For a show of close-ups this is exactly where you click to "look closer at Maya", and the tiles look like passive monitors.
2. **H: The disk / time-left line is truncated at 1280 wide.** Air.png, a-drop.png and a-set-maya.png all show "…04:4x". The text is `.truncate()`d into what's left after the header buttons (ui.rs:2658), so the most important "will this take fit" number disappears on a 13" laptop.
3. **H: The header overlaps at 1024x700.** "Set up streaming", "Screen", "Check" and "Health (n)" draw on top of each other (compact.png). The header lays out right-to-left first and then left-to-right into the same row (ui.rs:2328-2335), and nothing wraps or collapses.
4. **H: Hold-to-stop has no hint before you try.** A quick click only swaps the timer for "Hold to stop" for 1.5 s (ui.rs:2457, 2486). Before that, the pill looks like an ordinary button. That invites repeated clicks, and while the hint shows, the timer is hidden.
5. **H: "Open in Resolve" doesn't open Resolve.** It copies the path and shows instructions (takes.rs:180-200). The first time, it opens Settings instead (takes.rs:193-201). There are 7–9 steps from Stop to the timeline. The same action is labelled "Open in Resolve" on the card (takes.rs:356) and "Resolve" in Recent takes (takes.rs:257).
6. **M: Virtual PTZ ⌂ Home sits under Zoom out and snaps the on-air close-up to the default (full) shot** (production.rs:288; engine/ptz.rs:349-352). No confirm, no undo, one click.
7. **M: Part of the right column is out of reach at small sizes.** At 1024x700 the Graphics panel is pushed out of the scroll area (compact.png), because the top panels get `available − 260 px` (ui.rs:5521). At 1280x800 the audio ON buttons are clipped (a-drop.png). Nothing shows that more is below. This is the "cramped right column".
8. **M: The picture-in-picture scene is hidden behind "+2 ›" / "+3 ›" at 1480 and 1280 wide** (main.png, air.png). Brayden's 7th used scene is out of sight, and Couch and Grid (which he doesn't use) take bank slots.
9. **M: Markers vanish from Cutaway when the take stops.** The ▾ list only exists in the Recording branch (ui.rs:2498, markers.png: "until the take stops"). The take card and Recent takes show no marker count (takes.rs:329, 228-250).
10. **M: The same ▾ glyph does two jobs in the same spot.** While idle it's Recent takes (ui.rs:2578). While recording it's the marker list (ui.rs:2498).
11. **M: Disk free is printed in four places, with slightly different values.** Header "29.58 GB free · 30 min left", Health "29.59 GB … about 30 min", Check "29.60 GB … about 30 min", Settings → Recording "room for about 30 min" (main.png, health.png, check.png, set-recording.png).
12. **M: "4 Maya" is printed twice and means different things.** Multiview tile = source 4, bank button = scene key 4 (main.png). They match today only by coincidence, and stop matching as soon as scenes are reordered.
13. **M: Scene controls are duplicated and behave differently.** Edit layout has "+ New scene / Duplicate / **Delete**" with no confirm (ui.rs:3406). Settings → Scenes has "+ New scene / Duplicate / **Remove**", which asks first (set-scenes.png; README "Put scenes in order").
14. **M: Names are edited in two places, and they apply differently.** The guide's people table applies live (guide.png). Settings → Video sources → Name title needs Apply/OK (a-set-maya.png, settings/mod.rs:763-771).
15. **M: The auto-cut on a camera drop is buried and off by default.** "If it drops while on air" is at the bottom of each source's page and set to "Don't switch" (set-sources.png). The drop alert itself works well (a-drop.png: toast, ⚠ on scene, title follows).
16. **M: Edit layout with Preview off edits the on-air scene** ("changes show at once", edit.png; README Edit layout row). Delete sits beside Duplicate in the same bar.
17. **M: Settings forgets context.** The selected source, scene and graphic go back to the first item on every open (settings/mod.rs:411, 448). Find is cleared (settings/mod.rs:414). The page is remembered only for the session (ui.rs:268, 475, 5177), and egui memory (scroll, folds inside pages) isn't persisted (ui.rs:5460).
18. **M: Settings covers the program monitor at 1280x800** (a-set-maya.png, set-*.png). Framing Maya in Settings hides the program you'd check her against, although the Settings preview does show "Now".
19. **L: AUTO lights up with no rules and does nothing** except add a warning to the Log (engine/mod.rs:1752-1756; ui.rs:3777).
20. **L: FTB is drawn dim and looks disabled, but one click fades the show to black** (ui.rs:3749; main.png). It sits right next to STINGER.
21. **L: Marker ✕ deletes at once, with no undo**, and sits beside the editable name (ui.rs:4361; markers.png).
22. **L: Things that only exist on right-click or hover:** saving over a PTZ preset (production.rs:297), replay slow-mo and angle (ui.rs:2532-2560), split into close-ups (ui.rs ~3650; tooltip in air.png), folding and moving panels (panels.rs:44), full disk text (ui.rs:2658).
23. **L: Panel headings are tiny dim labels that are actually click and drag targets** (panels.rs:41-48). "AUDIO" looks the same but is inert (ui.rs:4381), so a click on it does nothing.
24. **L: Streaming clutter for a record-only show.** "Set up streaming" in the header (ui.rs:2635), a Stream panel saying "No destinations yet." with Set up, and a STREAM meter (main.png): three reminders of a feature he doesn't use.
25. **L: Each panel heading has its own "Set up" button** (panels.rs:54; main.png: 3 of them), the same word repeated down the column.
26. **L: The Stop confirm puts the red action on the left and Cancel on the right** (ui.rs:4014-4031; stop.png), the opposite of macOS order.
27. **L: The Take saved card is hidden while Settings is open, and dismissing it is permanent** (ui.rs:2582-2583; takes.rs:376-378). Recent takes ▾ is the only way back to it.
28. **L: Transition style and length appear both in the bottom bar and in Settings → Switching** (main.png, set-switching.png). The pre-show lip-sync tool appears in both Check and Audio inputs (check.png, set-audio.png).
29. **L: The pre-show Check doesn't run by itself** ("before recording" is off, config.rs:1513), and its ⚠ rows can't be clicked through to a fix (check.png).
30. **L: Health opens over the REC pill and Marker** (health.png), and its count moves between 1, 4, 5 and 6 with no clear "fixed" state.

## (d) One glance away vs under "More"

**One glance away (always visible, never truncated, at 1280x800 and 1024x700):**
- REC state and timer, with a visible "hold to stop" affordance
- Disk time left (one number, one place) and the clock
- Program monitor and multiview (as monitors; clicking should preview, not go on air)
- The scenes he actually uses (7), all visible, numbered by key
- Audio meters plus the ON/OFF state per mic
- Marker (with count) and Clip
- Names title on/off state
- One combined "problems" light (Health + Log) that goes amber
- A dropped-camera alert (as now)

**Could live under "More" or fold away after pre-show:**
- Stream panel, STREAM meter and "Set up streaming" (hide them while no destinations exist)
- Replay, Screen, AUTO, CC, stinger, wipe, push and dip styles (keep CUT/MIX and the length)
- FTB (keep it, but away from the transition styles)
- PTZ joystick and zoom (pre-show only; fold during the show)
- Edit layout, Check, the "?" guide, the per-panel "Set up" buttons
- Health detail and the Log list (behind the single light)
- Recent takes (it fits better in a post-show "Takes" view that also lists markers)
