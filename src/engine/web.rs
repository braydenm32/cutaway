//! Web page overlays: a page (a URL, or an HTML file) drawn over the
//! program with its transparency, like a browser source in OBS. Alerts,
//! chat boxes, scoreboards and animated graphics made for the web just work.
//!
//! Pages are drawn by a Chromium browser already on the computer (Edge,
//! Chrome, Brave, Chromium or Vivaldi), started hidden and run through its
//! DevTools protocol: each page has its own tab at the overlay's size, with
//! a transparent background, and sends a new picture whenever it changes.
//! On macOS and Linux the browser talks over a pipe, so it closes when
//! Cutaway does, however that happens; on Windows it's on a local port, and
//! a job object closes it with Cutaway.

use crate::util::LockExt;
use anyhow::{anyhow, bail, Context, Result};
use image::RgbaImage;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

/// How long the browser has to answer.
const ANSWER: Duration = Duration::from_secs(15);

/// Where Chromium browsers live, most likely first. `CUTAWAY_BROWSER` picks
/// one by hand.
fn candidates() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::env::var_os("CUTAWAY_BROWSER").map(PathBuf::from).into_iter().collect();
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        for app in ["Google Chrome", "Microsoft Edge", "Brave Browser", "Chromium", "Vivaldi", "Google Chrome Canary"] {
            for root in [PathBuf::from("/Applications"), home.join("Applications")] {
                v.push(root.join(format!("{app}.app/Contents/MacOS/{app}")));
            }
        }
    }
    #[cfg(windows)]
    {
        let dirs: Vec<PathBuf> = ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"].iter().filter_map(std::env::var_os).map(PathBuf::from).collect();
        for exe in ["Microsoft\\Edge\\Application\\msedge.exe", "Google\\Chrome\\Application\\chrome.exe", "BraveSoftware\\Brave-Browser\\Application\\brave.exe", "Chromium\\Application\\chrome.exe", "Vivaldi\\Application\\vivaldi.exe"] {
            for d in &dirs {
                v.push(d.join(exe));
            }
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        for dir in std::env::var("PATH").unwrap_or_default().split(':') {
            for exe in ["chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "microsoft-edge", "brave-browser"] {
                v.push(Path::new(dir).join(exe));
            }
        }
        v.push("/opt/pw-browsers/chromium".into());
    }
    v
}

/// The browser that draws web pages, if there is one.
pub fn find_browser() -> Option<PathBuf> {
    candidates().into_iter().find(|p| p.is_file())
}

/// The browser's name, for the settings ("Microsoft Edge").
pub fn browser_name(path: &Path) -> String {
    let s = path.to_string_lossy().to_lowercase();
    for (k, name) in [("edge", "Microsoft Edge"), ("brave", "Brave"), ("vivaldi", "Vivaldi"), ("chrome", "Google Chrome"), ("chromium", "Chromium")] {
        if s.contains(k) {
            return name.into();
        }
    }
    path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
}

/// What the overlay's address means: a web address, or a file on this
/// computer (an HTML file made for the show).
pub fn page_url(s: &str) -> Result<String> {
    let s = s.trim();
    if s.is_empty() {
        bail!("enter a web address or choose an HTML file");
    }
    let lower = s.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") || lower.starts_with("file://") || lower.starts_with("data:") {
        return Ok(s.to_string());
    }
    let p = Path::new(s);
    if p.is_absolute() || p.exists() {
        let p = std::fs::canonicalize(p).with_context(|| format!("{s} wasn't found"))?;
        let path = p.to_string_lossy().replace('\\', "/");
        let path = path.trim_start_matches("//?/");
        let encoded: String = path.chars().map(|c| if c == ' ' { "%20".to_string() } else if c == '#' { "%23".to_string() } else { c.to_string() }).collect();
        return Ok(if encoded.starts_with('/') { format!("file://{encoded}") } else { format!("file:///{encoded}") });
    }
    // "example.com/overlay": the web.
    Ok(format!("https://{s}"))
}

// ---- talking to the browser ----------------------------------------------------------------

type Writer = Box<dyn FnMut(&[u8]) -> std::io::Result<()> + Send>;
type Reader = Box<dyn FnMut() -> std::io::Result<Vec<u8>> + Send>;

struct Browser {
    write: Mutex<Writer>,
    next_id: AtomicU64,
    /// Answers waited for, by message id.
    waiting: Mutex<HashMap<u64, crossbeam_channel::Sender<Value>>>,
    /// Each tab's events, by its session.
    sessions: Mutex<HashMap<String, crossbeam_channel::Sender<Value>>>,
    child: Mutex<Child>,
    gone: AtomicBool,
    _profile: Profile,
}

/// The browser's own throwaway profile, removed when it closes.
struct Profile(PathBuf);

impl Drop for Profile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        let mut c = self.child.lock_ok();
        let _ = c.kill();
        let _ = c.wait();
    }
}

/// The one browser, while any page needs it.
fn browser() -> Result<Arc<Browser>> {
    static ONE: OnceLock<Mutex<Weak<Browser>>> = OnceLock::new();
    let slot = ONE.get_or_init(|| Mutex::new(Weak::new()));
    let mut slot = slot.lock_ok();
    if let Some(b) = slot.upgrade().filter(|b| !b.gone.load(Ordering::Relaxed)) {
        return Ok(b);
    }
    let b = Browser::start()?;
    *slot = Arc::downgrade(&b);
    Ok(b)
}

impl Browser {
    fn start() -> Result<Arc<Browser>> {
        let exe = find_browser().ok_or_else(|| anyhow!("web pages are drawn by a Chromium browser, and none was found: install Google Chrome or Microsoft Edge"))?;
        static N: AtomicU64 = AtomicU64::new(0);
        // Profiles left behind by a Cutaway that didn't close properly.
        if let Ok(dir) = std::fs::read_dir(std::env::temp_dir()) {
            for e in dir.flatten() {
                let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(24 * 3600));
                if old && e.file_name().to_string_lossy().starts_with("cutaway-web-") {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        let profile = std::env::temp_dir().join(format!("cutaway-web-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile)?;
        let mut cmd = Command::new(&exe);
        cmd.args([
            "--headless=new",
            &format!("--user-data-dir={}", profile.display()),
            "--no-first-run",
            "--no-default-browser-check",
            "--hide-scrollbars",
            "--mute-audio",
            "--autoplay-policy=no-user-gesture-required",
            // Keep animating although nobody's looking at it.
            "--disable-background-timer-throttling",
            "--disable-renderer-backgrounding",
            "--disable-backgrounding-occluded-windows",
            "--disable-extensions",
            "--disable-sync",
            "about:blank",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
        // Chromium won't run as root (a container, a test machine) with its sandbox.
        #[cfg(target_os = "linux")]
        if unsafe { libc::geteuid() } == 0 {
            cmd.arg("--no-sandbox");
        }
        let (child, write, read) = Self::connect(cmd, &profile).with_context(|| format!("{} didn't start", browser_name(&exe)))?;
        let b = Arc::new(Browser {
            write: Mutex::new(write),
            next_id: AtomicU64::new(1),
            waiting: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            child: Mutex::new(child),
            gone: AtomicBool::new(false),
            _profile: Profile(profile),
        });
        let weak = Arc::downgrade(&b);
        std::thread::Builder::new().name("web pages".into()).spawn(move || Self::read_loop(weak, read))?;
        Ok(b)
    }

    /// macOS and Linux: the DevTools protocol on the browser's descriptors
    /// 3 (in) and 4 (out). When Cutaway goes, so does the pipe, and the browser.
    #[cfg(unix)]
    fn connect(mut cmd: Command, _profile: &Path) -> Result<(Child, Writer, Reader)> {
        use std::io::Write;
        use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
        use std::os::unix::process::CommandExt;
        let pipe = || -> Result<(OwnedFd, OwnedFd)> {
            let mut fds = [0; 2];
            // SAFETY: pipe() fills the two descriptors, owned from here on.
            // Close-on-exec, so no other program (the browser included)
            // keeps Cutaway's ends open: when Cutaway goes, the browser sees
            // its pipe end.
            if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                bail!("no pipe: {}", std::io::Error::last_os_error());
            }
            let fds = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
            for fd in [&fds.0, &fds.1] {
                use std::os::fd::AsRawFd;
                unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
            }
            Ok(fds)
        };
        let (to_read, to_write) = pipe()?; // Cutaway → browser
        let (from_read, from_write) = pipe()?; // browser → Cutaway
        let (a, b) = (to_read.into_raw_fd(), from_write.into_raw_fd());
        // SAFETY: only async-signal-safe calls between fork and exec. The
        // descriptors are first copied out of the way, so 3 and 4 can't be
        // overwritten before they're used (and dup2 leaves them open on exec).
        unsafe {
            cmd.pre_exec(move || {
                let (a2, b2) = (libc::fcntl(a, libc::F_DUPFD, 10), libc::fcntl(b, libc::F_DUPFD, 10));
                if a2 < 0 || b2 < 0 || libc::dup2(a2, 3) < 0 || libc::dup2(b2, 4) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        cmd.arg("--remote-debugging-pipe");
        let child = cmd.spawn();
        // SAFETY: ours again, and the child has its copies.
        unsafe {
            drop(OwnedFd::from_raw_fd(a));
            drop(OwnedFd::from_raw_fd(b));
        }
        let child = child?;
        let mut out = std::fs::File::from(to_write);
        let mut inp = std::io::BufReader::new(std::fs::File::from(from_read));
        let write: Writer = Box::new(move |msg: &[u8]| {
            out.write_all(msg)?;
            out.write_all(&[0])?;
            out.flush()
        });
        let read: Reader = Box::new(move || {
            use std::io::BufRead;
            let mut buf = Vec::new();
            if inp.read_until(0, &mut buf)? == 0 {
                return Err(std::io::ErrorKind::UnexpectedEof.into());
            }
            buf.pop();
            Ok(buf)
        });
        Ok((child, write, read))
    }

    /// Windows: the DevTools protocol on a local port, as a WebSocket. A job
    /// object closes the browser with Cutaway.
    #[cfg(windows)]
    fn connect(mut cmd: Command, profile: &Path) -> Result<(Child, Writer, Reader)> {
        use std::io::Write;
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        cmd.arg("--remote-debugging-port=0");
        let mut child = cmd.spawn()?;
        crate::platform::win::kill_with_us(&child);
        // The browser says which port, and its address, in a file.
        let file = profile.join("DevToolsActivePort");
        let t0 = std::time::Instant::now();
        let (port, path) = loop {
            if let Ok(s) = std::fs::read_to_string(&file) {
                let mut lines = s.lines();
                if let (Some(p), Some(path)) = (lines.next().and_then(|p| p.trim().parse::<u16>().ok()), lines.next()) {
                    break (p, path.trim().to_string());
                }
            }
            if let Ok(Some(status)) = child.try_wait() {
                bail!("it closed ({status})");
            }
            if t0.elapsed() > ANSWER {
                let _ = child.kill();
                bail!("it didn't open its DevTools port");
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let mut sock = std::net::TcpStream::connect(("127.0.0.1", port))?;
        sock.set_nodelay(true)?;
        let key = crate::control::ws::base64(&rand_bytes::<16>());
        write!(sock, "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")?;
        // The answer's head, up to the blank line.
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            use std::io::Read;
            if sock.read(&mut byte)? == 0 || head.len() > 8192 {
                bail!("its DevTools port didn't answer");
            }
            head.push(byte[0]);
        }
        if !head.starts_with(b"HTTP/1.1 101") {
            bail!("its DevTools port said no: {}", String::from_utf8_lossy(&head).lines().next().unwrap_or(""));
        }
        let mut out = sock.try_clone()?;
        let mut inp = std::io::BufReader::new(sock);
        let write: Writer = Box::new(move |msg: &[u8]| out.write_all(&crate::control::ws::masked_frame(0x1, msg, rand_bytes::<4>())));
        let read: Reader = Box::new(move || loop {
            match crate::control::ws::read_message_max(&mut inp, 256 << 20)? {
                crate::control::ws::Message::Text(t) => return Ok(t.into_bytes()),
                crate::control::ws::Message::Binary(b) => return Ok(b),
                crate::control::ws::Message::Close => return Err(std::io::ErrorKind::UnexpectedEof.into()),
                _ => {}
            }
        });
        Ok((child, write, read))
    }

    #[cfg(not(any(unix, windows)))]
    fn connect(_cmd: Command, _profile: &Path) -> Result<(Child, Writer, Reader)> {
        bail!("web pages aren't available on this system")
    }

    /// Hand answers and events to whoever's waiting for them.
    fn read_loop(me: Weak<Browser>, mut read: Reader) {
        loop {
            let msg = read();
            let Some(b) = me.upgrade() else { return };
            let Ok(msg) = msg else {
                b.gone.store(true, Ordering::Relaxed);
                // Everyone waiting hears it's gone.
                b.waiting.lock_ok().clear();
                b.sessions.lock_ok().clear();
                return;
            };
            let Ok(v) = serde_json::from_slice::<Value>(&msg) else { continue };
            if let Some(id) = v.get("id").and_then(Value::as_u64) {
                if let Some(tx) = b.waiting.lock_ok().remove(&id) {
                    let _ = tx.send(v);
                }
            } else if let Some(s) = v.get("sessionId").and_then(Value::as_str) {
                let tx = b.sessions.lock_ok().get(s).cloned();
                if let Some(tx) = tx {
                    let _ = tx.try_send(v);
                }
            }
        }
    }

    /// Send a command (to a tab, with its session) and wait for the answer.
    fn call(&self, method: &str, params: Value, session: Option<&str>) -> Result<Value> {
        if self.gone.load(Ordering::Relaxed) {
            bail!("the browser closed");
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut msg = json!({ "id": id, "method": method, "params": params });
        if let Some(s) = session {
            msg["sessionId"] = json!(s);
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        self.waiting.lock_ok().insert(id, tx);
        (self.write.lock_ok())(msg.to_string().as_bytes()).context("the browser stopped listening")?;
        let v = rx.recv_timeout(ANSWER).map_err(|_| {
            self.waiting.lock_ok().remove(&id);
            anyhow!("the browser didn't answer ({method})")
        })?;
        if let Some(e) = v.get("error") {
            bail!("{method}: {}", e.get("message").and_then(Value::as_str).unwrap_or("failed"));
        }
        Ok(v.get("result").cloned().unwrap_or(Value::Null))
    }

    /// Send without waiting (acknowledgements).
    fn tell(&self, method: &str, params: Value, session: &str) {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({ "id": id, "method": method, "params": params, "sessionId": session });
        let _ = (self.write.lock_ok())(msg.to_string().as_bytes());
    }
}

#[cfg(windows)]
fn rand_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    let _ = getrandom::fill(&mut b);
    b
}

// ---- a page ------------------------------------------------------------------------------------

/// A web page being drawn: its newest picture, whenever it changes.
pub struct WebPage {
    picture: Arc<Mutex<Option<(u64, Arc<RgbaImage>)>>>,
    problem: Arc<Mutex<Option<String>>>,
    stop: Arc<AtomicBool>,
    reload: Arc<AtomicBool>,
}

impl Drop for WebPage {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl WebPage {
    /// Open `address` at `size` (CSS pixels). `changed` is called with each
    /// new picture.
    pub fn open(address: &str, size: (u32, u32), changed: Arc<dyn Fn() + Send + Sync>) -> WebPage {
        let page = WebPage { picture: Default::default(), problem: Default::default(), stop: Default::default(), reload: Default::default() };
        let (picture, problem, stop, reload) = (page.picture.clone(), page.problem.clone(), page.stop.clone(), page.reload.clone());
        let address = address.to_string();
        let size = (size.0.clamp(16, 3840), size.1.clamp(16, 2160));
        let _ = std::thread::Builder::new().name("web page".into()).spawn(move || {
            let mut wait = Duration::from_secs(2);
            while !stop.load(Ordering::Relaxed) {
                let r = run_page(&address, size, &picture, &stop, &reload, &changed, &problem);
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                // It went wrong (or the browser closed): say so, and try again in a while.
                *problem.lock_ok() = Some(match r {
                    Err(e) => format!("{e:#}"),
                    Ok(()) => "the page closed".into(),
                });
                changed();
                let until = std::time::Instant::now() + wait;
                while std::time::Instant::now() < until && !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                wait = (wait * 2).min(Duration::from_secs(30));
            }
        });
        page
    }

    /// The newest picture: (a number that changes with it, the picture).
    pub fn picture(&self) -> Option<(u64, Arc<RgbaImage>)> {
        self.picture.lock_ok().clone()
    }

    /// Why it isn't showing, if it isn't.
    pub fn problem(&self) -> Option<String> {
        self.problem.lock_ok().clone()
    }

    /// Load it again (to restart an animation, or pick up changes).
    pub fn reload(&self) {
        self.reload.store(true, Ordering::Relaxed);
    }
}

fn run_page(
    address: &str,
    size: (u32, u32),
    picture: &Mutex<Option<(u64, Arc<RgbaImage>)>>,
    stop: &AtomicBool,
    reload: &AtomicBool,
    changed: &Arc<dyn Fn() + Send + Sync>,
    problem: &Mutex<Option<String>>,
) -> Result<()> {
    static VERSIONS: AtomicU64 = AtomicU64::new(1);
    let url = page_url(address)?;
    let b = browser()?;
    let target = b.call("Target.createTarget", json!({ "url": "about:blank" }), None)?;
    let target = target["targetId"].as_str().ok_or_else(|| anyhow!("the browser didn't open a tab"))?.to_string();
    // The tab closes with this page, however it ends.
    struct Tab(Arc<Browser>, String, Option<String>);
    impl Drop for Tab {
        fn drop(&mut self) {
            if let Some(s) = &self.2 {
                self.0.sessions.lock_ok().remove(s);
            }
            let _ = self.0.call("Target.closeTarget", json!({ "targetId": self.1 }), None);
        }
    }
    let mut tab = Tab(b.clone(), target.clone(), None);
    let session = b.call("Target.attachToTarget", json!({ "targetId": target, "flatten": true }), None)?;
    let session = session["sessionId"].as_str().ok_or_else(|| anyhow!("the browser didn't attach to the tab"))?.to_string();
    let (tx, rx) = crossbeam_channel::bounded::<Value>(64);
    b.sessions.lock_ok().insert(session.clone(), tx);
    tab.2 = Some(session.clone());
    let s = Some(session.as_str());
    b.call("Emulation.setDeviceMetricsOverride", json!({ "width": size.0, "height": size.1, "deviceScaleFactor": 1, "mobile": false }), s)?;
    // See-through wherever the page doesn't paint.
    b.call("Emulation.setDefaultBackgroundColorOverride", json!({ "color": { "r": 0, "g": 0, "b": 0, "a": 0 } }), s)?;
    b.call("Page.enable", json!({}), s)?;
    b.call("Inspector.enable", json!({}), s).ok();
    let nav = b.call("Page.navigate", json!({ "url": url }), s)?;
    if let Some(e) = nav.get("errorText").and_then(Value::as_str) {
        bail!("{url} couldn't be opened ({e})");
    }
    b.call("Page.startScreencast", json!({ "format": "png", "maxWidth": size.0, "maxHeight": size.1, "everyNthFrame": 1 }), s)?;
    *problem.lock_ok() = None;
    while !stop.load(Ordering::Relaxed) {
        if reload.swap(false, Ordering::Relaxed) {
            b.call("Page.reload", json!({ "ignoreCache": true }), s)?;
        }
        let ev = match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(ev) => ev,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if b.gone.load(Ordering::Relaxed) {
                    bail!("the browser closed");
                }
                continue;
            }
            Err(_) => bail!("the browser closed"),
        };
        match ev.get("method").and_then(Value::as_str) {
            Some("Page.screencastFrame") => {
                let p = &ev["params"];
                // Next one, please.
                b.tell("Page.screencastFrameAck", json!({ "sessionId": p["sessionId"] }), &session);
                let Some(data) = p["data"].as_str() else { continue };
                let Some(png) = crate::util::unbase64(data) else { continue };
                let Ok(img) = image::load_from_memory_with_format(&png, image::ImageFormat::Png) else { continue };
                let mut img = img.into_rgba8();
                unpremultiply(&mut img);
                *picture.lock_ok() = Some((VERSIONS.fetch_add(1, Ordering::Relaxed), Arc::new(img)));
                changed();
            }
            Some("Inspector.targetCrashed") => bail!("the page crashed"),
            _ => {}
        }
    }
    Ok(())
}

/// Chromium's pictures come with their colours already multiplied by
/// their transparency; the compositor wants them as they are. (A picture
/// with any colour brighter than its opacity wasn't multiplied, and is
/// left alone.)
fn unpremultiply(img: &mut RgbaImage) {
    if img.pixels().any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]) {
        return;
    }
    for p in img.pixels_mut() {
        let a = p[3] as u32;
        if a > 0 && a < 255 {
            for c in 0..3 {
                p[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(page_url("https://example.com/a").unwrap(), "https://example.com/a");
        assert_eq!(page_url("example.com/overlay").unwrap(), "https://example.com/overlay");
        assert!(page_url("  ").is_err());
        let f = std::env::temp_dir().join("cutaway web test.html");
        std::fs::write(&f, "<p>hi</p>").unwrap();
        let u = page_url(&f.to_string_lossy()).unwrap();
        assert!(u.starts_with("file:///") && u.ends_with("cutaway%20web%20test.html"), "{u}");
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn colours_come_back_from_premultiplied() {
        let mut img = RgbaImage::from_pixel(2, 1, image::Rgba([64, 64, 64, 64]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 0]));
        unpremultiply(&mut img);
        assert_eq!(img.get_pixel(0, 0).0, [255, 255, 255, 64]);
        // Not premultiplied: left as it is.
        let mut img = RgbaImage::from_pixel(1, 1, image::Rgba([200, 10, 10, 64]));
        unpremultiply(&mut img);
        assert_eq!(img.get_pixel(0, 0).0, [200, 10, 10, 64]);
    }

    /// A real browser draws a page with a see-through background. (Skipped
    /// where there's no Chromium browser.)
    #[test]
    fn draws_a_page_with_transparency() {
        if find_browser().is_none() {
            eprintln!("no Chromium browser here; skipping");
            return;
        }
        let html = "data:text/html,<body style='margin:0'><div style='position:absolute;left:20px;top:10px;width:100px;height:50px;background:rgba(255,255,255,0.5)'></div></body>";
        let (tx, rx) = crossbeam_channel::unbounded();
        let page = WebPage::open(html, (200, 100), Arc::new(move || {
            let _ = tx.send(());
        }));
        let t0 = std::time::Instant::now();
        // (The first picture can be from before the page has loaded.)
        let drawn = |p: &WebPage| p.picture().is_some_and(|(_, i)| i.get_pixel(60, 30)[3] > 0);
        while !drawn(&page) && t0.elapsed() < Duration::from_secs(30) {
            let _ = rx.recv_timeout(Duration::from_millis(200));
            if let Some(p) = page.problem() {
                eprintln!("waiting: {p}");
            }
        }
        let (_, img) = page.picture().expect("a picture of the page");
        assert_eq!(img.dimensions(), (200, 100));
        assert_eq!(img.get_pixel(5, 5)[3], 0, "see-through where the page is empty");
        let box_px = img.get_pixel(60, 30).0;
        assert!((box_px[3] as i32 - 128).abs() <= 3 && box_px[0] > 240, "the half-see-through white box: {box_px:?}");
        drop(page);
    }
}
