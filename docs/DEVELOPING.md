# Developing Cutaway

How Cutaway is built, how to build and test it on Linux, how releases are signed, and what has been tested where. For using Cutaway, see the [manual](guide/README.md).

## How it works

```
camera / network / media ──► source slot ──┬──► compositor (GPU) ──┬──► monitors, second screen, virtual camera
                                           │     ▲   ▲             ├──► [Program file] ──► [proxy]
                                           │ switcher graphics     └──► [stream encoder] ─┬─► [RTMP: YouTube]
                                           └──► [ISO file] ──► [proxy]                    ├─► [RTMP: Twitch]
                                                                                          └─► [SRT]
audio in ─► voice processing ─► mixer (drift-corrected) ─┬─► recording mix ─► limiter ─► [AAC in every file]
       │                                                 ├─► stream mix ─► leveller ─► limiter ─► [stream AAC]
       │                                                 └─► headphones
       └──► [WAV]
```

Branches in [brackets] exist only while recording or live. Every file can
also be mirrored to a backup drive.

**Timing:** everything is stamped on one clock, the same one macOS uses for
camera and audio timestamps.

- The compositor renders each program frame a little behind real time
  (50 ms), using the frame each camera captured at that instant.
- The mixer does the same for audio, resampling each input slightly so its
  clock can't drift away from the video. Scene audio changes are scheduled
  for the moment the picture changes.
- Recording starts and stops on a program frame boundary. Each source is
  placed onto its own constant-rate frame grid from that instant, and audio
  is cut at the exact sample, so every file starts at zero together.

**Isolation:** recording files, encoders, stream destinations, network
sources and controllers each run on their own threads with their own queues.
A full disk, a slow encoder, a stream server that goes away or a network
source that drops can't stall the cameras, the program or the other outputs.

The remote server (`src/control/remote.rs`) uses one thread to accept, one to
let go of holds from a phone that stopped saying it was holding, one for each
request while it is answered, and two for each WebSocket controller (a
reader and a writer; 1 + 2N threads with N controllers). A controller that
sends nothing for 40 seconds or can't be written to for 10 is dropped. The
app only builds the state for the remote while a controller is connected or
something asked for it in the last few seconds.

| File | What's in it |
|---|---|
| `src/engine/mod.rs` | Starts sources, the compositor and mixer; takes and streams |
| `src/engine/take.rs`, `recorder.rs` | Recording: retiming, encoders, file writers, WAVs, proxies, backups |
| `src/engine/compositor.rs` | GPU compositor (wgpu): layers, styling, keying, transitions, graphics |
| `src/engine/audio.rs`, `dsp.rs` | Drift correction, mixes, voice processing, loudness, limiter |
| `src/engine/switcher.rs`, `autoswitch.rs` | Live scene, preview, transitions; automatic switching |
| `src/engine/streamer.rs` | Stream encoder, per-destination connections, reconnects |
| `src/engine/netsrc.rs`, `feed.rs`, `ndi.rs`, `player.rs` | Network sources, NDI, the media player |
| `src/engine/health.rs`, `precheck.rs`, `watchdog.rs` | Disk and recording health, pre-show check, device reconnects |
| `src/engine/export.rs`, `podcast.rs` | Final Cut / Resolve / Premiere timelines, chapters; the podcast episode |
| `src/engine/captions.rs`, `lipsync.rs` | Live captions and transcripts; lip-sync measurement |
| `src/engine/vertical.rs` | The vertical version's layouts (the compositor draws it) |
| `src/engine/ptz.rs`, `replay.rs` | PTZ cameras (VISCA, NDI) and virtual PTZ; instant replay |
| `src/engine/guests.rs`, `guest.html`, `phone.html` | Remote guests, phone cameras and backup recordings (WebRTC) |
| `src/engine/framing.rs`, `cutout.rs` | Auto-framing; background removal |
| `src/control/` | Command language, web remote and WebSocket API, OSC, MIDI, Stream Deck key pictures, show profiles |
| `src/media/` | MP4/MOV, WAV, FLV, MPEG-TS writers and readers; crash recovery |
| `src/net/` | RTMP/RTMPS and SRT clients, RTMP server; live chat; Google sign-in and uploads; the update check and updating in place |
| `src/platform/macos/` | AVFoundation, ScreenCaptureKit, VideoToolbox, AudioToolbox, CoreMediaIO, Speech, Vision |
| `src/platform/win/` | Media Foundation capture, encoders and decoders, Windows Graphics Capture, the virtual camera |
| `src/platform/audio_*.rs` | Audio devices on both (CoreAudio; WASAPI and ASIO) |
| `src/platform/whisper.rs`, `onnx.rs` | Captions with Whisper; finding people with ONNX Runtime (Windows) |
| `assets/logo/` | The logo, and `make_logo.py`, which draws it and every app icon made from it (Mac, Windows, the window, the phone remote, the Stream Deck plugin) |
| `windows/` | The Windows build script, icon, and the virtual camera source (`windows/vcam`, a Media Foundation media source DLL) |
| `models/` | The two small models for finding people on Windows, with their licenses |
| `macos/virtualcam/` | The camera extension (Swift) and its signing script |
| `src/ui.rs`, `src/ui/`, `src/settings/` | The director interface: the rail of areas, the desk, and the settings pages (which save as you go, `src/settings/live.rs`) |
| `streamdeck/` | The Stream Deck plugin (no dependencies), its icons and its test |

## Development on Linux

`cargo run -- --demo 4` and `cargo test` work on Linux with test sources,
network sources and the media player. The Linux build uses OpenH264 and
FDK-AAC in place of the system encoders (neither is part of the Mac or
Windows app), and, like Windows, Whisper for captions and ONNX Runtime for
finding people (set `ORT_DYLIB_PATH` to `libonnxruntime.so`).

The shipped build uses its own profile, `dist` (in Cargo.toml: link-time
optimisation, one codegen unit, debug info stripped). It is slower to build, so
the day-to-day loop uses `--release`. Build it with `cargo build --profile dist`:
the binary is `target/dist/cutaway` (the scripts in `macos/` and `windows/` do
this for the packages, and CI's tests keep using `--release`).

Checking the other systems' code from Linux:

```sh
cargo check --target aarch64-apple-darwin                               # macOS
cargo check --target x86_64-pc-windows-msvc --no-default-features        # Windows (without whisper.cpp's C++ build)
cargo check --target x86_64-pc-windows-msvc --manifest-path windows/vcam/Cargo.toml
```

## Pinned downloads

Everything Cutaway (or its build) downloads from someone else is a fixed
version, checked against its SHA-256 before it's used, so a changed file is
refused rather than run. To move one on, change the version and the
fingerprint together:

| What | Where | Fingerprint from |
|---|---|---|
| cloudflared (the guest link) | `CLOUDFLARED` and `download_url` in `src/net/tunnel.rs` | The "SHA256 Checksums" list in that release's notes on GitHub |
| Whisper speech models (Windows captions) | `MODELS_AT` and `model_file` in `src/platform/whisper.rs` | Each file's page on Hugging Face (ggerganov/whisper.cpp at that commit) shows its SHA256 |
| ONNX Runtime and DirectML (Windows build) | The `Get-NuGet` lines in `windows/build.ps1` | `Get-FileHash` (or `sha256sum`) of the .nupkg from api.nuget.org |

cloudflared stops being supported by Cloudflare about a year after its
release, so move it on at least that often. NDI's installer always comes
from NDI's newest link, so instead of a fingerprint it must carry a valid
code signature before it's opened. Steinberg's ASIO SDK (only for
self-builds with ASIO) is the same; set `CUTAWAY_ASIO_SHA256` to insist on
one. The workflows use only GitHub's own actions (`actions/*`); Rust comes
from rustup on the runner.

## Signed updates (for whoever publishes releases)

The updater checks each download against the release's `SHA256SUMS.txt`.
So that only the maker can publish an update, that list is signed with a
key kept off line (never on GitHub):

1. Once: `cutaway --make-update-key ~/cutaway-update.key` makes the key and
   prints its public half. Keep the file private (a USB stick and a password
   manager). The public half goes in `PUBLIC_KEY` in `src/net/signing.rs`.
2. Each release: the Release workflow then makes a **draft** release. Download
   its `SHA256SUMS.txt`, run `cutaway --sign-update SHA256SUMS.txt --key
   ~/cutaway-update.key`, add the `SHA256SUMS.txt.sig` it writes to the draft,
   and publish it.

From the first version with the public key in it, Cutaway refuses an update
whose fingerprints aren't signed with that key. Versions before it keep
updating as they always have.

## Signing the Mac app (Developer ID and notarization)

The Release workflow signs Cutaway.app with an Apple Developer ID and has
Apple notarize it, so Macs open it without "Apple could not verify". It only
does this for releases; builds of ordinary pushes stay ad-hoc signed and never
see the certificate. Until the secrets below exist, releases are unsigned and
the workflow says so with a warning.

Once (needs the Apple Developer Program):

1. Xcode → Settings → Accounts → your team → Manage Certificates → **+** →
   **Developer ID Application**. Export it from Keychain Access (My
   Certificates) as a `.p12` with a password.
2. App Store Connect → Users and Access → Integrations → App Store Connect
   API → **+**, access **Developer**. Download the `.p8` and note its Key ID
   and the Issuer ID.
3. Optional, to ship the virtual camera: register the App ID
   `app.cutaway.Cutaway` with the **System Extension** capability and make a
   **Developer ID** provisioning profile for it.
4. Repository → Settings → Secrets and variables → Actions, add (files as
   base64, e.g. `base64 -i file | pbcopy`):

| Secret | What it is |
|---|---|
| `MAC_CERT_P12` | the `.p12`, base64 |
| `MAC_CERT_PASSWORD` | its password |
| `MAC_NOTARY_KEY_P8` | the `.p8`, base64 |
| `MAC_NOTARY_KEY_ID` | the key's Key ID |
| `MAC_NOTARY_ISSUER` | the Issuer ID |
| `MAC_PROVISION_PROFILE` | optional: the provisioning profile, base64; adds the virtual camera |

What it does (`macos/sign.sh`, which also works on your own Mac): signs with
the hardened runtime, a secure timestamp and `macos/Cutaway.entitlements`
(camera, microphone, and loading NDI's own runtime), sends the app to Apple's
notary service, staples the approval on, and checks it with Gatekeeper
(`spctl`), before and after the zip round trip. Any failure stops the
release, so a half-signed app can't be published. The update fingerprints
(`SHA256SUMS.txt`) are made from the signed zips, so signing updates works
the same as before.

When the first signed release goes out, change the "Open Anyway" steps in
README.md, docs/guide/install.md and docs/guide/first-hour.md for the Mac.
Existing users see one last Keychain prompt (and may need to allow camera,
microphone and screen recording once more), because to macOS the signed app
is a new app; after that, updates are quiet.

## What has been tested

These were tested on Linux, in a sandbox with test sources, software encoders
and a software GPU:

- **Recording:** 3 sources at different frame rates (25, 30 and 60 fps), one
  with a video delay, plus a 4-channel input (split into mono WAVs) and a
  delayed mono input. Every file was exactly the recording length, and a flash
  and beep every second land on the same frame in every file.
- **Backup copies, proxies, timecode, markers, chapters and timelines:** a
  take with all of them on; the backup files match the originals, and the
  timelines were checked against the cut list.
- **Network sources and the media player:** an RTMP sender, an SRT sender and
  a media playlist recorded together; picture and sound stay within a frame
  of each other.
- **Drift correction:** a simulated interface 80 ppm fast stayed within 1 ms
  over 10 minutes.
- **Streaming:** to local RTMP and SRT servers at once, while recording. Audio
  and video in the received streams line up within 1 ms. A server that
  dropped mid-stream was reconnected automatically.
- **Crash recovery:** a recording killed mid-recording was recovered; every file
  played to within about a second of the end.
- **Remote control:** the web remote, tally pages and every command, from a
  browser and from scripts; the WebSocket API; OSC commands and feedback.
- **Stream Deck plugin:** validated with Elgato's tools, and run against
  Cutaway with a stand-in for the Stream Deck app: live key pictures, taking
  a scene, record with hold-to-stop, a Stream Deck+ dial setting a level,
  fade to black, the prompter, a PTZ preset, the settings panel, and a wrong
  PIN.
- **Auto-framing and background removal:** with a test picture of two people
  and a stand-in for Vision: close-ups follow the person on the left and on
  the right, the two-shot holds both, and the cut-out composites over slides.
  The framing logic (headroom, zoom limit, steadiness, smooth moves, going
  wide when people leave) has its own tests.
- **Captions, transcripts, input tracks and the podcast:** with a scripted
  recogniser: captions on the stream only, transcripts timed to the recording;
  Program files with a track per input; a podcast measured at −16 LUFS by
  FFmpeg, with chapters.
- **Lip-sync measurement:** a test picture that claps, from cameras 40 ms
  and 120 ms slow and inputs with their own delays; the suggested delays
  line everything up to within a frame.
- **Interface:** the main window, preview/program, the second screen, the
  pre-show check, health and settings, under a virtual display.
- **Remote guests:** a WebRTC client standing in for a browser joined by
  link, sending H.264 and Opus; its picture reached the guest's source and
  its voice the mixer. A real (Chromium) browser joined through the guest
  page and heard the show's mix back. (Chromium's open-source build has no
  H.264, so its picture wasn't tested; Chrome, Edge and Safari have it.) The
  Cloudflare tunnel couldn't be reached from the test machine.
- **Stingers:** a track-matte stinger over the program, with the scenes
  changing at its cut point; the matte, black-is-see-through and solid kinds
  checked pixel by pixel.
- **Instant clips, sound pads, ducking, mix-minus, talkback, timers and the
  rundown:** their own tests, and the Stream Deck stand-in pressing each new
  key against a running Cutaway.
- **Phone cameras and guest backups:** a WebRTC client sending only video
  through the phone page's path; the phone page in Chromium (which, lacking
  H.264, now says so plainly); a guest in Chromium recording a backup while
  Cutaway recorded, uploaded into the recording's Guests folder and checked with
  FFprobe; pieces out of order, repeated or for the wrong take refused.
  The QR code was read back from a screenshot.
- **The vertical version:** its layouts and framings in tests, the compositor
  drawing it pixel by pixel, and a recording recorded with `Program_Vertical`
  while streaming the program and the vertical version to two local RTMP
  servers at once.
- **NDI output:** the sender starts with the NDI runtime; receiving it back
  needs NDI's network discovery, which the test machine doesn't have.
- **PTZ:** the VISCA messages (and VISCA over IP's framing) byte by byte
  against the protocol, and virtual PTZ moves and presets.
- **Replay, fade to black, LUTs, noise reduction, tickers, comments and the
  prompter:** their own tests (a replay played back in slow motion; a LUT
  and a fade checked pixel by pixel; RNNoise turning a fan down by about 19
  dB under speech; Twitch and YouTube chat messages read).
- **Uploads:** Google sign-in (the browser coming back to Cutaway) and
  resumable uploads that fail part-way and carry on, against a stand-in for
  Google; a whole recording sent to a stand-in Drive. Google itself couldn't be
  reached from the test machine.
- **Adaptive bitrate:** the bitrate steps down and back up as a stand-in
  connection backs up and clears, and the encoder takes a new bitrate
  mid-stream.
- **Session files:** the DAWproject checked against the official schema;
  the Resolve script run (in LuaJIT, as in Resolve) against a stand-in for
  Resolve's scripting API: the clips it lays down, track by track, checked
  against a real take's switch log; Resolve itself couldn't be run.
- **Finding people with ONNX Runtime** (the Windows way), on Linux with ONNX
  Runtime 1.31: faces found in photos, and a person cut out over slides
  through the whole app.
- **Whisper captions:** the streaming logic (partial and settled words,
  timing, long speech) with a stand-in recogniser, and whisper.cpp itself
  loading and running a model.
- **Unit tests:** `cargo test` (about 380 tests, plus Windows-only ones that run there), including the compositor's
  styling, keying, person masks, transitions and graphics, the voice
  processing, loudness, limiters and leveller, automatic switching,
  auto-framing, captions, lip sync, the podcast, OSC, the WebSocket, the
  command language and the timeline exports.

**Not yet run on Windows:** everything Windows-specific (Media Foundation
capture, encoders and decoders, screen capture, WASAPI and ASIO, DirectML,
Whisper with a real model, the virtual camera) compiles for Windows but
hasn't run there. The GitHub workflow runs the tests and a test recording on
Windows; the Media Foundation tests there also measure Windows' AAC encoder
delay.

**On a real Mac:** Cutaway is used for real shows on an Apple Silicon Mac
(cameras, recording, switching), and NDI has been tested there and works.

**Not yet run on a Mac:** the other parts that only exist on macOS compile
against the macOS frameworks for Apple Silicon but haven't been run on real
hardware yet. Expect a little tuning on first contact; the status light
(Now and the Log) is the place to start:

- ProRes recording and VideoToolbox decoding (network sources, the media
  player).
- Headphone output.
- MIDI.
- The virtual camera (needs the signing step).
- Live captions (the Speech framework) and auto-framing and background
  removal (the Vision framework).
- The Stream Deck plugin in the real Stream Deck app.
- PTZ control on a real network.
- Phone cameras from a real iPhone or Android phone, and uploads to the real
  YouTube and Google Drive.
