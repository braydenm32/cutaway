[← The manual](README.md)

# Getting started

## First run

- **Your first show:** the first time Cutaway opens, a short guide walks
  through cameras, **naming your people**, the microphone, **your scenes**,
  where recordings go, going live (optional), the pre-show check, a test
  recording, (optional) **editing afterwards** and (optional) **switching from
  a phone or Stream Deck**. Each step has a button to
  the right page and ticks itself off when it's done. The cameras step counts
  real cameras only: test pictures standing in when none was found don't
  tick it, and it says where to let Cutaway use the camera. The microphone
  step names the mic it listens to and ticks only when that mic hears you
  (sound on another input doesn't count; it says which one does, so you can
  pick it in Sound › Inputs). Naming
  people is done right in the guide, with the same rows as **Sources ›
  People & names** that behave the same way: who's in each picture and their
  role are saved as you type, and a camera's new name when you press Enter or
  click elsewhere (it restarts that camera for a moment, so it waits while
  you're recording or live). The step also offers **＋ Make a name title**
  (with nobody named yet it takes you straight on to **Sources › People &
  names**, so the first name box is the next click) and
  **Close-ups…** (a close-up of each person from one camera). The cameras
  step also says how to add your screen (**＋ Add → Screen, window or app**).
  **Record a test** has a button that starts recording; once a recording is
  saved, the step says where to find it (the Recording saved card, or Record & stream ›
  Recordings) with a **Recordings…** button. On a Mac the guide says up front that macOS
  will ask about the camera and microphone (choose Allow). Your scenes turns on **Edit layout**, and Editing
  afterwards opens **Record & stream › Quality and files › Editing** (its For editing part), where the DaVinci Resolve,
  Premiere Pro and Final Cut projects are switched on. **Switch from a phone
  or Stream Deck** is skipped like Going live until you want it: its button
  turns Remote control on and opens **More › Control › Phone remote**, where
  the QR code is. A step's button folds
  the guide down to a small **Guide** button on the left, just above More, that shows
  how many steps are done (3 of 8, or 3/8 when the window is narrow), so the page it opened can be used; click it to carry on. Open
  it again any time from **More › App › Help** (or **First-show guide** under
  the status light). What it has ticked off (the mic heard you, the check
  was run, a test recorded) is remembered between launches, and if you quit
  with the guide open it opens again next time, unless every step that counts
  is done: the Done row then says *All set: this closes itself next time*, and
  it stays closed. Skipped optional steps don't hold it open, and you can
  still open it from **More › App › Help** or the status light.
- **Auto-setup:** on first launch Cutaway writes a settings file with every
  camera and capture card it finds, plus your microphone (a USB mic or
  interface before the computer's own; a loopback or a phone nearby only if
  there's nothing else), to
  `~/Library/Application Support/Cutaway/config.toml` (on Windows,
  `%APPDATA%\Cutaway\config.toml`).
  Change anything in the areas down the left of the window, or edit the
  file directly. `cutaway.example.toml` explains every option.
- **The rail down the left** has the areas: **Show** (the live desk),
  **Sources**, **Scenes**, **Sound**, **Graphics**, **Record & stream** (it shows as **Record** on the rail), and **More**
  at the bottom (⌘1–⌘7, or Ctrl+1–7 on Windows; **Esc** goes back to Show
  when you're not typing). Clicking an area turns the top bar into its tabs;
  the ones used least sit under **More ▾** at the end of the row, and so do
  any that don't fit a narrow window. Each area opens where you left it on
  this computer, and Cutaway always starts on Show. Below 1200 pixels wide
  the rail shows icons only (hover for the name). The logo at the top opens
  **Updates & about** (a blue dot on **More** means a new version is out),
  and the show's name under it switches shows.
- **The settings pages** open inside the window, in the area they belong to
  (Video sources is Sources › Cameras, Recording is Record & stream › Quality and files, and
  so on); less-used parts fold away, showing how they're set. Hover a
  setting's name, or its ⓘ, for what it does.
- **Search (⌘K, Ctrl+K on Windows, or the magnifier in the top bar)** opens
  one box over whatever's open. It finds any setting by its name or by what
  its help says, any area, tab or page (the old Settings page names like
  "Remote & MIDI" still work), and the desk's own buttons and controls
  ("fade to black", "named marker", "12-hour clock"). ↑ ↓ choose, Enter goes
  there: the area and tab open, a folded part unfolds, and the setting's name
  lights up for a moment. A desk button is shown, never pressed, so nothing
  goes on air from search. ⌘K works even while you're typing in a box; Esc
  (or ⌘K again) closes it, and what you typed is still there next time.
- **Changes save as you go** on every page: what you change applies
  straight away and is saved, a box when you press Enter or click elsewhere,
  a port or the interface size when you let go of it. There's no OK, Apply or
  Cancel. **↶ Undo** and **↷ Redo** at the bottom of the page (or ⌘Z when
  you're not typing in a box) take it back one step at a time, and **Undo
  changes on this page** puts the whole page back as it was when you opened
  it. Changes that would add, remove or restart a source or a sound input, or
  the program's size or frame rate, wait in an amber line at the bottom
  ("Waiting: restarts Couch · adds Couch left") with **Apply (restarts
  cameras)** (or **Apply (restarts inputs)**) and **Undo these**, and an
  amber dot on the area in the rail; going back to Show asks before dropping
  them. A change saved on a page puts on only what you changed there, so a
  fader, ON/OFF, transition or anything else changed on the desk meanwhile
  stays as it is. Your show's keys keep working on these pages, except while
  you type in a box. Esc goes back to Show.
- **More** (the last area on the rail) holds the rest, in three tabs:
  **Control** (Phone remote, Stream Deck, OSC and MIDI, Shortcuts), **Show
  tools** (Rundown, Macros, Teleprompter, Viewer comments, Replay) and
  **App** (Shows, General, Diagnostics, Updates & about, Help & first-show
  guide). Viewer comments is always there, so you can set them up before
  streaming. The rundown segment and macro you had picked are picked again
  when you come back.
- **Starting up:** the window opens straight away; while the cameras come up,
  the monitors show a clapperboard with a line from the studio floor.
- **Check your hardware:** **More › App › Diagnostics → Check formats**
  (or `cutaway --probe-encoders`) shows the encoders (including whether
  ProRes is available), NDI and the GPU Cutaway will use; **List devices
  again** there (or `cutaway --list-devices`) lists the cameras, capture
  cards and audio inputs it can see, with **Copy**. Where the settings file
  is: **Diagnostics → Settings file → Show**.
- **Try it without hardware:** `cutaway --demo 4` runs with 4 test pictures and
  a test tone.

## If something goes wrong

- **The log:** everything Cutaway reports, including the full details of any
  internal error, goes to `~/Library/Logs/Cutaway/Cutaway.log`. Find it under
  **More › App › Diagnostics → Show**, or in Console.app under Log Reports.
- **The status light** at the top right says **All good**, or **2 to check**
  in amber: problems now (a source with no signal or below its frame rate, a
  file failing, a mic gone quiet, the drive low on space) plus warnings in
  the log you haven't looked at yet. Click it: **Now** lists the problems and
  everything at a glance, **Log** is everything Cutaway has noted this session
  (**Warnings only**, and **Copy** to paste it into a message), and **Run
  pre-show check** runs the check.
- **No sound in the recording:** while you record, Cutaway listens to the
  recording mix. If it stays silent (below about −60 dB, quieter than any
  room) for 45 seconds, because a mic is off, muted, unplugged or the wrong
  input is picked, it says so in a message, on the status light and on the
  phone remote. It says it once, and again if the sound comes back and then
  goes again. A small amber **no sound** beside REC shows whenever the
  recording has been silent for a few seconds.
- **Messages:** an error stays at the bottom of the window for 30 seconds
  (or until you click it), and several stack up rather than replacing each
  other; the status light and the Log keep it after that. Disk-space and
  backup-drive warnings show there too, and every error also goes in the Log.
  If a scene key does nothing, check the top bar: **⌨ Typing** means a box has
  the keyboard (Esc).
- **Problem report:** **More › App › Diagnostics → Save a problem report…**
  saves one .zip to send: the log, the last recording's note, your settings and
  the devices found. Stream keys, passwords and tokens are taken out, even
  where they appear in the log.
- **Now, under the status light,** shows each source's frame rate
  ("measuring…" for the first second after the cameras start, rather than a
  false 0), each file being recorded (frames written, repeated, dropped,
  queued: Repeated and Dropped turn amber for trouble in the last minute, not
  for one bad moment hours ago), each input's re-syncs and the drives.
- **A device that misbehaves** doesn't stop the app. It's marked offline (the
  same meaning everywhere: the desk, the phone remote, OSC and the pre-show
  check), the reason is in the Log, and it's reopened when it comes back.
- **A source that drops while it's on air** (a camera unplugged, a network
  feed gone) puts a red chip on the program monitor ("Cam 2: no signal,
  reconnecting…"), one error message at the bottom, and a ⚠ on every scene
  button that uses it, until it's back. To have Cutaway cut away by itself,
  set **Sources › Cameras → If it drops while on air** (on each source's page, under its Name, Type and Device) to a scene
  (say, the wide shot for a close-up camera): it cuts there once, as the
  signal goes, unless that scene also needs a source with no signal. It
  doesn't switch back when the source returns.
- **A crash in one part of Cutaway** doesn't spread to the rest: the other
  parts keep using the shared settings and state it left behind, and the
  crash's details are in the log.
- **Run from Terminal** to watch messages live:
  ```sh
  /Applications/Cutaway.app/Contents/MacOS/Cutaway
  ```
