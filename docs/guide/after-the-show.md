[← The manual](README.md)

# After the show: uploads and editing

## Upload after the show

**Record & stream › Upload** sends each recording to **YouTube** and/or **Google Drive**
once it's saved. It signs in with your own Google account, through an
OAuth client you make once in Google Cloud (the settings walk you through
it: a project, the YouTube Data API and/or Google Drive API turned on, and a
**Desktop app** client whose ID and secret you paste in). Then:

- **YouTube** gets the program, private, unlisted or public, titled with the
  show's name and the day it was recorded (or your own title), with the
  markers (not the clips) as chapters in the description.
- **Google Drive** gets the program, the program with the vertical version
  and the mix (and the podcast, when one's made), or the whole recording, in a
  folder per take (inside a "Cutaway" folder).
- An **Upload** button appears in the main window's Uploads panel (and on
  the Recording saved card) when a recording is saved (and its podcast made); send an
  older take from **Record & stream › Recordings**, or uploads start straight away if you choose. They go on in the
  background, one at a time, with their progress, and pick up again if the
  connection drops.
- **Quitting doesn't lose them.** The queue is kept in `uploads.json` beside
  the settings (what to send and where, and Google's upload session; never
  your sign-in, which stays in the Keychain / Credential Manager). Next time
  Cutaway opens, the Uploads panel says "2 uploads didn't finish": **Carry
  on** picks each up where Google got to (Drive files already sent aren't
  sent again; a session Google has expired, after about a week, starts that
  file again), or **Forget them**. It asks rather than starting by itself so
  an upload never takes your connection during a show.

Two things Google decides: while your Google Cloud app is in **Testing**,
Google signs Cutaway out after 7 days (publishing the app, which needs no
review for your own use, stops that); and YouTube keeps videos uploaded
through a project it hasn't audited **private**, so make them public in
YouTube Studio (or apply for the audit). Drive has no such limit, and
Cutaway only sees the Drive files it made.

## Editing your recordings

- **Final Cut Pro:** File → Import → XML, and pick the `.fcpxml`. You get a
  multicam clip of every recorded angle (and the program), and a project cut
  as you switched it live, with the markers, and every angle full length on
  its own lane above it (switched off; turn one on, or blade it, to use it).
  Switch angles in the angle viewer to change any cut.
- **DaVinci Resolve:** press **Open in Resolve** (on the Recording saved card or
  in Record & stream › Recordings). Cutaway installs (or updates) its Resolve script,
  notes the recording for it and starts Resolve, and the card shows the one step
  left: in Resolve, **Workspace → Scripts → Cutaway - Build project from a
  take** (if Resolve shows its Project Manager, open any project first).
  Within 15 minutes of Open in Resolve the script builds that recording straight
  away; later (or run on its own) it asks for the recording's folder, starting
  at the latest recording. With a project from the same show and day already open
  (the recording before a break), it asks whether to add this recording to it, in a
  bin of its own with its own timeline, or make a new project. You get a project named after the recording, at its frame
  rate and picture size, with bins for the cameras and the audio, proxies
  linked, and the timeline **… (live edit)**: your cuts on V1, every camera
  and the program full length on its own track above (V2 and up, named
  after the camera, switched off so the live edit plays; turn one on to
  use it), the mix on A1 and the markers. The script lays the timeline down
  clip by clip itself, so it doesn't depend on Resolve's XML import; if
  that doesn't work in your Resolve it imports the `.fcpxml` instead. Older
  recordings work too (Open in Resolve adds the cut list from the recording's
  Premiere timeline), and a recording moved or copied to another drive still
  works: pick it where it is now. Why the one step: the free DaVinci
  Resolve only runs scripts from its own menu, and Resolve's project files
  (`.drp`) are a closed format, so Cutaway can't hand it a finished project.
  If Resolve isn't installed in the usual place (`/Applications` on a Mac,
  `Program Files\Blackmagic Design` on Windows), the `.fcpxml`'s path is
  copied (File → Import → Timeline, and paste) and a line says so. The
  script can also be installed from Record & stream › Quality and files › Editing, where **a
  project from each recording** saves the file the script needs with every recording.
- **REAPER:** open the recording's `.rpp`: every WAV is on its own track, lined
  up, with the markers.
- **Fender Studio Pro (Studio One), Bitwig, Cubase:** open (or import) the
  take's `.dawproject`.
- **Premiere Pro:** File → Import, and pick the `(Premiere).xml`. The live
  edit is on V1, with every angle on its own track above, ready to swap in.
- **Composite scenes** (side by side, picture-in-picture) cut to the
  `Program` file; single-camera scenes cut to that camera's file.
- **Close-ups saved as a crop** (their **Record** row in Sources › Cameras:
  **as a crop of Cam 1**) have no file of their own: their angle and shots are the wide
  camera's file, zoomed and moved to the same shot (Final Cut: Transform,
  and Trim when the close-up isn't the timeline's shape; Resolve: the
  Inspector's Zoom, Position and Cropping; Premiere: Motion and Crop). Reframe
  them there. Final Cut gets their moves as keyframes; Resolve and Premiere
  get one framing per stretch, a move cutting to where it ended.
- **Proxies:** the `Proxies` folder has matching small files with the same
  names, timecode and length, for Final Cut's, Resolve's and Premiere's proxy
  workflows.
- **YouTube chapters:** paste `Chapters.txt` into the video description. It
  follows YouTube's rules (the first at 0:00, at least three, ten seconds
  apart), so it's only there when the markers make that; clips are on the
  timelines as markers but aren't chapters.
