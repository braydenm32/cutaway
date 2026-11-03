[← The manual](README.md)

# Installing Cutaway

## Install on a Mac

Needs a Mac with Apple Silicon (M1 or later) and macOS 13 Ventura or newer.

1. **Command line tools:** in Terminal, run `xcode-select --install` (skip if
   you already have Xcode).
2. **Rust:** install it from https://rustup.rs, then add the Apple Silicon
   target:
   ```sh
   rustup target add aarch64-apple-darwin
   ```
3. **Build the app:** inside this folder, run:
   ```sh
   ./macos/bundle.sh --install
   ```
   This builds `Cutaway.app` and copies it to Applications. Without
   `--install` it stays in `target/Cutaway.app`.

   **Or download it ready-made** from the
   [Releases page](https://github.com/braydenm32/cutaway/releases/latest)
   (see [Download](../../README.md#download)). Builds of every change are also kept under
   **Actions → macOS →** a run **→ Artifacts**. It isn't signed by an Apple
   developer, so macOS refuses the first time: go to **System Settings →
   Privacy & Security** and choose **Open Anyway** (once per new build).
4. **Permissions:** on first launch, macOS asks for camera and microphone
   access. Allow both. For screen sources, also turn Cutaway on in **System
   Settings → Privacy & Security → Screen Recording**, then reopen the app.
   The first time you use the phone remote, a network source or a stream
   server on your network, macOS 15 and later ask for **Local Network**
   access, and the firewall (if it's on) asks about incoming connections.
   Allow both. Turning on live captions asks for **Speech Recognition**.

You can also run it straight from Terminal, which is handy for the
command-line options below:

```sh
cargo run --release -- --list-devices
cargo run --release
```

### Virtual camera

On Windows 11, see the Windows install steps: **Record & stream › Screens & webcam ›
Install…** asks for administrator rights once.

On a Mac, the virtual camera isn't in the download yet, so there's no
**Webcam** button and Record & stream › Screens & webcam says it's coming later.
macOS only runs camera extensions from apps signed with a paid Apple
Developer account, which the download isn't yet. If you build Cutaway
yourself and have such an account, this is the one-time signing step:

1. In your [Apple Developer account](https://developer.apple.com/account),
   create an App ID `app.cutaway.Cutaway` with the **System Extension**
   capability, and an App ID `app.cutaway.Cutaway.Camera`.
2. Create a **Developer ID** provisioning profile for `app.cutaway.Cutaway`
   and download it.
3. Build and sign:
   ```sh
   ./macos/bundle.sh
   ./macos/virtualcam/build.sh "Developer ID Application: Your Name (TEAMID)" TEAMID ~/Downloads/Cutaway.provisionprofile
   ```
4. Copy `target/Cutaway.app` to `/Applications` (camera extensions only load
   from there), open it, and choose **Record & stream › Screens & webcam ›
   Install…**. Approve the extension in System Settings if asked (Privacy &
   Security on macOS 13–14; General → Login Items & Extensions on macOS 15
   and later).
5. Press **Webcam** under the status light (top right). In the video-call app, choose the
   **Cutaway** camera, and your interface or a loopback device as the
   microphone.

### NDI

NDI is optional, and Cutaway doesn't include it. Choose **NDI** as a
source's type in **Sources › Network & phone feeds** and pick from the senders it
finds. To send the program (and, if you like, every source) to the network
as NDI, turn it on in **Record & stream › Screens & webcam › NDI output**; it shows
how many receivers are watching. PTZ cameras on NDI can be steered from
Cutaway (see [PTZ cameras](sources.md#ptz-cameras)).

If NDI isn't installed, those settings offer **Install NDI…**: it downloads
NDI's free runtime from NDI and opens its installer, which asks for your
password. Cutaway picks NDI up as soon as it's installed, without a restart,
and starts any NDI sources and NDI output that were waiting for it. The
pre-show check offers the same when a show uses NDI on a computer without
it. NDI Tools or the NDI SDK from [ndi.video](https://ndi.video/tools/) work
too. Nothing else changes if NDI isn't installed.

NDI® is a registered trademark of Vizrt NDI AB.

## Install on Windows

Needs Windows 11 (64-bit, Intel or AMD) and a DirectX 12 graphics card.
**The Windows version is new:** it's built from the same code as the Mac
version, and GitHub's Windows machines run its tests and a test recording on
every change, but it hasn't been used on a Windows PC with real cameras yet.
Expect some tuning on first contact; the log
(`%LOCALAPPDATA%\Cutaway\Logs\Cutaway.log`) and the status light (top
right) are the places to start.

1. **Tools, once:** [Rust](https://rustup.rs) and the
   [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
   with **Desktop development with C++** (it includes CMake). For ASIO
   interfaces, also LLVM: `winget install LLVM.LLVM`.
2. **Build:** in this folder, in PowerShell:
   ```powershell
   powershell -ExecutionPolicy Bypass -File windows\build.ps1
   ```
   The first time, it downloads ONNX Runtime (for auto-framing and
   background removal) and Steinberg's ASIO SDK. The result is
   `dist\Cutaway` (run `Cutaway.exe`) and a zip of it to copy to other PCs.
   `-NoAsio` builds without ASIO.
3. **Or download it ready-made** from the
   [Releases page](https://github.com/braydenm32/cutaway/releases/latest)
   (see [Download](../../README.md#download)). GitHub builds it on its Windows machines,
   after the tests and a test recording pass; builds of every change are
   also kept under **Actions → Windows →** a run **→ Artifacts**. That build
   leaves out ASIO (interfaces still work through Windows audio); build it
   yourself, as above, for ASIO.
4. **First launch:** an unsigned app makes Windows say it "protected your
   PC": choose **More info → Run anyway** (once). If cameras or microphones
   don't show, turn on **Windows Settings → Privacy & security → Camera** (and
   **Microphone**) → **Let desktop apps access…**.
5. **Firewall:** the first time you turn on the phone remote, a network
   source, the stream server or guests, Windows Defender Firewall asks
   whether Cutaway may accept connections. Tick **Private networks** and
   choose **Allow access**. If you pressed Cancel, the phone can't connect:
   go to **Windows Security → Firewall & network protection → Allow an app
   through firewall**, find Cutaway and tick **Private**.

Settings are in `%APPDATA%\Cutaway\config.toml`, recordings go to
`Videos\Cutaway`.

### What's different on Windows

| | Mac | Windows |
|---|---|---|
| Cameras, capture cards | AVFoundation | Media Foundation (webcams, USB/PCIe capture cards) |
| Screen | ScreenCaptureKit | Windows Graphics Capture |
| Encoding | VideoToolbox: H.264, HEVC, ProRes | The graphics card's H.264/HEVC encoder (NVIDIA, AMD, Intel), or Microsoft's software H.264. No ProRes |
| Decoding (players, network sources, replays, MJPEG webcams) | VideoToolbox | Media Foundation on the graphics card, or in software if the card can't (set `CUTAWAY_SOFTWARE_DECODE=1` to always use software) |
| Audio | CoreAudio | Windows audio (WASAPI) and **ASIO** (interfaces show as "ASIO: …", every channel) |
| Live captions | Apple's speech recognition | **Whisper**, on the PC: the speech model downloads once from Graphics › More ▾ › Captions (57 MB Fast, 181 MB Accurate) |
| Auto-framing, background removal | Apple's Vision | Open models (YuNet, PP-HumanSeg) on the graphics card through ONNX Runtime and DirectML |
| Virtual webcam | Not in the Mac download yet (coming later; a self-signed build can have it) | Windows 11's virtual camera: **Record & stream › Screens & webcam › Install…** asks for administrator rights once |
| Log | `~/Library/Logs/Cutaway` | `%LOCALAPPDATA%\Cutaway\Logs` |

Everything else (switching, graphics, the mixer, recording formats,
streaming, network sources, NDI, the remote, OSC, MIDI and the Stream Deck
plugin) is the same. Two small differences: Windows' AAC encoder tops out at
192 kbit/s, and HEVC network sources and clips need Microsoft's free **HEVC
Video Extensions** on some PCs.
