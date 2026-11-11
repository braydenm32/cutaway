[← The manual](README.md)

# Your first hour

This page is everything worth knowing before your first show: what the
computer needs, getting Cutaway open, the permissions to allow, where your
files go, what Cutaway sends over the internet, and the problems people
most often hit in the first hour, with their fixes. Once Cutaway is open,
its **first-show guide** walks you through the rest, step by step.

## What your computer needs

| | Smallest | Comfortable (several cameras, 4K) |
|---|---|---|
| Mac | Apple Silicon (M1), 8 GB memory, macOS 13 | M1 Pro/Max or newer, 16 GB |
| Windows | Windows 11 64-bit, a graphics card with a video encoder (NVIDIA GTX 1050 or newer, AMD RX 5000 or newer, Intel 7th-generation graphics or newer), 8 GB memory | NVIDIA RTX, recent AMD or Intel Arc, 16 GB |
| Drive | An SSD with room for the show: about 6 GB an hour for each 1080p file | A fast external SSD for 4K and many files |
| Network (streaming) | Upload speed about 1.5 times the stream's bitrate | |

An Intel Mac won't run it. The pre-show check adds up what your show
records and says when it may be too much for the computer. How far a
computer goes is on [Performance](performance.md).

**Windows is new.** The Windows version is built from the same code as the
Mac one and passes the same tests and a test recording on every change, but
it hasn't been used on a Windows PC with real cameras yet. If something
doesn't work there, the log and a problem report (below) are the best help.

## Opening Cutaway the first time

- **Mac:** unzip, move **Cutaway.app** to Applications and open it. It's
  signed and checked by Apple, so it opens straight away. (Versions before
  0.9.12 weren't: for those, macOS refuses the first time, and **System
  Settings → Privacy & Security → Open Anyway** lets it in.)
- **Windows:** the Windows app isn't signed yet, so Windows warns you once.
  Unzip the whole folder anywhere (running it from inside the
  zip doesn't work) and run **Cutaway.exe**. When Windows says it "protected
  your PC", choose **More info → Run anyway**.
  - **No "Run anyway" button:** Smart App Control is on (**Windows Security
    → App & browser control → Smart App Control**). It blocks every unsigned
    app. Turning it off lets Cutaway run, but Windows can't turn it back on
    without a reset, so decide whether that's right for this PC.
  - **Cutaway.exe vanished, or "contains a virus":** Microsoft Defender
    sometimes flags new, unsigned apps. Open **Windows Security → Virus &
    threat protection → Protection history**, check it's Cutaway, choose
    **Allow on device** and unzip again.

## Permissions to allow

Cutaway asks for each of these the first time it needs it. Saying no
doesn't break anything else, and you can change your mind later.

| What | When it asks | If you said no |
|---|---|---|
| Camera and microphone | The first launch | Mac: System Settings → Privacy & Security → Camera (and Microphone). Windows: Windows Settings → Privacy & security → Camera (and Microphone) → Let desktop apps access… |
| Screen Recording (Mac) | The first screen or window source | System Settings → Privacy & Security → Screen Recording, turn Cutaway on, then reopen Cutaway |
| Local Network (macOS 15 and later) | The phone remote, a network source or a stream server on your network | System Settings → Privacy & Security → Local Network |
| Firewall | The same things, on Windows (and on a Mac with the firewall on) | Windows Security → Firewall & network protection → Allow an app through firewall, find Cutaway and tick **Private** |
| Speech Recognition (Mac) | Turning on live captions | System Settings → Privacy & Security → Speech Recognition |

## Where your files go

| | Mac | Windows |
|---|---|---|
| Recordings (each in a folder of its own) | `Movies/Cutaway` | `Videos\Cutaway` |
| Settings | `~/Library/Application Support/Cutaway/config.toml` | `%APPDATA%\Cutaway\config.toml` |
| Log | `~/Library/Logs/Cutaway/Cutaway.log` | `%LOCALAPPDATA%\Cutaway\Logs\Cutaway.log` |
| Upload queue | `uploads.json`, beside the settings | the same |
| Updates being put in | `Updates`, beside the settings | the same |
| Stream keys, passwords, your Google sign-in | The Keychain | Credential Manager |
| DaVinci Resolve script (once installed) | `~/Library/Application Support/Blackmagic Design/DaVinci Resolve/Fusion/Scripts/Utility` | `%APPDATA%\Blackmagic Design\DaVinci Resolve\Support\Fusion\Scripts\Utility` |

Change where recordings go in **Record & stream › Quality and files**. **More
› App › Diagnostics → Settings file → Show** opens the settings folder.

**Starting again or removing Cutaway:** **More › App › Diagnostics →
Remove my settings and keys…** closes Cutaway and removes the settings,
saved shows, stream keys and passwords, and the log (your recordings stay).
To remove Cutaway itself, also delete the app (or the Windows folder). On
Windows, remove the virtual camera first if you installed it (**Record &
stream › Screens & webcam → Remove**). More in [Privacy](privacy.md).

## What Cutaway sends over the internet

Nothing about you or your shows, unless you ask it to.

- **Update check:** once when it starts, Cutaway asks GitHub whether there's
  a newer version. Turn that off in **More › App › General**.
- **Streaming and uploads** go only where you set them up: your streaming
  services, and YouTube and Google Drive with your own Google sign-in.
- **Remote guests:** the first time you invite one, Cutaway downloads
  Cloudflare's small tunnel program so guests can reach you without router
  setup (or uses Tailscale, if you pick it). The guest page, the guest's
  name and their backup recording (if you turn that on) pass through
  Cloudflare's (or Tailscale's) tunnel; their live picture and sound come
  straight to your computer where the network allows. To find a way
  through each other's routers, both ends ask a public address server
  (Cloudflare's and Google's) what their internet address is.
- **Live captions** are worked out on your computer. On Windows the speech
  model downloads once, when you turn captions on.
- **Problem reports** are saved as a file for you to send. Cutaway never
  sends one itself.

Everything else stays on your computer: recordings, captions, markers and
the log. The log is kept for a month at most. Cutaway doesn't encrypt
recordings itself, so if the computer could be lost or shared, turn on
FileVault (Mac) or BitLocker (Windows). [Privacy](privacy.md) has the
whole list, and what to tell the people you record.

## The first hour's common problems

1. **A camera isn't listed.** Check the permission above, then **More ›
   App › Diagnostics → List devices again**. A camera another app is using
   (Zoom, FaceTime, OBS) may not open until that app lets go of it.
2. **A source says "no signal".** It's unplugged, asleep, or in use by
   another app. Cutaway reconnects by itself when it comes back. The status
   light (top right) says which one.
3. **No sound in the recording.** The meter for your mic doesn't move: pick
   the right input in **Sound › Inputs** and check its **ON** button is green.
   Cutaway warns you after 45 seconds of silence while recording, and shows a
   small **no sound** beside REC.
4. **A screen source is black (Mac).** Turn Cutaway on in **Screen
   Recording** (above), then quit and reopen Cutaway.
5. **Keys do nothing.** If the top bar says **⌨ Typing**, a box has the
   keyboard: press Esc. On a MacBook, F1–F12 change brightness and volume
   unless you hold **Fn**.
6. **The phone remote won't connect.** The phone must be on the same Wi-Fi.
   Allow Local Network and the firewall (above), and type the PIN shown in
   **More › Control › Phone remote**.
7. **A stream service refuses the key.** Copy a fresh key from the service
   (YouTube Studio, say) into **Record & stream › Streaming**. Cutaway stops
   retrying a refused key and says so.
8. **A guest can't connect.** A few office, school and hotel networks block
   direct connections. Set a relay under **Strict networks (TURN)**, in the **TURN server** box, in the guest
   settings (see [Remote guests](guests.md)).
9. **The drive is filling up, or the recording stops at 4 GB.** The time
   left shows beside REC. A drive formatted as FAT32 can't hold a file over
   4 GB, and Cutaway warns before you record. Record to another drive, or
   reformat this one as APFS or exFAT (Mac) or NTFS or exFAT (Windows).
10. **Something else.** Click the status light: **Now** lists what's wrong,
    and **Log** has everything Cutaway noted. More on
    [what to do when something goes wrong](getting-started.md#if-something-goes-wrong).

## Getting help

**More › App › Diagnostics → Save a problem report…** saves one .zip with
the log, the last recording's note, your settings and the devices found.
Stream keys, passwords, tokens and the remote's PIN are taken out, even
where they appear in the log, and so are your Google account and your user
folder's name in file paths. Guests' names can still be in the log: look
through it before you send it if that matters. Send it along with what you were doing when it went wrong.

## Licences

Cutaway includes these pieces from others, each under its own licence:

- The symbol fonts are cut-down Noto fonts, under the SIL Open Font License
  ([assets/fonts](../../assets/fonts/README.md)).
- Auto-framing and background removal on Windows use the YuNet (MIT) and
  PP-HumanSeg (Apache 2.0) models ([models](../../models/README.md)).
- Live captions on Windows use [whisper.cpp](https://github.com/ggml-org/whisper.cpp)
  (MIT), and ONNX Runtime (MIT) runs the models above.
- Noise reduction uses RNNoise, through [nnnoiseless](https://github.com/jneem/nnnoiseless) (BSD).
- NDI® is a registered trademark of Vizrt NDI AB.
