# 1 — Usage audit (Phase 1)

Cutaway has no analytics (no PostHog, GA, logs of clicks or database), so this
audit comes from **simulated users**: agents playing real people through the
real 0.9.7 app, built and screenshotted on Linux with test pictures standing in
for cameras, plus a read of the code behind every screen. Numbers below are
**estimates per show** from those walkthroughs, not measurements, except where
marked *measured*.

Personas run (full walkthroughs in the appendix files):

| Persona | Setup | File |
|---|---|---|
| Show operator (Brayden) | 13" MacBook, record only. 3 cameras, the 4K Couch camera cut into Maya and Sam close-ups, screen capture, 7 scenes, name titles, Resolve afterwards | `agent-show-operator.md` |
| First-time buyer | Windows laptop, 1 webcam + USB mic, 2-person interview with a name title | `agent-new-stream-phone.md` (P1) |
| Streamer | 2 cameras + screen, YouTube + Twitch at once, chat, sound pads | `agent-new-stream-phone.md` (P2) |
| Phone in one hand | The web remote at 390×844 and 360×740, plus the desktop in a 1024×700 and 1280×800 window | `agent-new-stream-phone.md` (P3) |

## Screens ranked by use

"Visits" counts deliberate looks or presses in a one-hour show. "Desk/phone"
is the share of that use from the desktop vs the phone remote (P3 estimate).
Rage = places a user clicks again and again because nothing visible happens.
Dead = looks clickable and isn't, or is clickable and doesn't look it.

| Rank | Screen / surface | Visits per show | Time | Desk / phone | Rage / dead clicks | Above the fold at 1280×800? | Verdict |
|---|---|---|---|---|---|---|---|
| 1 | Scene bank (+ number keys, remote grid) | 60–150 | whole show | 40 / 60 | Phone taps lost while buttons rebuild (*measured*: 2 of 8 taps land at 150–250 ms). 7th scene hidden behind "+2 ›" | No: PiP is off screen at 1280 and 1480 | First, deep work |
| 2 | Program monitor | constant glance | whole show | 50 / 50 | — | Yes | First |
| 3 | Header status: REC pill, timer, disk, clock | 30+ glances | — | 50 / 50 | Hold-to-stop: a click only says "Hold to stop", invites repeat clicks | Disk line truncated to "…" at 1280; header overlaps itself at 1024 | First, deep work |
| 4 | Audio strip (meters, ON, faders) | 20–40 | — | 80 / 20 | AUDIO heading looks like the foldable ones but does nothing | ON buttons clipped at 1480, 1280 and 1024 | First, deep work |
| 5 | Multiview tiles | 30 glances, a few clicks | — | 100 / 0 | **A click puts the camera on air** (Preview off), where you'd click to look closer | Yes | Keep, change click |
| 6 | Marker / Clip | 3–10 | seconds | 50 / 50 | ▾ next to it is two different menus (recent takes / markers) | Yes | One glance |
| 7 | Graphics panel (Names, Logo) | 5–15 | seconds | 50 / 50 | Disappears entirely when there are no graphics (new users never find it) | Pushed out of view at 1024×700 | One glance |
| 8 | Stream panel | 0 (record show) / 30 (streamer) | — | 95 / 5 | Phone shows LIVE while both destinations fail | Yes | Only when streaming exists |
| 9 | Sound pads | 0 / 20 (streamer) | — | 40 / 60 | — | Below the fold at 1280 | Area tab |
| 10 | Viewer comments | 0 / 20 (streamer) | — | 70 / 30 | One click puts a chat line on air; phone can't read them | Varies | Area tab |
| 11 | PTZ panel (virtual close-ups) | 5–10 before, ~0 during | minutes pre-show | 90 / 10 | ⌂ Home under Zoom out snaps an on-air close-up wide | Yes | Fold during show |
| 12 | Take saved card / Recent takes | 1–2 | 1–3 min | 100 / 0 | "Open in Resolve" copies a path; first time it opens Settings | — | Deep work (after-show) |
| 13 | Pre-show Check | 1 | 1 min | 100 / 0 | ⚠ rows can't be clicked through to a fix | — | Keep, link fixes |
| 14 | Settings: Video sources | setup: 10+, show: 0–2 | 5–15 min | 100 / 0 | Forgets selected source on every open; covers the program | — | Deep work |
| 15 | Settings: Audio, Streaming, Graphics, Scenes, Recording | setup only | minutes each | 100 / 0 | Apply can undo desk changes made while open | — | Areas |
| 16 | Edit layout | between shows | minutes | 100 / 0 | Delete has no confirm; with Preview off it edits what's on air | — | Area tab |
| 17 | Health / Log | 0–5 (only when amber) | seconds | 100 / 0 | Health counts 0 fps for every camera for a second after start, so it's amber at rest | Opens over REC | Merge into one status chip |
| 18 | First-show guide | once ever | 10 min | 100 / 0 | Names people "for titles" but never makes a title; ticks "cameras" for test patterns | — | Deep work (onboarding) |
| 19 | Transitions beyond Cut/Mix, FTB, AUTO, Replay, Screen, Webcam | 0–2 | — | 90 / 10 | FTB drawn dim but live; AUTO lights up and does nothing without rules | Yes (taking space) | **More** |
| 20 | Rundown, Macros, Prompter, Syrinscape, Captions, Vertical, Upload, Scheduled, Remote & MIDI, Shortcuts, Diagnostics | 0 for these personas | — | — | — | — | **More** (themed) |

## What the data decides

**First, always one glance away** (desktop ≥1024 wide and phone): the scenes,
the program, REC + timer + one "time left" number, mic ON + level, Marker/Clip,
Names, and *one* status light that turns amber (replacing Health (n) + Log (n)).

**Second, one click away as areas**: Sources & people, Scenes, Sound, Graphics,
Record & stream (including takes).

**Under a themed "More"**: the fancy transitions and FTB (in a ⋯ beside
Cut/Mix), Replay, AUTO, the second screen and webcam, Rundown, Macros,
Prompter, Comments (until a stream destination exists), Captions, Vertical,
Upload, Scheduled, Remote & MIDI, Shortcuts, Diagnostics.

**Deep work goes to**: scene bank, header status, audio strip, after-the-show
(takes → Resolve), Sources setup, and onboarding.

Limits of this audit: no real user clicks were counted. If you can tell me which
of these you actually touch in a show, that beats every estimate here.
