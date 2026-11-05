[← The manual](README.md)

# Show tools and sound

## Replay, macros, ticker, comments and the teleprompter

- **Instant replay:** while recording, **⟲ Replay** plays the last few
  seconds (More › Show tools › Replay sets how many, and the slow
  motion speed) over the program, labelled "REPLAY", and fades back to live.
  It comes from the recording itself, so it costs nothing extra.
  - **From any camera:** **⋯** beside CUT / MIX on the scene bank has
    **⟲ Replay** (with **Slow**, each camera and **the last 5 / 10 / 20 /
    30 s**); the status light's panel has it too while recording. Pick a
    camera to see
    the moment from its angle, even one the program wasn't showing, or
    **Every angle** (2 to 4 cameras, in a grid). **Shows** in
    the replay settings sets what a plain click plays. It uses each camera's
    own file, so the camera needs **Record to its own file** on (and files
    that aren't ProRes, which are too big to keep in memory).
- **Macros:** **More › Show tools › Macros:** a name, a colour and a list of commands
  (any from the table above, with `wait:` between). They're buttons in the
  main window, Stream Deck keys and remote buttons, and can be cues.
- **Scrolling ticker:** **Graphics › More ▾ › Tickers & web overlays → ＋ Scrolling ticker:** the text, a label
  and the speed. ✎ on its button changes the text live.
- **Web page overlays:** **Graphics › More ▾ › Tickers & web overlays → ＋ Web page:** an address
  (or **Choose file…** for an HTML file) and the size the page is laid out
  at (the program's size by default). Its button shows and hides it like
  any graphic; **⟳** beside it (or **⟳ Load again** on its page) loads the page again. Pages are drawn by
  Google Chrome or Microsoft Edge (or Brave, Chromium or Vivaldi), run
  hidden in the background, so one of them needs to be installed: every
  Windows PC has Edge. A page's sound isn't heard.
- **Viewer comments:** **More › Show tools › Viewer comments:** your Twitch channel
  (no sign-in needed) and/or a YouTube live video with a YouTube API key.
  Comments appear in the main window's **Comments** panel; click one to put
  it on screen (styled in those settings), or **Show next**. The Comments panel
  appears on **Show** once you've set a Twitch channel or a YouTube stream
  here. The page itself is always there, so you can set it up before going live.
- **Teleprompter:** **More › Show tools › Teleprompter:** type or paste the script, or
  use the rundown's segment notes (it then jumps to each segment as the
  rundown moves on). Show it on the second screen (**Record & stream › Screens &
  webcam › The teleprompter**), or open `http://<address>:8790/prompter` on
  a tablet, which needs the remote control on. Mirror it for a glass
  prompter. Roll, pause, speed and jumps work from the Teleprompter panel,
  the Stream Deck, the remote, MIDI, or a keyboard on the tablet (space,
  arrows).

## Sound pads, Syrinscape and talkback

- **Sound pads:** **Sound › Pads → ＋ Add sounds…** (WAV, AIFF,
  MP3, AAC/M4A, FLAC, Ogg or CAF). Each pad has a name, colour, how it plays
  (once, loop, or while held), a level and a fade-out. Cutaway adds a **Sound
  pads** audio input for them, with its own fader, meter and WAV file (it
  waits for **Apply** on the bar, as a new input does). Tick
  **Duck under voices** on it to keep beds below the talking.
- **Syrinscape:** **Sound › More ▾ › Syrinscape:** paste your Syrinscape key (the auth
  token from syrinscape.com/account/auth-token), press **Test**, then **Load my
  soundsets** and **＋ Add** the moods and sounds you want as buttons (or
  paste a remote-control link). Keep the Syrinscape Online player open. To
  hear it in Cutaway, add an audio input set to **Sound from an app** → your
  browser or the Syrinscape app (on a Mac, allow Screen Recording when
  asked), or route the player to a virtual cable. ▶ beside a button plays
  it now (■ stops it), and **Volume** sets Syrinscape's own volume.
- **Talkback:** **Sound › More ▾ › Headphones & talkback → Talkback:** choose your
  mic. Add the presenters' headphones under **More outputs** (tick
  **Talkback**). Hold **TALK** or `B` to speak to them; guests hear it too
  unless you untick it.
- **Callers:** **More outputs → ＋ Caller's own mix** (a mix-minus, so they don't hear
  themselves): pick the output channels that go to the phone hybrid or call app, and tick
  the caller's input under **Leave out**.

## Getting your DAW's audio in

Cutaway records from audio *inputs*, so route the DAW's output to one:

- **Two computers, or a hardware mixer:** send the DAW's output into your audio
  interface inputs. In **Sound › Inputs**, pick the interface and set
  how many channels to capture. Then set the mix channels to the pair carrying
  the DAW mix, e.g. 3 and 4.
- **Same Mac:** set the DAW's output to a loopback device and pick that device
  in Cutaway. BlackHole (free) and Rogue Amoeba's Loopback both work. Many
  interfaces (RME, MOTU, UA, Focusrite) also have a built-in loopback channel
  pair you can use.
- **Lip sync:** cameras are usually 2–5 frames behind the audio. Press
  **Measure lip sync…** (Sound › Inputs, or in the pre-show check),
  then clap once, sharply, where every camera can see it. Cutaway finds the
  clap in every camera and every input and shows the delays it suggests;
  tick the ones to apply. (`cutaway --measure-sync` does the same from the
  command line.) You can also set **Delay** on any input or source by hand.

## Captions, transcripts and the podcast

- **Captions:** turn them on in **Graphics › More ▾ › Captions** (it saves as
  you go) and pick what to
  listen to (the mix, or one input) and the language. The first time, macOS
  asks for **Speech Recognition** permission; on Windows, press
  **Download** for the speech model (once). Captions appear on the stream,
  and on the recording too if you choose (**Show on**). **CC** (or `C`)
  shows and hides them during the show.
  - Recognition runs on the Mac. If a language's on-device model isn't
    installed, Settings says so: turn on Dictation for it in System Settings
    → Keyboard, which downloads it.
  - Each recording gets `Transcript.txt`, `Captions.srt` and `Captions.vtt` (for
    YouTube and editors), timed to the recording.
- **Input tracks:** **Record & stream › Quality and files › Editing**: turn on **Each input
  on its own track** to put every input on its own track in the Program file. **Input tracks**
  picks whether that's before or after the fader.
- **Podcast:** **Record & stream › Quality and files › Podcast** makes an M4A of the mix
  after each recording: measured, levelled to the target loudness (−16 LUFS for
  Apple Podcasts and Spotify), peaks limited to −1.5 dB, optionally mono,
  tagged with the title, show and author, with the markers as chapters.

## The vertical version

**Record & stream › Vertical version** makes a 9:16 version of the show (1080×1920
or 720×1280) alongside the program, from the same sources. It appears as a
tall monitor beside the program; click it to change its settings.

- **Each scene gets a tall layout:** automatically, a full-screen source
  fills it, picture in picture keeps its inset (at the top right, clear of
  the captions and buttons apps put at the bottom), and side-by-side and
  grid scenes are stacked top to bottom. Or choose **Stacked** or **One
  source** for any scene.
- **Each source gets a framing:** which part of its picture fills a tall
  frame (slide it to where the person sits) and how close.
- **Graphics:** the program's titles, logos and tickers come along, across
  its width: ones at the top stay near the top, the rest sit low down. Turn
  them off if you'd rather not.
- **Record** it in each recording as `Program_Vertical`, with the recording mix,
  and/or **stream** it: in Record & stream › Streaming, set a destination to send
  the vertical version.
- Wipes, pushes and dips become mixes in the tall picture; fade to black,
  replays and stingers come along.
