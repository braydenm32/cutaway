# Cutaway 0.9.7: walkthroughs for a new user, a streamer and a phone operator

There are no analytics, so these walkthroughs are the usage evidence for the calm redesign. Three personas each walked the app from start to finish. Everything below comes from code I read or screenshots I took. Visits and times are estimates for one typical show.

**New screenshots, in /home/claude/shots/, prefix `b-`:**

| File | What it shows |
|---|---|
| `b-p1-firstrun-1280.png` | A real first run with no config file, at 1280x800 |
| `b-p1-guide-1280.png`, `b-p1-guide-1024.png` | The first-show guide with 1 webcam and 1 USB mic |
| `b-p1-main-1280.png` | The same show with the guide closed: what the empty states look like |
| `b-p1-guide-settings-1280.png` | A guide button opens Settings, and Settings buries the guide |
| `b-p2-idle-1480.png` | 2 cameras + screen, 4 sound pads, comments and Names, before going live |
| `b-p2-live-1280.png` | GOING LIVE, with both destinations retrying |
| `b-p2-live1of2-1480.png` | LIVE, with 1 of 2 destinations up (a local ffmpeg RTMP sink) |
| `b-p3-live-1024.png` | LIVE in a 1024x700 window |
| `b-remote-pin-390.png` | The phone remote's PIN screen |
| `b-remote-390-top.png`, `b-remote-390-full.png` | The phone remote at 390x844, first screen and whole page |
| `b-remote-360-top.png`, `b-remote-360-full.png` | The phone remote at 360x740 |
| `b-remote-390-live.png` | The phone during a broken stream: it shows a healthy pink LIVE |
| `b-remote-390-ftb.png` | The phone after one tap on FTB |

**Phone tests:** I drove the remote page with Playwright against the real app (the remote on, PIN 4821), with recording running. "Measured" in this report means numbers from that script.

**Scales used below:**
- **Above the fold (ATF):** ✔ fully visible without scrolling, ◐ partly visible or clipped, ✘ not visible. Given for 1280x800 / 1024x700 / phone 390x844.
- **Frustration:** 0 (none) to 5 (worst).
- **Severity:** S1 means it can lose a show or broadcast, or block a new user. S2 is serious friction. S3 is polish.

---

## P1: a first-time buyer on a Windows laptop

This person has never used OBS. They have 1 webcam and 1 USB mic, and want to record a 2-person interview with a name title.

### (a) Screens touched

| Screen / panel / dialog | Visits per show | Time | Rage-click risk | Dead clicks | ATF 1280 / 1024 / phone | Frustration |
|---|---|---|---|---|---|---|
| Windows "protected your PC" (README:604) | 1, first launch only | 30 s | Med: "Run anyway" sits under "More info" | — | n/a | 3 |
| First-show guide | 1 now, 0–2 later | 5–15 min | Low | The step circles (○ ✔ –) look tickable but aren't | ◐ its own list scrolls; it covers the bank and mixer / ◐ / — | 3 |
| Settings → Video sources | 1–2 | 3–5 min | Low | Split… is hidden in a menu | ◐ form scrolls / ◐ / — | 2 |
| Settings → Audio inputs (mic device) | 0–1, should be 1 | 1–2 min | Low | — | ✔ / ◐ / — | 3, because the guide says the mic is fine (F2) |
| Settings → Look → Graphics (name title) | 1, only if they find it | 3 min | Low | — | ◐ / ◐ / — | 4, because nothing leads here (F3) |
| Main window, empty states | throughout | — | — | "…" in the header (F8) | ✔ / ◐ / — | 2 |
| Edit layout (from the guide) | 1 | 1–3 min | Med | — | ✔ / ◐ / — | 2 |
| Pre-show check | 1 | 20 s | Low | — | ✔ / ✔ / — | 1 |
| REC button (hold to stop) | 2–4 | — | **High**: a click only says "Hold to stop" | Yes, a single click doesn't stop | ✔ / ✔ / ✔ | 3 |
| Take card and Recent takes ▾ | 1 | 30 s | Low | ▾ only exists while not recording | ✔ / ✔ / — | 1 |

### (b) Friction findings

1. **S1: The guide never creates a name title, so the persona's main goal silently fails.**
   - Step 2, "Name your people", promises "name titles that follow whoever is on air" (guide.rs:108-110). It only fills in each source's person and role.
   - A fresh config has `graphics = []` (seen in `b-p1-firstrun-1280.toml`), so nothing ever appears on the picture.
   - The only way to make the title is Look → Graphics → "+ Names of whoever's on air (recommended)" (settings/graphics.rs:47). No guide step links there.
   - With no graphics, the GRAPHICS panel disappears completely, with no empty state (ui.rs:4117-4121; see `b-p1-main-1280.png`). The user gets no hint that titles exist.
2. **S1: The microphone step goes green for the wrong mic.**
   - First-run setup picks the *first* audio device (main.rs:377-379). On a Windows laptop that is usually the built-in array mic, not the USB mic.
   - Step 3 counts as done as soon as *any* input passes -60 dB (guide.rs:37-38). The laptop mic hears the user too, so the guide shows ✔ "Cutaway can hear you" while the USB mic sits unused.
   - Nothing on the step names the device being used.
3. **S1: With no camera detected, the guide says the cameras are fine.**
   - If no camera is found (privacy setting off, which is common on Windows 11), setup makes two test-pattern sources called "Test 1" and "Test 2" (main.rs:372-375).
   - The guide counts them as real cameras and ticks Cameras ✔ "2 cameras with a picture" (guide.rs:44, 94-98; `b-p1-firstrun-1280.png`).
   - The camera-privacy hint (guide.rs:99) never shows, because the test sources are "alive". The persona sees colour bars and a green tick.
4. **S2: A 2-person interview on 1 webcam has no guided path.**
   - The guide says "Set a close-up for each person if two share a camera" (guide.rs:109) but gives no button.
   - The feature is Split… → "Into 2 close-ups", hidden in a menu on Video sources (settings/sources.rs:157-160).
   - With 1 source the scene bank holds a single button (`b-p1-main-1280.png`). The multiview repeats the program, and CUT, MIX, WIPE, PUSH, DIP and STINGER do nothing useful.
5. **S2: Settings covers the guide that "stays open while you work".**
   - The guide claims it stays open (guide.rs:65).
   - Every step button opens Settings, which is drawn on top and hides everything but the guide's "Done" row (`b-p1-guide-settings-1280.png`).
   - At 1280x800 the guide itself covers the scene bank, the mixer and the meters it tells you to watch (`b-p1-guide-1280.png`). Step 3 says to watch "the meters at the bottom", but they are on the right.
6. **S2: Guide progress is lost on close and reopen.**
   - The ticks for heard, checked, scenes and recorded are memory only (ui.rs:262-274, 472-478).
   - The guide opens by itself only on the very first run (ui.rs:151, main.rs:234). If the user quits halfway, it never comes back on its own. Its only re-entry is "?" at the top right.
   - The ticks it did keep (names, folder) are mixed with ones that reset, such as "Run the check".
7. **S2: Stop-recording needs a hold, so a first-timer will rage-click.** A single click on REC only says "Hold to stop" (README:743). Cmd/Ctrl+R asks "Stop recording?" instead (`stop.png`). Two different ways to stop, and neither is discoverable before the first try.
8. **S3: The header loses information.** At 1280 the status line (disk space, path of the last take) shrinks to "…" (`b-p1-main-1280.png`). At 1024, "Set up streaming", Screen, Check, Health and the clock draw on top of each other (`b-p1-guide-1024.png`, `compact.png`).
9. **S3: The guide's wording is fine but the screens around it are busy.** A first-timer sees STINGER, FTB, AUTO, Replay, Clip and Marker before ever recording. They can't break anything there, but every one of them is noise.
10. **S3: A new user can't go wrong in the destructive places.**
    - Settings asks before discarding changes (settings/mod.rs:800-824).
    - Deleting a scene asks first.
    - Quitting mid-take asks (ui.rs:5466-5469).
    - The guide's rename box reopens the camera only when you leave the field (guide.rs:115-121).

---

## P2: a streamer

This person has 2 cameras and a screen, goes live to YouTube and Twitch at once, reads chat, uses sound pads and the Stream panel, and worries about dropped frames and bitrate.

### (a) Screens touched

| Screen / panel / dialog | Visits per show | Time | Rage-click risk | Dead clicks | ATF 1280 / 1024 / phone | Frustration |
|---|---|---|---|---|---|---|
| Settings → Output → Streaming | 1 to set up, then 0–1 | 5–10 min the first time | Low | "+ Add" is a combo box, not a button | ✔ / ◐ / — | 2 |
| Settings → Show → Viewer comments | 1, plus every show for YouTube | 5–20 min the first time | Low | — | ✔ / ✔ / — | 4 (F3) |
| Settings → Sound pads | 1 | 5 min | Low | — | ◐ / ◐ / — | 2 |
| Header GO LIVE / GOING LIVE / LIVE pill | 2 to 4 | — | Med | Status text cut off (F5) | ✔ / ◐ the pill covers Check, Health and the clock / ✔ | 3 |
| STREAM panel | 30+ glances | ongoing | Low | Hover-only details | ✔ / ✔ / ✘ not on the phone | 3 |
| COMMENTS panel | constant | ongoing | **High**: one click puts a comment on air | — | ✔ / ✔ / ◐ the phone has no text | 3 |
| SOUND pads | 10–40 | ongoing | Med: fires on press, even on a drag | Names cut to 14 characters | ✔ at 1480 / ✘ under the fold at 1280 and 1024 / ✔ when scrolled | 3 |
| AUDIO strip (faders, ON) | 5–20 | ongoing | Med | ON buttons cut off | ◐ / ◐ / ◐ the phone has only mute | 4 (F2) |
| Health | 2–5 when worried | 30 s | Low | Nothing about the stream | ◐ the window covers the header's REC / ◐ / — | 4 (F1) |
| Log | 1–3 | 30 s | Low | — | ✔ / ✔ / — | 1 |
| End-stream confirm | 1 | 3 s | Low | — | ✔ / ✔ / hold 1 s on the phone | 1 |
| Scene bank, multiview | 100+ | — | Low | — | ✔ / ◐ +N › / ✔ | 1 |

### (b) Friction findings

1. **S1: No dropped-frame or bitrate history for the stream anywhere.**
   - A destination's state carries only state, kbps, message, attempts and a `congested` flag (engine/streamer.rs:47-56). There is no frame counter.
   - The Stream panel shows "⚠ connection too slow — dropping frames" with no count, and Mbps only while Live (ui.rs:4085-4106).
   - Health has no stream section at all (ui.rs:4580-4675). Its "Dropped" column means "extra frames from a fast source" in the *recording* (ui.rs:4657). A streamer will misread it.
   - There's no target-versus-actual figure either. In `b-p2-live1of2-1480.png` YouTube shows 1.9 Mbps against a 6.2 Mbps setting, with no context.
2. **S1: When the right-hand column fills up, the mixer's ON buttons drop out of view.**
   - The top panels may take everything but 260 px (ui.rs:5521).
   - With Stream, Comments, Graphics and Sound open, the ON buttons and dB readouts are cut off at 1480x900, 1280x800 and 1024x700 (`b-p2-idle-1480.png`, `b-p2-live-1280.png`, `b-p3-live-1024.png`).
   - At 1280 the SOUND pads also slide under the fold. Only the heading peeks out, overlapped by "AUDIO" (`b-p2-live-1280.png`).
3. **S2: Comments setup duplicates what streaming already knows, and costs YouTube users every show.**
   - Viewer comments wants a Twitch channel, a YouTube stream URL and a Google Cloud API key, all typed in separately (settings/comments.rs:10-17). None of it comes from the YouTube or Twitch destination already set up.
   - The YouTube field is a stream address, which changes each broadcast unless they use /live/.
   - Errors show as raw OS text: "failed to lookup address information: Name or service not known" (`b-p2-live-1280.png`).
4. **S2: One click on a chat line puts it on air.**
   - Clicking a comment shows it on screen immediately, with no preview step (ui/production.rs:440-447). Only the hover text says so.
   - The list is a 170 px window that sticks to the bottom (production.rs:427). A click aimed at a comment that is scrolling past lands on another one.
   - Dangerous with trolls.
5. **S2: The live status line is cut off exactly when it matters.**
   - At 1480 it reads "1 of 2 destinations live — …" (`b-p2-live1of2-1480.png`). The "— see Stream panel" part (ui.rs:2630) is truncated.
   - At 1280 and 1024 the GOING LIVE / LIVE pill covers the status line and clock completely. At 1024 it also covers Screen, Check and Health (`b-p2-live-1280.png`, `b-p3-live-1024.png`).
6. **S2: Settings changes made mid-show can silently undo live moves.**
   - Settings works on a snapshot draft (settings/mod.rs:548). On Apply, only panel order and folding are carried over from the live config (ui.rs:5291-5296).
   - Main-window faders write straight to the live config (ui.rs:4430, 4521-4526).
   - So: open Settings to fix a stream key, ride a fader meanwhile, press OK, and the fader jumps back. Names typed in the guide while Settings is open, and the clock's 12/24-hour setting, are lost the same way.
   - This is from reading the code; it is not reproduced on screen.
7. **S2: Bitrate is one setting for every destination.** Twitch and YouTube get one encode at one bitrate (settings/mod.rs:1675, "encoded once"). The tool says to stay under 70% of your upload speed (settings/mod.rs:1834) but has no way to measure it.
8. **S3: Pad details.**
   - Names are cut to 14 characters with no ellipsis: "Drumroll pleas" (panels.rs:328; `b-p2-idle-1480.png`).
   - Pads fire on pointer-down using click-and-drag (panels.rs:317, 339-343). A drag over a pad plays it.
   - The Stop button only exists while something is playing (panels.rs:352).
9. **S3: A panel edge line is drawn across the scene bank.** The right column's border runs down through the bank buttons (`main.png` at x≈1110, `b-p2-live-1280.png` at x≈900).
10. **S3: The Health window opens on top of the header,** covering the logo, show picker and REC (`health.png`).

---

## P3: the same operator, with a phone in one hand and a small desktop window

### (a) Screens touched

| Screen / control | Visits per show | Time | Rage-click risk | Dead clicks | ATF 1280 / 1024 / phone 390 (y = position on the page) | Frustration |
|---|---|---|---|---|---|---|
| Remote PIN screen | 1 per device | 20 s | Low | — | phone ✔ | 1 |
| Remote header: REC, GO LIVE, Marker, Clip, Replay, FTB | 10–30 | — | **FTB: one tap, no confirm** | REC and LIVE taps while on only say "Hold to…" | phone ✔, but at the top, outside thumb reach | 3 |
| Remote program picture | constant | — | — | — | phone ✔, at y 426–835 | 1 |
| Remote scene grid | 100+ | — | **Very high** (F1) | Yes (F1) | phone ✔ rows 1–3 (y 459–690) | 5 |
| Remote switching row | 2–5 | — | Low | — | phone ◐ at y 732–836 | 1 |
| Remote graphics | 5–20 | — | High (F1) | ✎ text edits don't exist on the phone | phone ✘, y 876 | 3 |
| Remote comments | 5–30 | — | Med | You can't read the comments | phone ✘, y 965 | 4 |
| Remote sound pads | 10–40 | — | Low (they use pointer-down) | — | phone ✘, y 1054–1209 | 2 |
| Remote audio (mute only) | 2–10 | — | **High: one tap mutes the mic** (measured) | No faders | phone ✘, y 1258 | 4 |
| Desktop at 1280x800 | throughout | — | — | Status line "…" | — | 3 |
| Desktop at 1024x700 | throughout | — | — | The header overlaps itself | — | 4 |

### (b) Friction findings

1. **S1: Phone taps on scenes get lost while the show is running. Measured.**
   - The page throws away and rebuilds every scene, graphic, audio and macro button on every state push (remote.html:211 and the lines after it).
   - The app pushes about every 150 ms while recording or while levels move (ui.rs:1708; remote.rs:419-424). The scene grid was rebuilt 18 times in 3 s.
   - A button replaced between finger-down and finger-up never gets its click.
   - Results by how long the press lasted:

     | Press length | Scene taps that worked |
     |---|---|
     | 60 ms | 5/6 |
     | 100 ms | 3/8 |
     | 150–250 ms | 2/8 |
     | 350 ms | 0/6 |

   - A deliberate one-thumb press is the worst case. This is the main rage-click generator on the phone.
   - Pads, REC, LIVE and Talk use pointer-down or static elements, so they aren't affected.
2. **S1: The phone shows a healthy LIVE while the stream is broken.**
   - The phone's state carries only `live` and `live_seconds`. There is no destination state, and `problems` covers only health: sources, files, disk (ui.rs:1558-1563).
   - With both destinations failing, the phone showed the pink "◉ LIVE 00:00:03", `problems: []`, and the disk line (`b-remote-390-live.png`). The desktop showed orange GOING LIVE / "Retrying".
3. **S1: Dangerous one-tap actions on the phone.**
   - FTB blacks the program on one tap, no confirm (measured: black = true; `b-remote-390-ftb.png`). It sits next to Replay and Clip.
   - An audio button mutes the mic on one tap (measured; remote.html:276).
   - GO LIVE starts a broadcast on one tap (remote.html:128, "Starting is one tap"). The pre-show check is off by default (config.rs:1513).
   - Only stopping REC and LIVE needs a hold.
4. **S2: Things the phone can't do that the desktop can.**
   - Read comments: it has only "Show next" and "Hide" (remote.html:338). You put chat on air blind.
   - Set fader levels, or see any stream bitrate or destination state.
   - Fix a source drop or see the multiview. It shows only the program, refreshed every 700 ms.
   - Edit a title's text (✎), name a marker, or see the marker list.
   - Undo, open the take or the Log, or see Health detail.
   - Get any confirm when the desk raises one. A remote Record with the check on returns "Running the pre-show check on the desk" (ui.rs:1285).
5. **S2: What's within thumb reach is the wrong set for one hand.**
   - At 390x844 the first screen shows REC, GO LIVE, Marker, Clip, Replay, FTB, the program and 3 rows of scenes (`b-remote-390-top.png`).
   - Graphics, pads, comments and audio start at y 876 or lower (`b-remote-390-full.png`, a page 1473 px tall). At 360x740, Clip drops to a third header row.
   - The most-used controls in a one-hand grip (scenes, pads) are mid-page. The big pills you rarely press (REC, GO LIVE) sit at the top. FTB, which shouldn't be easy to hit, sits beside Replay.
   - Tally links and the Stream Deck API hint fill the bottom of an operator's screen.
6. **S2: The desktop at 1024x700 is not usable as a second, small window.**
   - The header overlaps itself: Replay, "Set up streaming", Screen, Check, Health and the clock (`compact.png`, `b-p1-guide-1024.png`).
   - While LIVE, the pill covers Check, Health and the clock; only "og (3)" of the Log button is left (`b-p3-live-1024.png`).
   - GRAPHICS is pushed below the right column's scroll (`compact.png` shows Stream, PTZ, then Audio). The audio ON buttons are cut off.
   - At 1280x800 the status line is "…" and the bank shows "+2 ›" (`air.png`, `b-p1-main-1280.png`).
7. **S3: The phone's text overlaps.** The program-name tag is fixed at left:84px (remote.html:31) and overlaps the "PROGRAM" tag at a 2x device pixel ratio (`b-remote-390-top.png`). The audio meter is a thin black bar on a green button, so it is hard to read (`b-remote-390-full.png`).
8. **S3: Joining the phone works well.** Settings → Remote & MIDI shows a QR code with the PIN built in (settings/control.rs:75-79). The PIN screen also says where to find the PIN. This is good onboarding.

---

## (c) Screens ranked by use across all three personas

| Rank | Surface | Share on desktop / phone (P3 estimate) | Notes |
|---|---|---|---|
| 1 | Scenes (bank and remote grid) | 40 / 60 | Most-pressed control. Broken taps on the phone (P3 F1) |
| 2 | Program monitor and status: REC timer, LIVE state | 50 / 50 | Glanced at constantly. The phone hides stream trouble (P3 F2) |
| 3 | Audio strip (meters, ON, faders) | 80 / 20 | Clipped on every size we tried (P2 F2). The phone has mute only |
| 4 | Stream panel | 95 / 5 | The phone has none of it |
| 5 | Comments | 70 / 30 | The phone can't read them |
| 6 | Sound pads | 40 / 60 | The phone version works well |
| 7 | Graphics toggles | 50 / 50 | The empty state is invisible to new users (P1 F1) |
| 8 | Multiview tiles | 100 / 0 | Duplicates the program with 1 camera |
| 9 | Marker / Clip / Replay | 50 / 50 | |
| 10 | Health / Log | 100 / 0 | Visited in trouble; no stream data in Health |
| 11 | Pre-show check | 100 / 0 | Once per show |
| 12 | Settings (Sources, Audio, Streaming, Graphics, Comments) | 100 / 0 | Heavy at setup, near zero during the show. Overwrites live changes (P2 F6) |
| 13 | First-show guide | 100 / 0 | Once. Gaps decide whether a new buyer succeeds |
| 14 | Transition row (WIPE, PUSH, DIP, STINGER, FTB), Edit layout, PTZ | about 90 / 10 | Rarely touched during the show |

## (d) What must be one glance away, and what can go under "More"

**One glance away, on desktop at 1024 and up and on the phone:**
- REC with its timer, and the LIVE state *per destination*. That means "1 of 2 live", bitrate against target, and dropped frames on the phone too.
- The scene buttons, with taps that don't get lost.
- The program picture.
- Mic ON/mute plus a level meter, never clipped. A fader can be one tap deeper.
- Any active problem (source drop, disk, destination down), in one line that is never truncated.
- Pads and Show next / Hide comment, for streamers who use them. Comment text must be readable before it goes on air.
- Graphics toggles, with an empty state that offers "+ Name titles" instead of disappearing.

**Under "More" (desktop) or a second tab (phone):**
- WIPE, PUSH, DIP, STINGER and the transition duration. Keep CUT/MIX plus the current mode.
- FTB, which also needs a hold or confirm on the phone.
- AUTO, Replay and Clip when not recording, Edit layout, and PTZ unless there is a PTZ camera.
- Screen (the second window), Check (until before the show), Health and Log. Turn these into one "status" chip that expands.
- The clock, the show picker and the disk line. Fold them into the status chip at under 1280 wide.
- On the phone: tally links, the Stream Deck/API hint, media players, macros, timers and the teleprompter.
