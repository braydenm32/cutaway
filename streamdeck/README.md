# Cutaway for Stream Deck

Run Cutaway from an Elgato Stream Deck (any model, including Stream Deck+
dials and the Stream Deck app's virtual decks).

## Install

1. In Cutaway: **Settings → Remote & MIDI**, turn on **Remote control** and
   note the port (8790) and PIN.
2. Double-click **Cutaway.streamDeckPlugin** (or press **Install the Stream
   Deck plugin…** on that settings page). The Stream Deck app asks to
   install it. Needs Stream Deck 6.5 or later, on macOS 12+ or Windows 10+.
3. Drag a Cutaway action onto a key. In its settings, enter the address
   (`127.0.0.1:8790` when Cutaway runs on the same computer) and the PIN —
   once; every key shares them.

## Actions

| Action | Key | Notes |
|---|---|---|
| Scene | take, preview or cut to a scene | live picture of the scene, red border on air, green in preview |
| Source | a source full screen | live picture and tally |
| Take / Cut | preview to program | lit while something is in preview |
| Transition | cut, mix, wipe, push, dip | lit when it's the current one |
| Record | start; hold a second to stop | red with the running time |
| Go Live | hold to go live, hold to end | red with the running time; holds can be turned off |
| Marker | drop a marker (optional label) | |
| Graphic | show/hide a logo, title or lower third | lit while on |
| Audio Input | on/off, with a level meter | on a Stream Deck+ dial: turn for level, press or tap for on/off |
| Auto Switching | follow who's talking on/off | |
| Captions | live captions on/off | |
| Media Player | play/pause, next, restart, previous | shows the item playing |
| Command | any Cutaway command | e.g. `preview:2`, `graphic:Logo:on`, `gain:Mic:-6` |

Live pictures refresh once a second by default (choose Smooth, Slow or Off
in any key's settings).

## How it works

The plugin (`app.cutaway.streamdeck.sdPlugin/bin/plugin.js`) has no
dependencies: it talks to the Stream Deck app and to Cutaway's remote
control (`/api/ws` for state and commands, `/api/key/…png` for key
pictures) with a small built-in WebSocket client.

## For developers

```sh
python3 make_icons.py                       # redraw the icons
npx @elgato/cli validate app.cutaway.streamdeck.sdPlugin
npx @elgato/cli pack app.cutaway.streamdeck.sdPlugin   # → .streamDeckPlugin
node test/mock-streamdeck.js 127.0.0.1:8790 1234       # end-to-end test against a running Cutaway
```

The test stands in for the Stream Deck app: it takes scene 2, starts a
recording, checks a short press doesn't stop it but a held one does, turns a
dial, and checks the key pictures and the settings panel.
