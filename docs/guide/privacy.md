# Privacy

Cutaway is a program you run on your own computer. It doesn't have
accounts, and the people who make it don't receive your recordings,
settings, guests' details or anything else about you or your shows.

This page says what Cutaway keeps, what it sends and where, and how to
remove it. It's written with Australia's Privacy Act and its Australian
Privacy Principles (the APPs) in mind, and applies wherever you use Cutaway.

## What Cutaway keeps on your computer

| What | Where | How long |
|---|---|---|
| Recordings, captions, markers and the editor timelines | Your recording folder (`Movies/Cutaway` on a Mac, `Videos\Cutaway` on Windows, unless you chose another) | Until you delete them |
| Settings and saved shows (including guests' and presenters' names you typed in) | The settings folder (see [Where your files go](first-hour.md#where-your-files-go)) | Until you change or remove them |
| Stream keys, passwords and your Google sign-in | The Keychain (Mac) or Credential Manager (Windows), locked to your user account | Until you remove them |
| The log (what Cutaway did, which can include guests' names and file names) | The log folder | A month at most |
| Uploads still to go | `uploads.json`, beside the settings | Until they're sent or cancelled |
| A remote guest's backup recording, if you turn it on | Your recording folder, beside the show | Until you delete it |

Cutaway doesn't encrypt recordings itself. If the computer could be lost or
shared, turn on FileVault (Mac) or BitLocker (Windows).

## What Cutaway sends over the internet

Nothing about you or your shows, unless you ask it to:

- **The update check** asks GitHub once at start whether there's a newer
  version (it sends only what any web browser does). Turn it off in
  **More › App › General**.
- **Streams and uploads** go only to the services you set up (YouTube,
  Twitch, Google Drive and so on), with your own keys and sign-in. Their
  own privacy policies cover what they do with them.
- **Remote guests** reach you through Cloudflare's tunnel (or Tailscale, if
  you pick it). The guest page, their name and their backup recording pass
  through it; their live picture and sound come straight to your computer
  where the network allows. Both ends ask a public address server
  (Cloudflare's and Google's) for their internet address. Cloudflare and
  Google are overseas companies.
- **Problem reports** are saved as a file for you to send yourself. Stream
  keys, passwords, your Google account and your user folder's name are taken
  out first. Guests' names can still be in the log, so look through it
  before you send it if that matters.

## Your guests and the people you record

When you record or stream people, you're the one collecting their picture,
voice and name, so it's worth telling them:

- **Remote guests** see a notice on the guest page before they join: that
  the show may be recorded or streamed, and that their name, picture and
  sound go to you.
- **People in the room or on a call:** tell them before you start, and get
  their OK. In New South Wales, South Australia, Western Australia,
  Tasmania and the ACT, recording a private conversation generally needs
  everyone's consent, and other places have their own rules.
- **If your business is covered by the Privacy Act** (most with a turnover
  over $3 million, and some smaller ones, such as health services), the APPs
  apply to what you record and keep: say why you collect it, keep it safe,
  delete it when you no longer need it, and let people see what you hold
  about them. Most small studios aren't covered, but following them is good
  practice.

## Removing what Cutaway keeps

**More › App › Diagnostics → Remove my settings and keys…** closes Cutaway,
then removes your settings and every saved show, the stream keys and
passwords in the Keychain or Credential Manager, the log, the list of
uploads still to go and Cutaway's own downloads (the guest tunnel program).
Your recordings and Cutaway itself stay. To keep a copy of your settings,
export them first (**More › App › Shows → Export settings…**).

To remove Cutaway itself afterwards, delete the app (or the Windows folder).
On Windows, remove the virtual camera first if you installed it (**Record &
stream › Screens & webcam → Remove**).

## Questions

Ask on the [Cutaway GitHub page](https://github.com/braydenm32/cutaway/issues).
If Cutaway ever starts sending anything new (crash reports, say), it will
ask you first, and this page will say what is sent, where and why.
