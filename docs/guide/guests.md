[← The manual](README.md)

# Remote guests

## Remote guests

1. **Sources › Guests:** tick **let guests join by link** and press
   **＋ Guest spot** for each guest. A spot is a video source and an audio
   input of the same name, so it can go in scenes, be recorded and be mixed
   like any camera and mic. Press **Apply** on the bar at the bottom.
2. **Send the link:** the guests panel (and the settings) show each spot's
   link — **Copy link** and send it by message or email. The first time,
   Cutaway downloads Cloudflare's small tunnel program (or uses `cloudflared`
   if it's installed); the address is new each time Cutaway starts, so send
   links on the day. For links you can send days ahead, use Tailscale Funnel
   (below).
3. **The guest** opens the link in Chrome, Edge, Safari or Firefox, checks
   their camera and mic, types their name and presses **Join**. Their spot
   turns green with their name, and a message on the desk says so ("Maya
   joined as Guest 1"); another says when they leave, and if their spot was
   on air at the time, it's a warning on the status light too. They hear the show without their own voice,
   your talkback, and see the program.
4. **During the show:** take their spot like any camera; their level is on
   their audio strip; hold **✕** for a second to disconnect them (a quick
   click only says so, so a slip beside **Copy link** can't drop their call).
   If they reload the page they simply rejoin.

- **Screen share:** tick **Screen share** beside a guest's spot and apply.
  That adds a source called "*Name*'s screen", and their page gets a
  **Share screen** button. They pick the whole screen, one window or a
  browser tab, and it arrives on that source (sharp for slides and text, up
  to 1080p), separate from their camera, so a scene can show both side by
  side. **Stop sharing** on their page, or **✕** on the screen's row in the
  guests panel, ends it. Sharing needs a computer (Chrome, Edge or Safari);
  phones and tablets don't offer it.

- **Connection warnings:** every couple of seconds each guest's browser (and
  each phone camera) tells Cutaway how its connection is holding up: how much
  of the picture is getting lost, how far behind it is, its frame rate and
  whether the upload is holding it back. Signal bars beside the guest in the
  guests panel show it (hover for the reason), and a **Weak connection** or
  **Poor connection** tag appears on their tile, so you can take them off
  air or ask them to move before their picture freezes. A guest whose
  picture stops coming shows as poor straight away. Bad news shows at once
  and good news once it has lasted a few seconds, so it doesn't flicker.
  The guest sees a line on their own page too, with what might help.

- **Firewalls:** guests connect directly to Cutaway (WebRTC), which works on
  almost every home and mobile network. A few office, school and hotel
  networks block that; set a TURN server (Cloudflare Calls, Twilio, your own
  coturn) under **Strict networks** to relay for them.
- **Links that stay the same (Tailscale Funnel):** install
  [Tailscale](https://tailscale.com/download) and sign in (a free account),
  then choose **Tailscale Funnel** under **Reached through** and apply.
  The first time, Cutaway shows a link to allow Funnel for your Tailscale
  network; after that it connects by itself whenever Cutaway starts. Links
  look like `https://your-mac.tail1234.ts.net/g/…` and keep working from one
  show to the next, so you can send them days ahead. The address is named
  after the computer: renaming it in Tailscale changes the links.
- **Your own address:** instead of the tunnel, choose **my own https://
  address** and forward it to the guest port (8792); browsers only allow the
  camera on https pages.
- **Echo:** ask guests to wear headphones; their browser's echo cancellation
  handles the rest.
- **Backup recordings:** tick **Backup recordings** and, while you record,
  each guest's browser also records them and sends it in pieces into the
  take's `Guests` folder (`Guest_1_backup.webm` or `.mp4`, with a note of
  when it starts). If their connection is slow, it finishes after you stop;
  their page asks them to keep it open until it's done, and the guests panel
  shows how much has arrived. Their page tells them it's happening.
