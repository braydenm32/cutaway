# 2 — Page forensics (Phase 1)

Why people struggle on each heavily used screen, as numbered fix lists.
Evidence is `file:line` in `src/` (0.9.7) or a screenshot from the walkthroughs.
Fuller notes: `agent-show-operator.md` and `agent-new-stream-phone.md`.
Severity: **H** can ruin a show or a first try; **M** costs time or trust; **L** polish.

## Main window: header

1. **H** Header lays out right-to-left, then left-to-right into the same row, so at 1024 wide "Set up streaming", Screen, Check and Health draw on top of each other (ui.rs:2328, compact.png).
2. **H** The disk / time-left line is truncated first, so on a 13" laptop it shows "…04:40" (ui.rs:2658, air.png). The one number that answers "will this take fit?" is the one that disappears.
3. **H** Hold-to-stop gives no hint before you try; a click swaps the timer for "Hold to stop" for 1.5 s, which hides the timer and invites repeat clicks (ui.rs:2457, 2486).
4. **M** Health (n) and Log (n) are two amber counters for one question ("is anything wrong?"). Health is amber at rest because of a false 0 fps reading (numbers audit #3).
5. **M** The ▾ next to Marker is Recent takes when idle and the marker list while recording (ui.rs:2498, 2578).
6. **L** Three streaming reminders on a record-only show: header link, Stream panel "No destinations yet", STREAM meter (main.png).
7. **L** Key hints are hard-coded and some are wrong, e.g. the guide's "Press Record (R)" when the key is Cmd/Ctrl+R (guide.rs:193).

## Main window: scene bank

1. **M** Scenes past the window width hide behind "+2 ›"; Brayden's 7th scene (PiP) is off screen at 1280 and 1480 (main.png, air.png).
2. **M** "4 Maya" on a multiview tile is source 4; "4 Maya" on a bank button is scene key 4. Same styling, different meaning; they drift apart as soon as scenes are reordered.
3. **L** Seven transition buttons (CUT, MIX, WIPE, PUSH, DIP, STINGER, FTB) sit beside the bank; only Cut/Mix are used in these shows. FTB is drawn dim but one click blacks the show (ui.rs:3749).
4. **L** The ms box stays live for Stinger, where it does nothing (ui.rs:3761).
5. **L** AUTO lights up with no rules and only writes a warning to the log (engine/mod.rs:1752).

## Main window: multiview

1. **H** With Preview off, clicking a tile takes that camera to air with the transition (ui.rs:3628, 3702). It's exactly where you click to look closer at Maya's framing.
2. **L** Split into close-ups and auto-framing follow/shot exist only on right-click (ui.rs:3597, 3649).

## Main window: right column

1. **M** The top panels get "window height − 260 px" (ui.rs:5521), so at 1024×700 Graphics scrolls out of view and at 1280×800 the audio ON buttons are clipped. Nothing shows there's more below. This is the "cramped" feeling.
2. **M** AUDIO can't fold or move, though it looks like the headings that can (ui.rs:4381).
3. **M** PTZ ⌂ Home sits right under Zoom out and snaps an on-air close-up to the whole picture, no undo (production.rs:288).
4. **M** The Graphics panel vanishes when there are no graphics, so a new user never learns titles exist (ui.rs:4117).
5. **L** Each panel has its own "Set up" button; three in a column (panels.rs:54).
6. **L** Panel headings are tiny dim labels that are actually click-and-drag targets (panels.rs:41).

## Edit layout

1. **M** With Preview off you edit the scene on air ("changes show at once"), and Delete sits beside Duplicate with no confirm (ui.rs:3406), while Settings → Scenes → Remove asks first.
2. **L** Undo/Redo buttons only exist here; elsewhere it's keys only (ui.rs:3227).

## Settings window

1. **H** Apply writes back the copy taken when Settings opened, undoing fader, ON/OFF, transition and other desk changes made meanwhile (ui.rs:5173, 5292). Mid-show this looks like levels jumping by themselves.
2. **M** Selected source, scene and graphic reset on every open; Find is cleared; the page is remembered only for the session (settings/mod.rs:411–448, ui.rs:268).
3. **M** It covers the program monitor at 1280×800 while you frame a camera.
4. **M** Names are edited in two places that behave differently: the guide applies at once, Settings needs Apply (settings/mod.rs:763).
5. **M** "If it drops while on air" is at the bottom of each source page, off by default.
6. **L** Transition style and length live both in the bank and Settings → Switching; lip-sync is in both Check and Audio inputs.

## After the show (take card, recent takes)

1. **H** "Open in Resolve" copies the path and shows instructions; the first time it opens Settings instead (takes.rs:180–200). 7–9 steps from Stop to a timeline.
2. **M** Markers can't be seen in Cutaway once the take stops; the card and Recent takes show no marker count (ui.rs:2498, takes.rs:329).
3. **L** The card hides while Settings is open and dismissing it is permanent; the same action is "Open in Resolve" on the card and "Resolve" in the menu.
4. **L** The card and Recent takes report different sizes for the same take (numbers audit #7).

## First-show guide (new users)

1. **H** It names people "for the name titles" but never creates a title; a fresh show has no graphics and the panel is hidden (guide.rs:108, ui.rs:4117).
2. **H** First run picks the first audio input (on a laptop, the built-in mic) and the mic step goes green when any input hears sound, so a USB mic is silently unused (main.rs:377, guide.rs:37).
3. **H** With no camera found, setup adds test patterns and the guide ticks "cameras with a picture", so the privacy hint never shows (main.rs:372, guide.rs:94).
4. **M** Settings opens over the guide that "stays open"; progress isn't saved between launches.
5. **M** No guided way to make a close-up per person from one webcam (Split… is in a menu).

## Phone remote

1. **H** *Measured:* buttons rebuild about every 150 ms (18 rebuilds in 3 s), so scene taps get lost: 3 of 8 land at 100 ms, 0 of 6 at 350 ms (remote.html:211).
2. **H** *Measured:* it shows a healthy LIVE while both destinations are failing; it gets no per-destination state.
3. **M** One tap with no confirm: FTB, mic mute, GO LIVE. RECORD/GO LIVE can open the pre-check on the desk, which the phone user can't see (ui.rs:4680).
4. **M** Comments can't be read on the phone, only pushed on air blind. No faders.
5. **L** Recording problems never reach the remote (numbers audit #12).

## Health and Log

1. **M** Two windows for one question; Health opens over REC and Marker.
2. **M** Repeated/Dropped turn amber on whole-session totals, so one early blip leaves it amber all show (ui.rs:4641).
3. **M** No stream section: no dropped frames or bitrate history for a stream anywhere.

## Things that look clickable and aren't, or the reverse

- AUDIO heading (inert) vs the other headings (fold + drag).
- FTB looks disabled, is live.
- Multiview tiles look like monitors, are take buttons.
- Right-click only: PTZ preset save, replay slow-mo/angle, split, auto-framing, playlist, timers. Shift+M only: named marker. Backspace only: back to last scene. Clock click only: 12/24 h.
