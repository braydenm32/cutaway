//! What each setting does, in a few sentences: shown when you hover a
//! setting's name (straight away on its ⓘ) in Settings. Looked up by the
//! page that's showing and the name as written there.

use super::Section;
use std::cell::Cell;

thread_local! {
    static PAGE: Cell<Option<Section>> = const { Cell::new(None) };
}

/// The page being drawn (None between pages).
pub(super) fn set_page(page: Option<Section>) {
    PAGE.with(|p| p.set(page));
}

/// The explanation for a setting on the page being drawn.
pub(super) fn get(name: &str) -> Option<&'static str> {
    let page = PAGE.with(|p| p.get())?;
    for_page(page, name)
}

pub(super) fn for_page(page: Section, name: &str) -> Option<&'static str> {
    use Section::*;
    let text = match (page, name) {
        // ---- General & shows
        (General, "Interface size") => "Makes everything in Cutaway's window bigger or smaller: text, buttons and monitors. Handy on a big screen across the room, or a small laptop.",
        (General, "Ask before") => "Asks you to confirm before a recording stops or a stream ends, so a stray click or key can't end the show.",
        (General, "Pre-show check") => "Before you record or go live, Cutaway checks that every source has a picture, the inputs have sound, the disk has room and the stream servers answer. Anything wrong is listed, and you can still go ahead.",
        (General, "Updates") => "When Cutaway starts, it asks GitHub whether there's a newer release and tells you if there is (nothing about you or your show is sent); Update now in that notice downloads and puts it in. With the second box ticked, Cutaway downloads it by itself and puts it in the next time you close Cutaway, keeping your settings and shows.",

        // ---- Video sources
        (Sources, "Name") => "What this source is called on its button, in the multiview and scenes, and in the name of its own file (ISO_01_Name).",
        (Sources, "Type") => "Where the picture comes from: a camera or capture card on this computer, a screen, window or app on this computer, a test picture, a media player, a network feed (SRT, RTMP or NDI), a phone, or a close-up or virtual camera cut out of another source.",
        (Sources, "Device") => "Which camera or capture card. If it's unplugged, the source waits and connects by itself when it's back.",
        (Sources, "Device modes") => "The sizes and frame rates the device says it can send. Picking one sets the capture size and frame rate to match.",
        (Sources, "Pattern") => "Which generated picture to show. Test pictures are for trying layouts and checking that sources line up; each flashes once a second.",
        (Sources, "Listen on port") => "The network port Cutaway listens on for this feed. Each SRT or RTMP input needs its own; the sender points at this computer's address and this port (shown below).",
        (Sources, "Address") => "The SRT address to pull the stream from: srt://host:port, with ?streamid=… if the sender wants one. Cutaway calls out to it, and reconnects if it drops.",
        (Sources, "Stream key") => "The key the sender has to use. Leave it empty to accept any stream; a key stops a stray stream from landing here.",
        (Sources, "NDI source") => "Which NDI source on your network: other computers running NDI Tools or OBS, NDI cameras and converters. Press Find to look. Needs the free NDI runtime, which Cutaway can install.",
        (Sources, "Cut from") => "The source this one is cut out of, usually a wide shot. From a 4K camera, a 1080p cut-out stays sharp. It runs at that source's frame rate and delay, and records like any source.",
        (Sources, "Zoom limit") => "How far into the picture it may go. Further is tighter but softer, as fewer of the camera's pixels fill the frame.",
        (Sources, "Moves to presets take") => "How long a glide to a saved preset takes. Slower looks like a camera operator; quicker gets there before the moment's gone.",
        (Sources, "Capture size") => "The size Cutaway asks the device for. If it can't do exactly this, Cutaway converts, so every file stays consistent. Bigger takes more processing and disk.",
        (Sources, "Output size") => "The size of the cut-out picture (and its own file).",
        (Sources, "Picture size") => "The size this source works at in Cutaway, and the size of its own file. Incoming pictures are scaled to fit.",
        (Sources, "Frame rate") => "Frames per second asked of the source. Match the program (or a multiple of it) for the smoothest motion; anything else is converted.",
        (Sources, "Buffer") => "How long network pictures wait before they're used, to ride out hiccups. More is steadier but further behind live; less is closer to live but may stutter on Wi-Fi.",
        (Sources, "Record to its own file") => "Also records this source on its own in every take (an ISO file), without graphics or picture adjustments, for re-cutting the show in an editor.",
        (Sources, "Delay") => "Holds this source back, to line it up with slower sources or with the sound. Measure lip sync (in Audio inputs) works it out with a clap.",
        (Sources, "Controlled by") => "How Cutaway moves the camera. Most network PTZ cameras speak VISCA (the camera's manual says which kind and port); NDI PTZ cameras can be moved through NDI itself.",
        (Sources, "Camera's address") => "The camera's IP address on your network (a port is optional), and its VISCA number: 1 unless cameras are daisy-chained.",
        (Sources, "Speed") => "How fast pans, tilts and zooms move from the PTZ panel, Stream Deck dials and the remote.",
        (Sources, "Mounted upside down") => "Flips the directions for a camera hung upside down, so left on the panel still moves the picture left.",
        (Sources, "Follow") => "Who the close-up keeps in shot: everyone, the closest person, or whoever's on the left, in the middle or on the right.",
        (Sources, "Shot") => "How tight it frames them: close-up is tightest, wide keeps more of the room.",
        (Sources, "Moves take") => "How long each reframe glides for. Longer is calmer, like a patient camera operator; shorter keeps up with people who move a lot.",
        (Sources, "Loop") => "After the last file, start again from the first.",
        (Sources, "At start") => "Start playing as soon as Cutaway starts, instead of waiting for Play.",
        (Sources, "When taken") => "Start from the beginning whenever a scene with this player goes on air, for videos that should always run from the top.",
        (Sources, "Show images for") => "How long each still image stays up before the next file.",
        (Sources, "Crop left / right") | (Sources, "Crop top / bottom") => "Trims the edges off this source's picture: black bars, a desk edge, a stand. The program only; its own file stays whole.",
        (Sources, "Mirror") => "Flips the picture left to right (in the program only).",
        (Sources, "Brightness") => "Lifts or lowers the whole picture.",
        (Sources, "Contrast") => "Spreads the darks and lights apart (more) or brings them together (less).",
        (Sources, "Saturation") => "How strong the colours are: 0% is black and white.",
        (Sources, "Colour LUT") => "A .cube colour look-up table applied to this source: for a camera shooting log (S-Log, V-Log, C-Log, BRAW…) use the maker's log-to-Rec.709 LUT, or use any creative look. Strength blends it in.",
        (Sources, "Remove background") => "Cuts the people out of the picture without a green screen, so the scene's background or the sources layered below show behind them. Done on this computer: Apple's Vision on a Mac, a small model on the graphics card elsewhere. Best edges is finer round hair but busier.",
        (Sources, "Green / blue screen") => "Makes the colour of a real green or blue screen behind the subject see-through (a chroma key).",
        (Sources, "Screen colour") => "The colour to remove: use the green or blue button, or pick the exact colour.",
        (Sources, "Amount") => "How much around the screen colour goes see-through. Raise it until the screen's gone; too far and the subject starts to go too.",
        (Sources, "Edge softness") => "Feathers the edge between the subject and the background, so it isn't jagged.",
        (Sources, "Spill removal") => "Takes out the green or blue glow the screen casts onto hair and shoulders.",

        // ---- Audio inputs
        (Audio, "Name") => "What this input is called on its strip and in the mixes, and in its WAV file's name.",
        (Audio, "Device") => "Where the sound comes from: an audio interface or loopback device, the sound of a media player or network source (\"Sound from …\"), another app's sound, the sound pads, or a test tone.",
        (Audio, "Channels to capture") => "How many of the device's channels to take in. Every one is recorded to the WAV; pick which ones feed the mix below.",
        (Audio, "Program left / right") => "Which captured channels make the left and right of the mix. Pick the same channel for both to put a mono mic in the middle.",
        (Audio, "Gain") => "Level before everything else (processing, the fader and the mixes). Set it so speech peaks well below 0 dB on the meter.",
        (Audio, "On") => "Whether this input is heard in the mixes at all, the same as its ON button. Its WAV records either way.",
        (Audio, "Heard in") => "Which mixes it goes into. Leave music out of the recording to add it in the edit, or keep a talkback mic out of the stream.",
        (Audio, "Auto-mix") => "Makes this mic one of the ones the automatic mixer shares out: up while this person talks, down while someone else does. Turn automatic mixing on in Mixing & headphones.",
        (Audio, "Duck under voices") => "Turns this input down by itself while other inputs are talking: for music, pads and ambience. Set up ducking in Mixing & headphones.",
        (Audio, "Record to WAV") => "Saves this input on its own as a WAV in every take, untouched by processing, the fader or the ON button.",
        (Audio, "One file per channel") => "Saves each captured channel as its own mono WAV instead of one file with every channel, which is easier for multitrack editing.",
        (Audio, "Delay") => "Holds this input back to line it up with the pictures. Cameras usually run a few frames behind the sound: start around 60–100 ms, or use Measure lip sync.",
        (Audio, "Process this input") => "A voice channel strip: low cut, noise reduction, gate, EQ, compressor and de-esser. It shapes what the recording and stream hear; the WAV stays untouched, so you can redo it in editing. Off by default, as a DAW's output is usually processed already.",
        (Audio, "Low cut (rumble)") => "Removes rumble below this frequency: traffic, air conditioning, desk bumps. 80–100 Hz suits most voices.",
        (Audio, "Noise reduction") => "Reduces steady background noise such as fans, hum and traffic. More removes more, but too much makes voices sound watery.",
        (Audio, "Noise gate") => "Silences the input while it's quieter than this, so an open mic doesn't add room noise. Set it just above the noise and below the softest speech.",
        (Audio, "EQ") => "Tone: low (warmth), mid (presence, at the frequency you choose) and high (air). A few dB goes a long way.",
        (Audio, "Compressor") => "Evens out loud and quiet speech: above the threshold, the level is reduced by the ratio, and make-up gain brings everything back up.",
        (Audio, "De-esser") => "Softens harsh 's' and 'sh' sounds louder than this.",

        // ---- Remote guests
        (Guests, "Guests") => "Turns on the guest page, so people can join from a browser with a link: no app, no account. Each joins a guest spot below.",
        (Guests, "Reached through") => "How guests' browsers find Cutaway. The free Cloudflare tunnel needs nothing set up, but its address is new each time Cutaway starts, so send links on the day. Or use your own https:// address forwarded to this computer.",
        (Guests, "Port") => "The port the guest page runs on, on this computer. With the tunnel only this computer uses it; with your own address, forward it here.",
        (Guests, "Guests see") => "Sends guests a small copy of the program, so they can see what's on air without much of their connection.",
        (Guests, "Backup recordings") => "While you record, each guest's browser also records their camera and mic as captured, before the internet gets at it, and sends it in pieces into the take's Guests folder. It uses more of their upload, so it suits decent connections; their page tells them it's happening.",
        (Guests, "Screen share") => "Adds a source for the guest's screen. Their page gets a Share screen button: they choose the whole screen, a window or a tab, and it arrives on its own source while their camera stays on.",
        (Guests, "TURN server") => "A relay for guests on strict networks (some offices, schools and hotels) that block direct connections: Cloudflare Calls, Twilio or your own coturn. Most guests never need one.",
        (Guests, "Username") | (Guests, "Password") => "The TURN server's login.",

        // ---- Mixing & headphones
        (Mixing, "Limiter") => "Catches peaks, so the recording and stream mixes never clip. The ceiling is the loudest they can get; −1 dBFS is a safe choice.",
        (Mixing, "Stream loudness") => "Levels the stream mix slowly toward this loudness. YouTube, Spotify and most platforms play everything at about −14 LUFS, so this gets you there without riding faders. It works slowly (no pumping), and the recording stays as mixed.",
        (Mixing, "Output") => "The device your headphones are on. You hear the mix in sync with the program picture, a few frames behind live.",
        (Mixing, "Listen to") => "What you hear: the recording mix, the stream mix, or one input on its own. The headphone buttons on the audio strips solo an input quickly too.",
        (Mixing, "Volume") => "Headphone level only; no mix changes.",
        (Mixing, "Ducking") => "Turns music, pads and ambience down while people talk, and back up in the pauses. Tick which inputs go down; everything else counts as voices.",
        (Mixing, "Automatic mixing") => "Shares one level between the ticked mics, the way broadcast automixers do: each gets its share of how loud they all are, so the talker is up and the others drop away (up to 30 dB). Unlike a gate, nothing is ever switched off, so the start of a word is never lost.",
        (Mixing, "Mics") => "The mics to share out: people's mics, not music, pads or a DAW's mix.",
        (Mixing, "Turn down by") => "How far the ducked inputs go down while someone talks.",
        (Mixing, "When a voice is above") => "How loud a voice has to be to count as talking. Set it above the room noise on the mics.",
        (Mixing, "Back up after") => "How long a pause in the talking before ducked inputs come back up.",
        (Mixing, "These go down") => "The inputs that duck: usually music, the sound pads and Syrinscape.",
        (Mixing, "Your mic") => "The input you talk back on. Hold TALK (or B, or a Stream Deck Talkback key) and it goes only to outputs with talkback ticked: never to the recording or the stream.",
        (Mixing, "While talking back") => "Turns the rest down in those outputs while you talk, so you're heard.",
        (Mixing, "Guests hear it") => "Remote guests hear your talkback too.",
        (Mixing, "Name") => "What this output is called.",
        (Mixing, "Device") => "The audio device this output plays from.",
        (Mixing, "Channels") => "Which pair of the device's outputs it uses, and its volume.",
        (Mixing, "Mix") => "Which mix it starts from: the recording mix or the stream mix.",
        (Mixing, "Leave out") => "Inputs taken out of this output. For a caller, leave out their own voice (a mix-minus) so they don't hear themselves echo.",
        (Mixing, "Talkback") => "Whether your talkback is heard on this output.",

        // ---- Sound pads
        (Pads, "Key") => "The keyboard key that plays it: F1 to F12 for the first twelve pads.",
        (Pads, "Plays") => "Play once (press again to stop it early), loop until pressed again, or play only while the key or button is held.",
        (Pads, "Level") => "This pad's own volume, before the Sound pads fader.",
        (Pads, "Fade out") => "When it's stopped, it fades out over this long instead of cutting off.",

        // ---- Syrinscape
        (Syrinscape, "Control Syrinscape") => "Lets Cutaway press buttons in your Syrinscape Online player: from the sound panel, the Stream Deck, and scene and rundown cues. Needs a Syrinscape Online subscription, with the Online Player open in a browser or the app, on any computer.",
        (Syrinscape, "Auth token") => "Your personal token from syrinscape.com (Get yours). It works like Syrinscape's remote-control links, so keep it private.",

        // ---- Program
        (Program, "Size") => "The size of the program: what you record and stream. Changing it means stopping recording and streaming first. The stream can be sent smaller (in Streaming).",
        (Program, "Frame rate") => "Frames per second of the program: 25 or 50 in PAL countries, 30 or 60 elsewhere, 24 for a film look. Sources at other rates are converted.",
        (Program, "Background colour") => "What shows where no source covers the picture: around picture-in-picture frames, or behind people with the background removed.",
        (Program, "Background image") => "A picture behind everything instead of the colour (PNG or JPEG), filling the frame.",
        (Program, "Program monitor quality") => "How sharp the program monitor is in this window. Recordings aren't affected; lower it on a slower computer.",
        (Program, "Multiview quality") => "How sharp each source's monitor is in this window. Recordings aren't affected; lower it with many sources or a slower computer.",
        (Program, "Rounded corners") => "Rounds the corners of frames that don't fill the screen (picture in picture, side by side, grids). Full-screen shots stay clean.",
        (Program, "Border") => "A line around those frames, and its colour.",
        (Program, "Drop shadow") => "A soft shadow under those frames, so they lift off the background.",

        // ---- Graphics
        (Graphics, "Name") => "What the graphic's button says. The first nine also have Alt+1 to Alt+9.",
        (Graphics, "Text") => "The ticker's items, one per line; they scroll past in turn, again and again. Change them live from the ticker's button (✎).",
        (Graphics, "Label") => "A few words in front, like \"Coming up\" on a ticker or \"Starting in\" on a countdown. Optional.",
        (Graphics, "Speed") => "How fast the ticker scrolls, in picture widths a minute.",
        (Graphics, "Address") => "The web page to show: an address (like a StreamElements or Streamlabs alert or chat box) or an HTML file on this computer. It's see-through wherever the page has no background.",
        (Graphics, "Page size") => "The size the page is laid out at, in pixels: usually the program's size, for a page made for a whole screen. Width (below) sets how big it is on the picture.",
        (Graphics, "Drawn by") => "Web pages are drawn by Chrome or Edge, run hidden in the background (nothing opens on screen). Install one if none is found.",
        (Graphics, "Image") => "The picture to show. A PNG with a transparent background works best for logos.",
        (Graphics, "Name line") => "The title's top line, usually a name. Change it live from the title's button (✎), handy between guests.",
        (Graphics, "Second line") => "The line underneath: a role, a topic or a handle.",
        (Graphics, "Shows") => "What the clock shows: the time of day, a countdown to a time, a countdown, a stopwatch, or the time left in the rundown segment.",
        (Graphics, "Format") => "24-hour or 12-hour time, with or without seconds.",
        (Graphics, "Counting down to") => "The time of day it counts down to, like 19:30 or 7:30 pm, by this computer's clock.",
        (Graphics, "From") => "Where the countdown starts. Start, pause and reset it from its button (right-click), a Stream Deck Timer key, or the timer commands.",
        (Graphics, "Colours") => "The text colour, the box colour, and how solid the box behind the text is.",
        (Graphics, "Font") => "A font file (TTF or OTF) for the text, or the built-in one.",
        (Graphics, "Position") => "Where the graphic's top-left corner sits: across and down, as a share of the picture.",
        (Graphics, "Corner") => "Puts it in a corner in one click.",
        (Graphics, "Width") => "How wide it is, as a share of the picture; the height follows.",
        (Graphics, "Opacity") => "How solid it is. Lower lets the picture show through, as for a watermark.",
        (Graphics, "Comes on") => "How it arrives and leaves: a fade, a slide in from the nearer side, a wipe from left to right, a gentle rise, or a pop. And how long that takes (0 for straight on and off).",
        (Graphics, "Showing at start") => "Whether it's on screen when Cutaway starts. Its button (or Alt + its number) turns it on and off.",

        // ---- Captions
        (Captions, "Live captions") => "Turns speech into captions on the program, live, about a second behind. Recognition runs on this computer; nothing is sent anywhere. The operator sees the captions over the program monitor either way.",
        (Captions, "Status") => "Whether recognition is running, or why not.",
        (Captions, "Listen to") => "What's captioned: the recording mix, the stream mix, or one input (the presenter's mic gives the cleanest captions).",
        (Captions, "Language") => "The language spoken. On a Mac, if on-device recognition isn't available for it, turn on Dictation for that language in System Settings → Keyboard, which downloads it.",
        (Captions, "Speech model") => "OpenAI's Whisper model, downloaded once from Hugging Face. Fast suits any recent computer; Accurate makes fewer mistakes but needs a strong processor.",
        (Captions, "Show on") => "On the stream only, the recording stays clean and gets caption files instead (import the .srt into your editor or YouTube). Or put them on both.",
        (Captions, "On screen at start") => "Whether captions are showing when Cutaway starts. The CC button and the C key turn them on and off.",
        (Captions, "Look") => "Text size, how many lines show at once, and whether they sit at the bottom or the top.",
        (Captions, "Transcript") => "Saves what was said with every take: Transcript.txt, and Captions.srt and Captions.vtt for an editor or YouTube.",

        // ---- Switching
        (Switching, "Transition") => "How scenes change. Cut is instant; Mix glides frames to their new places and cross-fades; Wipe and Push bring the new picture in from a side; Dip goes through a colour; Stinger plays a video over the change. T switches between Cut and this.",
        (Switching, "Length") => "How long a transition takes.",
        (Switching, "Wipes and pushes go") => "Which way wipes and pushes move.",
        (Switching, "Dip through") => "The colour a Dip passes through between scenes.",
        (Switching, "Preview, then take") => "Pick the next scene in a preview monitor first, then send it to air with Take (Space) or Cut (Enter). Without it, clicking a scene takes it straight to air.",
        (Switching, "Video") => "The stinger: a short video (a logo sweep, a burst of colour) that plays over the program while the scenes change underneath. Pick Stinger as the transition to use it; automatic switching mixes instead.",
        (Switching, "See-through parts") => "How the stinger's transparency is stored. Most stinger packs have a \"track matte\" version, with the picture and a black-and-white matte side by side or one above the other, which works on any computer. Automatic works it out.",
        (Switching, "Change scenes at") => "When, from the start of the stinger, the scenes change underneath: the moment the video covers the screen. 0 changes halfway.",
        (Switching, "Its sound") => "The stinger's volume. It plays through the Sound pads input.",
        (Switching, "Fade length") => "How long Fade to black takes. FTB in the bottom bar (or ftb from the Stream Deck, MIDI or the remote) fades everything, graphics included; press it again to come back.",
        (Switching, "Sound fades too") => "The recording and the stream fade out with the picture. Headphones and callers keep hearing.",
        (Switching, "Replay the last") => "How much of the program a replay plays again. Press ⟲ Replay in the header while recording (or replay from the Stream Deck, MIDI or the remote); it fades in over the live picture while the show's sound carries on.",
        (Switching, "Shows") => "What ⟲ Replay plays again: the program as it went out, one camera (from its own recording, so a moment the program missed), or up to four cameras at once in a grid. Right-click ⟲ Replay to pick another each time. Cameras need \"Record to its own file\" on, and files that aren't ProRes.",
        (Switching, "Slow motion") => "The speed of a slow-motion replay (right-click ⟲ Replay, or replay:slow).",
        (Switching, "Label") => "Shows REPLAY in the corner while a replay plays.",
        (Switching, "At start") => "Whether automatic switching is on when Cutaway starts. The AUTO button and the A key turn it on and off.",
        (Switching, "Counts as talking above") => "How loud a mic has to be to count as talking. Set it just above the level each mic shows while that person is quiet (the live levels beside the rules help).",
        (Switching, "Talking for at least") => "How long someone has to talk before the shot changes to them, so a cough doesn't cut.",
        (Switching, "Shortest shot") => "The least time a shot stays up before it can change again.",
        (Switching, "Wide shot") => "A scene with everyone in it, for when several people talk at once or one person has been on for a long time.",
        (Switching, "When several talk at once") => "Go to the wide shot when more than one person talks, instead of following the loudest.",
        (Switching, "After one person for") => "After this long on one person, cut to the wide shot for a moment (never: stay on them).",
        (Switching, "After you switch by hand") => "How long automatic switching waits after you take a scene yourself.",

        // ---- Rundown
        (Rundown, "Name") => "What the segment's called: on the rundown, the presenters' clock and the recording's chapters.",
        (Rundown, "Length") => "How long it's planned to run. It counts down and turns red when it runs over; 0 is open-ended and counts up.",
        (Rundown, "Scene") => "The scene to take when the segment starts, or stay on the one that's on.",
        (Rundown, "Cues") => "Commands run as the segment starts: show a title or clock, play a pad, start a Syrinscape mood or a countdown, mute a mic… The scene's own cues run too.",
        (Rundown, "Notes") => "Talking points, who's on, what to set up: shown with the rundown in the main window, and on the teleprompter when it follows the rundown.",
        (Rundown, "Chapters") => "Adds a marker as each segment starts, so it becomes a chapter in the files and the podcast.",

        // ---- Macros
        (Macros, "Name") => "What the macro's button says. Run it from its button, the Stream Deck (Macro key), the remote, MIDI, cues, or the command macro:Name.",
        (Macros, "Colour") => "The button's colour.",
        (Macros, "Steps") => "Commands run in order; a wait pauses before the next. A macro can run another macro.",

        // ---- Teleprompter
        (Prompter, "Script") => "What the presenters read. A blank line starts a new paragraph (◀ and ▶▶ jump between them). Paste it, type it or load a text file.",
        (Prompter, "Rundown") => "Starts each part of the script with the rundown segment's name and notes, and jumps there as the rundown moves on.",
        (Prompter, "Speed") => "How fast it scrolls, in words a minute. 140–160 suits most presenters; it can be changed live.",
        (Prompter, "Text size") => "The text's height, as a share of the screen it's on.",
        (Prompter, "Mirror") => "Flips it left to right, for a prompter with glass in front of the camera.",
        (Prompter, "Colours") => "The text and background colours.",

        // ---- Viewer comments
        (Comments, "Twitch channel") => "Your Twitch channel's name, to read its chat. No sign-in needed.",
        (Comments, "YouTube stream") => "The address of your live stream (watch?v=… or /live/…), to read its chat.",
        (Comments, "YouTube API key") => "Reading YouTube chat needs a key. It's free: in Google Cloud Console, make a project, turn on \"YouTube Data API v3\", then Credentials → Create credentials → API key.",
        (Comments, "Position") => "Where the comment's top-left corner sits on the picture.",
        (Comments, "Width") => "How wide the comment box is.",
        (Comments, "Colours") => "The text and box colours, and how solid the box is. The commenter's name shows in their chat colour (Twitch) or the platform's.",
        (Comments, "Fade") => "How long a comment takes to fade in and out.",
        (Comments, "Hide after") => "Hides the comment by itself after this long (0: it stays until you hide it).",

        // ---- Recording
        (Recording, "Save takes in") => "The folder where each take gets a folder of its own. Use a fast drive with plenty of room.",
        (Recording, "Take name") => "The start of each take's folder name; the date and time follow.",
        (Recording, "File format") => "MP4 opens everywhere; MOV suits Final Cut Pro. ProRes always records as MOV.",
        (Recording, "Codec") => "H.264 plays anywhere; H.265 makes smaller files (good for 4K); ProRes (on a Mac) is the easiest to edit, but the files are very large.",
        (Recording, "Encoder") => "What encodes the video: the computer's built-in hardware encoder when there is one (fast, and leaves the processor free), or software.",
        (Recording, "Source file bitrate") => "The quality of each source's own file. Automatic sets it from each source's size and frame rate; higher is better and bigger.",
        (Recording, "Program bitrate") => "The quality of the program file. Automatic sets it from the program's size and frame rate.",
        (Recording, "Audio in video files") => "The quality of the sound inside the video files (AAC). 192–256 kbps is plenty.",
        (Recording, "Program audio in source files") => "Puts the recording mix in every source's own file too, so editors line the angles up by sound automatically.",
        (Recording, "WAV files") => "The bit depth of the WAV files. 24-bit suits most; 32-bit float can't clip, even if a level was too hot.",
        (Recording, "Backup copy") => "Records a second copy of every file into another folder as you go. Pick one on a different drive, so a full or failing drive can't lose the show.",
        (Recording, "Instant clips") => "The Clip button (or K) saves the last few seconds of the program into the take's Clips folder, to share straight away.",
        (Recording, "Crash protection") => "Files are written so that after a crash or a power cut they keep everything up to the last second. Next time Cutaway opens, it mends them and tells you when the take is safe.",
        (Recording, "Timecode") => "Stamps every file with the time of day as timecode, so editors can line the files up by it.",
        (Recording, "Editor timelines") => "Saves the live edit as a multicam timeline for Final Cut Pro, DaVinci Resolve and Premiere Pro, with every angle and your cuts.",
        (Recording, "Audio sessions") => "Saves a REAPER session and a DAWproject (Fender Studio Pro, Bitwig, Cubase) with every WAV on its own track, lined up.",
        (Recording, "DaVinci Resolve") => "Saves what Cutaway's Resolve script needs to build a whole project from the take. Install the script once; then in Resolve: Workspace → Scripts → Cutaway - Build project from a take.",
        (Recording, "Proxy files") => "Also records small copies of the video files for editing on a laptop; the editor switches to the full files for the final export.",
        (Recording, "Each input on its own track") => "Adds every input as its own audio track in the Program file, after the mix. Players play the mix; editors (Premiere, Resolve, Final Cut) see every track, named after its input, for rebalancing.",
        (Recording, "Input tracks") => "Before the fader: each input as it came in (with its processing), so nothing's lost if a mic was down or off. As heard: what went into the mix.",
        (Recording, "Mix WAV") => "Also saves the recording mix as Mix.wav, for audio editing. The podcast copy is made from it.",
        (Recording, "Podcast copy") => "After each take, makes an .m4a of the mix, levelled for podcasts, with the markers as chapters. It's made in the background once the take is saved; a limiter keeps peaks under −1.5 dBFS.",
        (Recording, "Loudness") => "The loudness it's levelled to: −16 LUFS suits Apple Podcasts and Spotify; −19 suits mono speech.",
        (Recording, "Channels") => "Stereo or mono, and the bitrate. Mono at 64–96 kbps is plenty for speech.",
        (Recording, "Podcast name") => "The show's name, written into the file.",
        (Recording, "Episode title") => "The episode's title in the file; blank uses the podcast name and the date.",
        (Recording, "Author") => "Who made it, written into the file.",

        // ---- Streaming
        (Streaming, "Name") => "What this destination is called in Cutaway.",
        (Streaming, "Service") => "Where it streams to. Picking a service fills in its server; Custom takes any RTMP or RTMPS address; SRT sends to an SRT receiver.",
        (Streaming, "Server") => "The server address from the service (usually filled in already).",
        (Streaming, "Address") => "The SRT receiver's address: srt://host:port.",
        (Streaming, "Stream key") => "The secret key from the service's live dashboard. Anyone with it can stream to your channel, and it's kept in the settings file, so keep that private.",
        (Streaming, "Stream ID (optional)") => "An SRT stream ID, if the receiver asks for one.",
        (Streaming, "Go live here") => "Include this destination when you go live. Untick it to keep it set up but skip it.",
        (Streaming, "Sends") => "The program, or the 9:16 vertical version for YouTube Shorts and TikTok (turn it on in Vertical version).",
        (Streaming, "Size") => "The size sent to every destination. Smaller than the program saves upload: 1920×1080 is typical, 1280×720 on a slow connection.",
        (Streaming, "Frame rate") => "Frames per second sent. 30 is typical; 60 for fast motion, if your upload allows.",
        (Streaming, "Video bitrate") => "The stream's picture quality, and the upload it needs for each destination. Keep the total under about 70% of your measured upload speed.",
        (Streaming, "Audio bitrate") => "The stream's sound quality. 160 kbps is plenty for most shows.",
        (Streaming, "Encoder") => "What encodes the stream: always H.264, which every service accepts. The hardware encoder leaves the processor free.",
        (Streaming, "If a connection drops") => "Reconnects a destination by itself if its connection drops. A destination that fails never affects the others, the program or your recordings.",
        (Streaming, "Give up after") => "How many reconnection attempts before giving up on a destination (0: keep trying).",
        (Streaming, "When going live") => "Starts recording too whenever you go live.",
        (Streaming, "Slow connection") => "Lowers the video bitrate while the connection can't keep up, and brings it back after (adaptive bitrate): fewer freezes for viewers.",

        // ---- Vertical version
        (Vertical, "Make it") => "Makes a 9:16 version of the show alongside the program, from the same sources, shown beside the program monitor. For Shorts, Reels and TikTok.",
        (Vertical, "Size") => "1080×1920 is full HD; 720×1280 is lighter on the computer.",
        (Vertical, "Record") => "Saves it into each take as Program_Vertical, with the recording mix.",
        (Vertical, "Stream") => "To stream it, set a destination in Streaming to send the vertical version (YouTube Shorts and TikTok take it); the others keep the program.",
        (Vertical, "Graphics") => "Puts the program's titles, logos and tickers on it too, across its width: ones at the top stay near the top, the rest sit low down.",

        // ---- Upload
        (Upload, "Client ID") | (Upload, "Client secret") => "From the OAuth client (Desktop app) in your own Google Cloud project: see \"Setting up your Google sign-in\" above. They let Cutaway sign in as an app you control.",
        (Upload, "Google account") => "The account takes are uploaded to. Signing in opens Google's page in your browser.",
        (Upload, "YouTube") => "Uploads the program to YouTube after each take.",
        (Upload, "Visibility") => "Who can watch it. YouTube keeps videos from projects Google hasn't audited private; make them public yourself in YouTube Studio.",
        (Upload, "Title") => "The video's title; blank uses the show's name and the date. {show} and {date} are filled in.",
        (Upload, "Description") => "The video's description.",
        (Upload, "Chapters") => "Adds the take's markers to the description as chapters (YouTube needs at least three, ten seconds apart).",
        (Upload, "Google Drive") => "Uploads files from each take to Google Drive. Cutaway only sees the Drive files it makes.",
        (Upload, "What") => "Which files: the program; the program, the vertical version and the mix; or every file in the take.",
        (Upload, "Folder") => "A folder in My Drive; each take gets its own folder inside it.",
        (Upload, "When") => "Upload as each take is saved, or wait for you to press Upload. Uploads go one at a time in the background, and pick up again if the connection drops.",

        // ---- Screens & webcam
        (Outputs, "Show") => "What the second-screen window shows: the program, the multiview or the teleprompter. It opens on the screen chosen below. Double-click it (or press F) to fill the screen; Esc leaves full screen, and Esc again (or its ✕) closes it. The header's Screen button turns it on and off.",
        (Outputs, "Full screen") => "Opens it filling its screen. Never over Cutaway itself: on the screen Cutaway is on (or with only one screen) it opens in a window.",
        (Outputs, "Clocks") => "Shows the time and the rundown segment's time left, on this screen only: for presenters, not viewers.",
        (Outputs, "Program") => "Sends the program, with the recording mix, as an NDI source that other computers, OBS, vMix, NDI Studio Monitor and NDI recorders on the network can use.",
        (Outputs, "Name") => "Its NDI name (NDI adds this computer's name in front).",
        (Outputs, "Sources") => "Also sends every source on its own, as NDI sources named after it.",
        (Outputs, "At start") => "Turns the virtual camera on when Cutaway starts, so video-call apps can pick the \"Cutaway\" camera straight away.",

        // ---- Remote & MIDI
        // ---- Graphics taking turns
        (Graphics, "Group name") => "What this group is called on its button beside the graphics, and in commands (rotation:Sponsors).",
        (Graphics, "Graphics") => "The graphics that take turns, in order. When the last one has been on, it starts again from the first.",
        (Graphics, "How often") => "How often the next graphic comes on, and how long it stays. If it stays on longer than the gap, they follow straight on from each other.",
        (Graphics, "Runs") => "When it starts and stops by itself. Its button beside the graphics starts or stops it any time, and so do the remote, Stream Deck, MIDI and keys (rotation:Name).",

        (Control, "Remote control") => "Lets phones, tablets, the Stream Deck plugin, Companion and scripts on this network run the show, through a web page and web addresses.",
        (Control, "Port") => "The port the remote runs on.",
        (Control, "PIN") => "A code needed to use the remote. Without one, anyone on the network could control the show.",
        (Control, "OSC") => "Accepts OSC commands over the network: QLab, Companion, TouchOSC, lighting desks.",
        (Control, "Listen on UDP port") => "The port OSC commands arrive on.",
        (Control, "Send state to") => "Where Cutaway sends its state as OSC whenever it changes (scenes, tally, recording, levels), so a controller can show it. Optional.",
        (Control, "MIDI control") => "Lets MIDI controllers' pads, keys and faders run commands. Map them below with Learn.",
        (Control, "Controller") => "Which MIDI device to listen to, or every one connected.",
        (Control, "Light up pads") => "Lights pads on controllers that support it, to show what's on air.",

        // ---- Diagnostics
        (Diagnostics, "Version") => "The Cutaway version you're running.",
        (Diagnostics, "Log file") => "What Cutaway has been doing. Send this file along when reporting a problem.",

        _ => return None,
    };
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every setting's name used on a page (written as label(ui, "…")) has
    /// an explanation for that page.
    #[test]
    fn every_setting_is_explained() {
        use Section::*;
        let files: [(&str, &[Section]); 16] = [
            (include_str!("mod.rs"), &[General, Audio, Program, Recording, Streaming, Diagnostics]),
            (include_str!("sources.rs"), &[Sources]),
            (include_str!("mixing.rs"), &[Mixing, Audio]),
            (include_str!("switching.rs"), &[Switching]),
            (include_str!("graphics.rs"), &[Graphics]),
            (include_str!("captions.rs"), &[Captions]),
            (include_str!("comments.rs"), &[Comments]),
            (include_str!("control.rs"), &[Control, General]),
            (include_str!("guests.rs"), &[Guests]),
            (include_str!("macros.rs"), &[Macros]),
            (include_str!("outputs.rs"), &[Outputs]),
            (include_str!("prompter.rs"), &[Prompter]),
            (include_str!("rundown.rs"), &[Rundown]),
            (include_str!("sound.rs"), &[Pads, Syrinscape]),
            (include_str!("upload.rs"), &[Upload]),
            (include_str!("vertical.rs"), &[Vertical]),
        ];
        let mut missing = Vec::new();
        for (f, pages) in files {
            for piece in f.split("label(ui, \"").skip(1) {
                let name = piece.split('"').next().unwrap();
                if !pages.iter().any(|s| for_page(*s, name).is_some()) {
                    missing.push(name.to_string());
                }
            }
        }
        assert!(missing.is_empty(), "no help for: {missing:?}");
    }
}
