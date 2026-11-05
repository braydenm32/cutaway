//! The phone/tablet remote and HTTP API: a small web server built in.
//!
//! * `/` — the remote, for a phone held in one hand: record and go live at
//!   the top, Scenes / Sound / Titles / More tabs at the bottom. Its buttons
//!   are made once and updated in place as the state is pushed, so a finger
//!   on one never loses its tap to a redraw.
//! * `/?tally=N` — a full-screen tally light for source N.
//! * `/api/state` — everything as JSON.
//! * `/api/action?do=<command>` — run a command (see `control`).
//! * `/api/program.jpg` — the program picture.
//! * `/api/tally/N` — `program`, `preview` or `off`, for DIY tally lights.
//! * `/api/key/scene/N.png`, `/api/key/source/N.png` — Stream Deck key
//!   pictures (`?size=144&label=0`).
//! * `/prompter` — the teleprompter, full screen, for a tablet;
//!   `/api/prompter` its script.
//! * `/api/ws` — WebSocket: the state is pushed whenever it changes, and
//!   commands can be sent as text (`scene:2`) or JSON
//!   (`{"do":"scene:2","id":1}`, answered with `{"type":"reply","id":1,...}`).
//!
//! Everything but the pages needs the PIN (`?pin=`, an `X-Cutaway-Pin`
//! header or the cookie the page sets). Five different wrong PINs in a row from one
//! address make it wait a minute.

use crate::util::LockExt;
use super::keys::{self, KeyKind, KeySources};
use super::state::ShowState;
use super::{ws, Action, Request};
use crate::engine::{FrameSlot, Waker};
use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Sender};
use std::io::{Read, Write};
use std::collections::HashMap;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("remote.html");
const PROMPTER_PAGE: &str = include_str!("prompter.html");
/// The page's icon, and the one a phone puts on its home screen.
const ICON_PNG: &[u8] = include_bytes!("../../assets/logo/remote.png");

/// What the server shares with the app.
pub struct RemoteShared {
    /// Current state as JSON (kept up to date by the app).
    pub state: Mutex<String>,
    /// Tally per source ("program", "preview", "off").
    pub tally: Mutex<Vec<String>>,
    pub program: Mutex<Option<Arc<FrameSlot>>>,
    pub pin: Mutex<String>,
    pub requests: Sender<Request>,
    pub waker: Waker,
    /// The program picture as JPEG: its frame generation, when it was
    /// made, and the bytes.
    jpeg: Mutex<Option<(u64, Instant, Arc<Vec<u8>>)>>,
    /// Key pictures already drawn: what they were drawn from, and the PNG.
    key_cache: Mutex<HashMap<(String, u32, bool), (u64, Arc<Vec<u8>>)>>,
    /// The state, and a counter that moves whenever it changes.
    show: Mutex<(u64, ShowState, Instant)>,
    changed: Condvar,
    /// What key pictures are drawn from.
    pub keys: Mutex<KeySources>,
    /// The teleprompter's script as JSON ({version, text, segments}).
    pub prompter: Mutex<String>,
    /// Wrong PINs by address: how many different ones in a row, when the
    /// last was, and what it was.
    wrong: Mutex<HashMap<IpAddr, (u32, Instant, String)>>,
    /// WebSocket controllers connected now.
    clients: AtomicUsize,
    /// When something last asked for the state (a page, a key picture, a
    /// tally light) over plain HTTP.
    demand: Mutex<Option<Instant>>,
    /// How many times the app has published, and when last (even when
    /// nothing had changed): the app's pulse.
    stamp: Mutex<(u64, Instant)>,
    stamped: Condvar,
    /// Held down from a phone over plain HTTP: (page, the command that lets
    /// go) and when its page last said it was still holding.
    holds: Mutex<HashMap<(String, String), Instant>>,
    holds_changed: Condvar,
}

/// Wrong PINs from one address before it has to wait.
const TRIES: u32 = 5;
/// How long it waits.
const LOCKED_FOR: Duration = Duration::from_secs(60);
/// A WebSocket that's had nothing for this long gets `{"type":"tick"}`.
const TICK_EVERY: Duration = Duration::from_secs(2);
/// A WebSocket that sends nothing for this long is gone: the server pings
/// every 15 s and a live client answers.
const READ_TIMEOUT: Duration = Duration::from_secs(40);
/// A write that can't finish in this long means the peer stopped reading.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);
/// Something asked for the state this recently: keep it up to date.
const DEMAND_FOR: Duration = Duration::from_secs(5);
/// A phone holding talkback, a pad or a camera move says so every second;
/// this long without hearing it, Cutaway lets go.
const HOLD_LEASE: Duration = Duration::from_secs(3);
/// Levels alone (no other change) go out at most this often.
const LEVELS_EVERY: Duration = Duration::from_millis(500);
/// The program picture is made again at most this often.
const JPEG_EVERY: Duration = Duration::from_secs(1);

impl RemoteShared {
    /// Whether `ip` is waiting after too many wrong PINs.
    fn locked_out(&self, ip: IpAddr) -> bool {
        let mut w = self.wrong.lock_ok();
        w.retain(|_, (_, at, _)| at.elapsed() < LOCKED_FOR);
        w.get(&ip).is_some_and(|(n, _, _)| *n >= TRIES)
    }

    /// A PIN tried. The same wrong PIN again (a phone or Stream Deck still
    /// using the old one after a change, asking every second) counts once:
    /// only guessing different ones locks an address out.
    fn note_pin(&self, ip: IpAddr, given: &str, right: bool) {
        let mut w = self.wrong.lock_ok();
        if right {
            w.remove(&ip);
        } else {
            let e = w.entry(ip).or_insert((0, Instant::now(), String::new()));
            if e.0 == 0 || e.2 != given {
                e.0 += 1;
                e.2 = given.to_string();
            }
            e.1 = Instant::now();
        }
    }
}

impl RemoteShared {
    pub fn new(requests: Sender<Request>, waker: Waker) -> Arc<Self> {
        Arc::new(RemoteShared {
            state: Mutex::new("{}".into()),
            tally: Mutex::new(Vec::new()),
            program: Mutex::new(None),
            pin: Mutex::new(String::new()),
            requests,
            waker,
            jpeg: Mutex::new(None),
            key_cache: Mutex::new(HashMap::new()),
            clients: AtomicUsize::new(0),
            demand: Mutex::new(None),
            stamp: Mutex::new((0, Instant::now())),
            stamped: Condvar::new(),
            holds: Mutex::new(HashMap::new()),
            holds_changed: Condvar::new(),
            show: Mutex::new((0, ShowState::default(), Instant::now())),
            changed: Condvar::new(),
            keys: Mutex::new(KeySources::default()),
            prompter: Mutex::new("{}".into()),
            wrong: Mutex::new(HashMap::new()),
        })
    }

    /// New state from the app; wakes WebSocket clients if anything changed.
    pub fn publish(&self, s: &ShowState) {
        self.store(s);
        // Only now: whoever this wakes finds the new state in place.
        let mut stamp = self.stamp.lock_ok();
        *stamp = (stamp.0 + 1, Instant::now());
        self.stamped.notify_all();
    }

    fn store(&self, s: &ShowState) {
        let mut show = self.show.lock_ok();
        if show.1 == *s {
            return;
        }
        // Only the meters moved: they go out a couple of times a second, not
        // with every publish. (The old state stays until then, so the last
        // movement is never lost.)
        if show.2.elapsed() < LEVELS_EVERY && same_but_levels(&show.1, s) {
            return;
        }
        *self.state.lock_ok() = serde_json::to_string(s).unwrap_or_else(|_| "{}".into());
        *self.tally.lock_ok() = s.sources.iter().map(|x| x.tally.clone()).collect();
        show.1 = s.clone();
        show.0 += 1;
        show.2 = Instant::now();
        self.changed.notify_all();
    }

    fn snapshot(&self) -> (u64, ShowState) {
        let show = self.show.lock_ok();
        (show.0, show.1.clone())
    }

    /// The state as JSON, with its counter (the text `publish` already made).
    fn json(&self) -> (u64, String) {
        let show = self.show.lock_ok();
        // (Before the app has published anything: the empty show, whole.)
        let json = if show.0 == 0 { serde_json::to_string(&show.1).unwrap_or_else(|_| "{}".into()) } else { self.state.lock_ok().clone() };
        (show.0, json)
    }

    /// Whether the app should keep the state up to date: a controller is
    /// connected, or something asked for it a moment ago.
    pub fn wanted(&self) -> bool {
        self.clients.load(Ordering::Relaxed) > 0 || self.demand.lock_ok().is_some_and(|t| t.elapsed() < DEMAND_FOR)
    }

    /// Called when something wants the state: if the app wasn't keeping it
    /// up (nobody was asking), wake it and wait a moment for a fresh one.
    fn ensure_fresh(&self) {
        let was = self.wanted();
        *self.demand.lock_ok() = Some(Instant::now());
        if was {
            return;
        }
        let stamp = self.stamp.lock_ok();
        let seen = stamp.0;
        drop(stamp);
        (self.waker)();
        let stamp = self.stamp.lock_ok();
        let _ = self.stamped.wait_timeout_while(stamp, Duration::from_millis(700), |s| s.0 == seen);
    }

    /// How long since the app last published (its pulse), in milliseconds.
    fn app_age_ms(&self) -> u64 {
        self.stamp.lock_ok().1.elapsed().as_millis() as u64
    }

    /// A command that started something held was run for page `who`: note
    /// how to let go, or forget it when the release itself comes.
    fn note_hold(&self, who: &str, cmd: &str) {
        if who.is_empty() {
            return;
        }
        let c = cmd.trim().to_ascii_lowercase();
        let mut h = self.holds.lock_ok();
        h.remove(&(who.to_string(), c.clone()));
        if let Some(up) = let_go_of(&c) {
            h.insert((who.to_string(), up), Instant::now());
            self.holds_changed.notify_all();
        }
    }

    /// Page `who` is still holding.
    /// False when it isn't held (any more): the lease ran out.
    fn hold_alive(&self, who: &str) -> bool {
        let mut h = self.holds.lock_ok();
        let mut any = false;
        for ((w, _), t) in h.iter_mut() {
            if w == who {
                *t = Instant::now();
                any = true;
            }
        }
        any
    }

    /// Lets go of holds whose page has gone quiet. Runs on its own thread.
    fn watch_holds(&self, stop: &AtomicBool) {
        let mut h = self.holds.lock_ok();
        while !stop.load(Ordering::Relaxed) {
            let gone: Vec<(String, String)> = h.iter().filter(|(_, t)| t.elapsed() > HOLD_LEASE).map(|(k, _)| k.clone()).collect();
            for k in &gone {
                h.remove(k);
            }
            if !gone.is_empty() {
                drop(h);
                for (_, up) in gone {
                    let _ = self.run(&up);
                }
                h = self.holds.lock_ok();
                continue;
            }
            // Nothing held: sleep until something is; held: look again soon.
            let wait = if h.is_empty() { Duration::from_secs(3600) } else { Duration::from_millis(500) };
            h = self.holds_changed.wait_timeout(h, wait).unwrap_or_else(|e| e.into_inner()).0;
        }
    }

    /// Wait (up to `timeout`) for the state to move past `seen`.
    fn wait_change(&self, seen: u64, timeout: Duration) -> u64 {
        let show = self.show.lock_ok();
        let (show, _) = self.changed.wait_timeout_while(show, timeout, |s| s.0 == seen).unwrap_or_else(|e| e.into_inner());
        show.0
    }

    fn key_png(&self, kind: KeyKind, size: u32, label: bool) -> Option<Arc<Vec<u8>>> {
        self.ensure_fresh();
        let src = self.keys.lock_ok().clone();
        let state = self.snapshot().1;
        // What the picture is drawn from, as one number: when it hasn't
        // moved, the picture from last time is the answer.
        let mut h = DefaultHasher::new();
        let (what, n) = match kind {
            KeyKind::Scene(i) => ("scene", i),
            KeyKind::Source(i) => ("source", i),
        };
        let layers: Vec<usize> = match kind {
            KeyKind::Scene(i) => {
                let layers = src.scenes.get(i).map(|sc| sc.layers.as_slice()).unwrap_or(&[]);
                format!("{layers:?}").hash(&mut h);
                let sl = state.scenes.get(i)?;
                (sl.name.as_str(), sl.program, sl.preview).hash(&mut h);
                layers.iter().map(|l| l.source).collect()
            }
            KeyKind::Source(i) => {
                let so = state.sources.get(i)?;
                (so.name.as_str(), so.tally.as_str(), so.offline).hash(&mut h);
                vec![i]
            }
        };
        (src.canvas, src.background).hash(&mut h);
        for l in layers {
            // (the frame's counter moves whenever a new picture arrives)
            src.previews.get(l).and_then(|p| p.latest()).map(|(g, _)| g).hash(&mut h);
        }
        let stamp = h.finish();
        let key = (format!("{what}{n}"), size, label);
        if let Some((s, png)) = self.key_cache.lock_ok().get(&key) {
            if *s == stamp {
                return Some(png.clone());
            }
        }
        let png = Arc::new(keys::png(&keys::render(&src, &state, kind, size, label)?)?);
        let mut cache = self.key_cache.lock_ok();
        if cache.len() > 200 {
            cache.clear();
        }
        cache.insert(key, (stamp, png.clone()));
        Some(png)
    }

    /// Run a command for a controller and wait for the answer.
    fn run(&self, cmd: &str) -> Result<String, String> {
        let action = Action::parse(cmd)?;
        let (tx, rx) = bounded(1);
        self.requests.send(Request { action, reply: Some(tx) }).map_err(|_| "Cutaway is closing".to_string())?;
        (self.waker)();
        rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| Err("Cutaway didn't answer".into()))
    }

    /// The program picture as JPEG, with the frame generation it was made
    /// from. A picture made in the last second is reused, so a few phones
    /// cost one encode a second.
    fn program_jpeg(&self) -> Option<(u64, Arc<Vec<u8>>)> {
        let slot = self.program.lock_ok().clone()?;
        let (generation, frame) = slot.latest()?;
        if let Some((g, at, j)) = &*self.jpeg.lock_ok() {
            if *g == generation || at.elapsed() < JPEG_EVERY {
                return Some((*g, j.clone()));
            }
        }
        // A view over the frame: no copy of the whole picture.
        let view = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(frame.width as u32, frame.height as u32, &frame.rgba[..])?;
        let small = if view.width() > 640 {
            let h = (640 * view.height() / view.width()).max(2);
            image::imageops::resize(&view, 640, h, image::imageops::FilterType::Triangle)
        } else {
            image::RgbaImage::from_raw(view.width(), view.height(), view.as_raw().to_vec())?
        };
        let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70).encode_image(&rgb).ok()?;
        let j = Arc::new(out);
        *self.jpeg.lock_ok() = Some((generation, Instant::now(), j.clone()));
        Some((generation, j))
    }
}

/// Whether two states differ only in the input meters.
fn same_but_levels(old: &ShowState, new: &ShowState) -> bool {
    if old.inputs.len() != new.inputs.len() {
        return false;
    }
    let mut t = new.clone();
    for (a, b) in t.inputs.iter_mut().zip(&old.inputs) {
        a.level = b.level;
    }
    t == *old
}

pub struct RemoteServer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    holds: Option<JoinHandle<()>>,
    shared: Arc<RemoteShared>,
    pub port: u16,
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.shared.holds_changed.notify_all();
        // The accept below waits for a connection: make one.
        let _ = TcpStream::connect_timeout(&std::net::SocketAddr::from(([127, 0, 0, 1], self.port)), Duration::from_secs(1));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        // (The holds thread isn't waited for: it may be waiting on the app,
        // which may be waiting for this drop. It stops by itself.)
        self.holds.take();
    }
}

impl RemoteServer {
    pub fn start(port: u16, shared: Arc<RemoteShared>) -> Result<RemoteServer> {
        let listener = TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("can't use port {port} for the remote (is something else using it?)"))?;
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let sh_all = shared.clone();
        let for_holds = shared.clone();
        let stop_holds = stop.clone();
        let thread = std::thread::Builder::new().name("remote".into()).spawn(move || {
            // Waits in accept (no waking when nobody is there); `Drop` makes
            // one last connection to get out.
            while !s2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((conn, _)) => {
                        if s2.load(Ordering::Relaxed) {
                            break;
                        }
                        // On macOS and Windows a connection inherits the
                        // listener's non-blocking mode; the client code
                        // expects blocking reads (with timeouts).
                        if conn.set_nonblocking(false).is_err() {
                            continue;
                        }
                        let sh = shared.clone();
                        let _ = std::thread::Builder::new().name("remote client".into()).spawn(move || {
                            let _ = handle(conn, &sh);
                        });
                    }
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        })?;
        let holds = std::thread::Builder::new().name("remote holds".into()).spawn(move || for_holds.watch_holds(&stop_holds))?;
        Ok(RemoteServer { stop, thread: Some(thread), holds: Some(holds), shared: sh_all, port })
    }
}

fn query(q: &str) -> Vec<(String, String)> {
    q.split('&').filter(|kv| !kv.is_empty()).map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (crate::util::percent_decode(k, true), crate::util::percent_decode(v, true))
    }).collect()
}

/// Compared without stopping at the first difference (so timing doesn't
/// give the PIN away a digit at a time).
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |d, (x, y)| d | (x ^ y)) == 0
}

fn respond(conn: &mut TcpStream, status: &str, ctype: &str, body: &[u8], extra: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    conn.write_all(head.as_bytes())?;
    conn.write_all(body)
}

fn handle(mut conn: TcpStream, sh: &RemoteShared) -> std::io::Result<()> {
    conn.set_read_timeout(Some(Duration::from_secs(5)))?;
    conn.set_nodelay(true).ok();
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16384 {
        let n = conn.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut lines = text.lines();
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_string();
    let target = parts.next().unwrap_or("/").to_string();
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let params = query(q);
    let param = |k: &str| params.iter().find(|(a, _)| a == k).map(|(_, v)| v.clone());
    let mut cookie_pin = None;
    let mut header_pin = None;
    let mut ws_key = None;
    let mut if_none_match = None;
    let mut upgrade = false;
    for l in lines {
        let lower = l.to_lowercase();
        if lower.starts_with("upgrade:") && lower.contains("websocket") {
            upgrade = true;
        } else if lower.starts_with("sec-websocket-key:") {
            ws_key = Some(l[18..].trim().to_string());
        }
        if lower.starts_with("cookie:") {
            // From the line as sent: the PIN can have capitals.
            for c in l[7..].split(';') {
                if let Some(p) = c.trim().strip_prefix("cutaway_pin=") {
                    cookie_pin = Some(p.to_string());
                }
            }
        } else if lower.starts_with("x-cutaway-pin:") {
            header_pin = Some(l[14..].trim().to_string());
        } else if lower.starts_with("if-none-match:") {
            if_none_match = Some(l[14..].trim().to_string());
        }
    }
    if method == "OPTIONS" {
        return respond(&mut conn, "204 No Content", "text/plain", b"", "Access-Control-Allow-Headers: X-Cutaway-Pin\r\n");
    }
    // The page itself is public (it asks for the PIN); everything else
    // needs it. There's always a PIN (with none set, nothing is let in), and
    // an address that keeps getting it wrong has to wait a minute.
    let pin = sh.pin.lock_ok().clone();
    let given = param("pin").or(header_pin).or(cookie_pin).unwrap_or_default();
    let ip = conn.peer_addr().map(|a| a.ip()).unwrap_or(IpAddr::from([0, 0, 0, 0]));
    let locked = sh.locked_out(ip);
    let authed = !locked && !pin.is_empty() && same(&given, &pin);
    if !locked && !given.is_empty() {
        sh.note_pin(ip, &given, authed);
    }
    if path == "/icon.png" || path == "/apple-touch-icon.png" {
        return respond(&mut conn, "200 OK", "image/png", ICON_PNG, "Cache-Control: max-age=86400\r\n");
    }
    if path == "/prompter" {
        return respond(&mut conn, "200 OK", "text/html; charset=utf-8", PROMPTER_PAGE.as_bytes(), "");
    }
    if path == "/" || path == "/index.html" {
        let cookie = if authed && !pin.is_empty() { format!("Set-Cookie: cutaway_pin={pin}; Path=/; Max-Age=31536000\r\n") } else { String::new() };
        return respond(&mut conn, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes(), &cookie);
    }
    if locked {
        return respond(&mut conn, "429 Too Many Requests", "text/plain", b"Too many wrong PINs: wait a minute", "Retry-After: 60\r\n");
    }
    if !authed {
        return respond(&mut conn, "401 Unauthorized", "text/plain", b"PIN needed", "");
    }
    if path == "/api/ws" {
        return match (upgrade, ws_key) {
            (true, Some(key)) => websocket(conn, &key, sh, param("mode").as_deref() == Some("tally")),
            _ => respond(&mut conn, "426 Upgrade Required", "text/plain", b"WebSocket only", ""),
        };
    }
    if let Some(rest) = path.strip_prefix("/api/key/") {
        let (what, file) = rest.split_once('/').unwrap_or((rest, ""));
        let n: usize = file.trim_end_matches(".png").parse().unwrap_or(0);
        let kind = match what {
            "scene" => KeyKind::Scene(n.wrapping_sub(1)),
            "source" => KeyKind::Source(n.wrapping_sub(1)),
            _ => return respond(&mut conn, "404 Not Found", "text/plain", b"scene or source", ""),
        };
        let size = param("size").and_then(|v| v.parse().ok()).unwrap_or(144);
        let label = param("label").is_none_or(|v| v != "0");
        return match sh.key_png(kind, size, label) {
            Some(png) => respond(&mut conn, "200 OK", "image/png", &png, ""),
            None => respond(&mut conn, "404 Not Found", "text/plain", b"no such scene or source", ""),
        };
    }
    match path {
        "/api/state" => {
            sh.ensure_fresh();
            let s = sh.state.lock_ok().clone();
            respond(&mut conn, "200 OK", "application/json", s.as_bytes(), "")
        }
        "/api/prompter" => {
            let s = sh.prompter.lock_ok().clone();
            respond(&mut conn, "200 OK", "application/json", s.as_bytes(), "")
        }
        "/api/program.jpg" => match sh.program_jpeg() {
            // The same picture as the caller has: nothing to send again.
            Some((g, _)) if if_none_match.as_deref() == Some(&format!("\"{g}\"")) => respond(&mut conn, "304 Not Modified", "image/jpeg", b"", &format!("ETag: \"{g}\"\r\n")),
            Some((g, j)) => respond(&mut conn, "200 OK", "image/jpeg", &j, &format!("ETag: \"{g}\"\r\nAccess-Control-Expose-Headers: ETag\r\n")),
            None => respond(&mut conn, "503 Service Unavailable", "text/plain", b"no picture yet", ""),
        },
        p if p.starts_with("/api/tally/") => {
            let n: usize = p[11..].parse().unwrap_or(0);
            sh.ensure_fresh();
            let t = sh.tally.lock_ok().get(n.wrapping_sub(1)).cloned().unwrap_or_else(|| "off".into());
            respond(&mut conn, "200 OK", "text/plain", t.as_bytes(), "")
        }
        "/api/alive" => {
            // 410: Cutaway has let go already (the lease ran out).
            if sh.hold_alive(&param("who").unwrap_or_default()) {
                respond(&mut conn, "200 OK", "text/plain", b"ok", "")
            } else {
                respond(&mut conn, "410 Gone", "text/plain", b"not held", "")
            }
        }
        "/api/action" => {
            let Some(cmd) = param("do").or_else(|| param("action")) else {
                return respond(&mut conn, "400 Bad Request", "text/plain", b"add ?do=<command>", "");
            };
            if let Err(e) = Action::parse(&cmd) {
                return respond(&mut conn, "400 Bad Request", "text/plain", e.as_bytes(), "");
            }
            let result = sh.run(&cmd);
            if result.is_ok() {
                // (A page that says who it is gets its holds let go if it
                // stops saying it's still holding.)
                sh.note_hold(&param("who").unwrap_or_default(), &cmd);
            }
            match result {
                Ok(msg) => respond(&mut conn, "200 OK", "text/plain", msg.as_bytes(), ""),
                Err(e) => respond(&mut conn, "409 Conflict", "text/plain", e.as_bytes(), ""),
            }
        }
        _ => respond(&mut conn, "404 Not Found", "text/plain", b"not found", ""),
    }
}

/// Counts a connected controller for as long as it lives.
struct Client<'a>(&'a AtomicUsize);
impl<'a> Client<'a> {
    fn new(n: &'a AtomicUsize) -> Self {
        n.fetch_add(1, Ordering::Relaxed);
        Client(n)
    }
}
impl Drop for Client<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// What a tally-only page needs: each source's name and tally.
fn tally_message(s: &ShowState) -> String {
    let sources: Vec<_> = s.sources.iter().map(|x| serde_json::json!({ "name": x.name, "tally": x.tally })).collect();
    serde_json::json!({ "type": "tally", "sources": sources }).to_string()
}

/// A WebSocket client: state pushed as it changes, commands answered. A
/// `tally` client (a tally light page) gets only the tally, not the show.
fn websocket(conn: TcpStream, key: &str, sh: &RemoteShared, tally_only: bool) -> std::io::Result<()> {
    let mut w = conn.try_clone()?;
    write!(w, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", ws::accept_key(key))?;
    // The server pings every 15 s and a live client answers, so 40 s of
    // nothing means it's gone; a write stuck for 10 s means it stopped
    // reading. Either ends the connection (and lets go of what it held).
    conn.set_read_timeout(Some(READ_TIMEOUT))?;
    conn.set_write_timeout(Some(WRITE_TIMEOUT))?;
    sh.ensure_fresh();
    let _client = Client::new(&sh.clients);
    let writer = Arc::new(Mutex::new(w));
    let closed = Arc::new(AtomicBool::new(false));
    let send_json = |writer: &Mutex<TcpStream>, json: &str| -> std::io::Result<()> {
        let msg = format!(r#"{{"type":"state","state":{json}}}"#);
        ws::write_text(&mut *writer.lock_ok(), &msg)
    };
    std::thread::scope(|scope| {
        // Reader: commands in, answers out.
        let (w2, c2) = (writer.clone(), closed.clone());
        let mut r = conn;
        scope.spawn(move || {
            // What's held down from this controller (talkback, a pad, a PTZ
            // move), as the commands that let go: run when the connection
            // goes, so nothing stays stuck on.
            let mut held: Vec<String> = Vec::new();
            loop {
                match ws::read_message(&mut r) {
                    Ok(ws::Message::Text(t)) => {
                        let (cmd, id) = match serde_json::from_str::<serde_json::Value>(&t) {
                            Ok(v) if v.is_object() => {
                                if v.get("get").and_then(|g| g.as_str()) == Some("state") {
                                    let _ = if tally_only { ws::write_text(&mut *w2.lock_ok(), &tally_message(&sh.snapshot().1)) } else { send_json(&w2, &sh.json().1) };
                                    continue;
                                }
                                (v.get("do").and_then(|d| d.as_str()).unwrap_or("").to_string(), v.get("id").cloned())
                            }
                            _ => (t.trim().to_string(), None),
                        };
                        let result = sh.run(&cmd);
                        if result.is_ok() {
                            let c = cmd.trim().to_ascii_lowercase();
                            held.retain(|h| *h != c);
                            if let Some(up) = let_go_of(&c) {
                                held.push(up);
                            }
                        }
                        let reply = serde_json::json!({
                            "type": "reply",
                            "id": id,
                            "command": cmd,
                            "ok": result.is_ok(),
                            "message": match &result { Ok(m) | Err(m) => m },
                        });
                        if ws::write_text(&mut *w2.lock_ok(), &reply.to_string()).is_err() {
                            break;
                        }
                    }
                    Ok(ws::Message::Ping(p)) => {
                        let _ = ws::write_frame(&mut *w2.lock_ok(), 0xA, &p);
                    }
                    Ok(ws::Message::Close) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            c2.store(true, Ordering::Relaxed);
            // Let go first: the connection may be a dead one.
            for up in held {
                let _ = sh.run(&up);
            }
            let _ = ws::write_frame(&mut *w2.lock_ok(), 0x8, &[]);
            let _ = w2.lock_ok().shutdown(std::net::Shutdown::Both);
        });
        // Writer: the state whenever it changes (at most 10 times a second),
        // and a small tick when nothing has gone for two seconds, so a page
        // can tell a quiet show from a lost connection. The tick says how
        // long ago the app last published, so a page can tell a frozen app.
        let mut seen = u64::MAX;
        let mut last_tally = String::new();
        let mut last_ping = Instant::now();
        let mut last_sent = Instant::now();
        while !closed.load(Ordering::Relaxed) {
            let generation = sh.wait_change(seen, Duration::from_millis(500));
            if generation != seen {
                seen = generation;
                let sent = if tally_only {
                    // Only when a tally actually changed.
                    let msg = tally_message(&sh.snapshot().1);
                    if msg == last_tally {
                        Ok(())
                    } else {
                        last_tally = msg;
                        ws::write_text(&mut *writer.lock_ok(), &last_tally)
                    }
                } else {
                    send_json(&writer, &sh.json().1)
                };
                if sent.is_err() {
                    break;
                }
                last_sent = Instant::now();
                std::thread::sleep(Duration::from_millis(100));
            } else if last_sent.elapsed() >= TICK_EVERY {
                let tick = format!(r#"{{"type":"tick","age_ms":{}}}"#, sh.app_age_ms());
                if ws::write_text(&mut *writer.lock_ok(), &tick).is_err() {
                    break;
                }
                last_sent = Instant::now();
            }
            if last_ping.elapsed() > Duration::from_secs(15) {
                last_ping = Instant::now();
                if ws::write_frame(&mut *writer.lock_ok(), 0x9, b"cutaway").is_err() {
                    break;
                }
            }
        }
        closed.store(true, Ordering::Relaxed);
        let _ = writer.lock_ok().shutdown(std::net::Shutdown::Both);
    });
    Ok(())
}

/// For a command that starts something held down (talkback on, a pad
/// pressed, a PTZ camera moving), the command that lets it go.
fn let_go_of(cmd: &str) -> Option<String> {
    let parts: Vec<&str> = cmd.split(':').collect();
    match parts.as_slice() {
        ["talkback", "on"] => Some("talkback:off".into()),
        ["pad", which, "press"] => Some(format!("pad:{which}:release")),
        ["ptz", cam, dir, speed] if speed.parse::<f32>().is_ok() && !matches!(*dir, "preset") => Some(format!("ptz:{cam}:stop")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn held_commands_know_how_to_let_go() {
        assert_eq!(let_go_of("talkback:on").as_deref(), Some("talkback:off"));
        assert_eq!(let_go_of("pad:3:press").as_deref(), Some("pad:3:release"));
        assert_eq!(let_go_of("ptz:2:left:0.5").as_deref(), Some("ptz:2:stop"));
        assert_eq!(let_go_of("ptz:2:preset:1"), None);
        assert_eq!(let_go_of("scene:2"), None);
        assert_eq!(let_go_of("talkback"), None);
    }

    use super::*;

    #[test]
    fn meters_alone_do_not_push_the_state_more_than_twice_a_second() {
        use super::super::state::InputState;
        let (tx, _rx) = crossbeam_channel::unbounded::<Request>();
        let sh = RemoteShared::new(tx, Arc::new(|| {}));
        let mut s = ShowState { inputs: vec![InputState { name: "Mic".into(), level: -30.0, ..Default::default() }], ..Default::default() };
        sh.publish(&s);
        let first = sh.snapshot().0;
        s.inputs[0].level = -20.0;
        sh.publish(&s);
        assert_eq!(sh.snapshot().0, first, "too soon for meters alone");
        // Something else changing sends everything, meters included.
        s.recording = true;
        sh.publish(&s);
        assert_eq!(sh.snapshot().0, first + 1);
        assert_eq!(sh.snapshot().1.inputs[0].level, -20.0);
        // A meter move left waiting goes out once its time comes.
        s.inputs[0].level = -10.0;
        sh.publish(&s);
        assert_eq!(sh.snapshot().0, first + 1);
        std::thread::sleep(LEVELS_EVERY + Duration::from_millis(50));
        sh.publish(&s);
        assert_eq!(sh.snapshot().0, first + 2);
        assert_eq!(sh.snapshot().1.inputs[0].level, -10.0);
    }

    #[test]
    fn a_phone_that_stops_saying_it_holds_is_let_go() {
        let (tx, rx) = crossbeam_channel::unbounded::<Request>();
        let sh = RemoteShared::new(tx, Arc::new(|| {}));
        *sh.pin.lock_ok() = "1357".into();
        let port = crate::util::free_tcp_port();
        let server = RemoteServer::start(port, sh.clone()).unwrap();
        let seen = Arc::new(Mutex::new(Vec::<Action>::new()));
        let seen2 = seen.clone();
        // The app's side: answer everything, remember what it was asked.
        let worker = std::thread::spawn(move || {
            while let Ok(r) = rx.recv_timeout(Duration::from_secs(8)) {
                seen2.lock_ok().push(r.action);
                if let Some(reply) = r.reply {
                    let _ = reply.send(Ok("ok".into()));
                }
            }
        });
        assert!(get(port, "/api/action?do=talkback%3Aon&who=a&pin=1357").0.contains("200"));
        // A pad held by a page that keeps saying so, and one that lets go itself.
        assert!(get(port, "/api/action?do=talkback%3Aon&who=b&pin=1357").0.contains("200"));
        assert!(get(port, "/api/action?do=talkback%3Aoff&who=b&pin=1357").0.contains("200"));
        assert!(get(port, "/api/action?do=pad%3A%231%3Apress&who=c&pin=1357").0.contains("200"));
        for _ in 0..4 {
            std::thread::sleep(Duration::from_millis(900));
            assert!(get(port, "/api/alive?who=c&pin=1357").0.contains("200"));
        }
        // "a" went quiet: after the lease Cutaway itself lets go. "b" let go
        // itself, and its keepalives did nothing more.
        std::thread::sleep(Duration::from_millis(900));
        let offs = seen.lock_ok().iter().filter(|a| **a == Action::Talkback(super::super::Toggle::Off)).count();
        assert_eq!(offs, 2, "{:?}", seen.lock_ok());
        assert!(get(port, "/api/alive?who=a&pin=1357").0.contains("410"), "the lease ran out: not held any more");
        assert!(get(port, "/api/alive?who=b&pin=1357").0.contains("410"), "b let go itself");
        assert!(get(port, "/api/alive?who=c&pin=1357").0.contains("200"), "c kept saying so");
        drop(server);
        drop(worker);
    }

    #[test]
    fn holds_are_still_watched_after_the_server_restarts() {
        let (tx, rx) = crossbeam_channel::unbounded::<Request>();
        let sh = RemoteShared::new(tx, Arc::new(|| {}));
        *sh.pin.lock_ok() = "2222".into();
        drop(RemoteServer::start(crate::util::free_tcp_port(), sh.clone()).unwrap());
        let port = crate::util::free_tcp_port();
        let server = RemoteServer::start(port, sh.clone()).unwrap();
        let worker = std::thread::spawn(move || {
            let mut asked = Vec::new();
            while let Ok(r) = rx.recv_timeout(Duration::from_secs(6)) {
                asked.push(r.action);
                if let Some(reply) = r.reply {
                    let _ = reply.send(Ok("ok".into()));
                }
                if asked.len() == 2 {
                    break;
                }
            }
            asked
        });
        assert!(get(port, "/api/action?do=talkback%3Aon&who=a&pin=2222").0.contains("200"));
        let asked = worker.join().unwrap();
        assert_eq!(asked.last(), Some(&Action::Talkback(super::super::Toggle::Off)), "{asked:?}");
        drop(server);
    }

    fn get(port: u16, path: &str) -> (String, String) {
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(c, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
        let mut raw = Vec::new();
        c.read_to_end(&mut raw).unwrap();
        let s = String::from_utf8_lossy(&raw).to_string();
        let (head, body) = s.split_once("\r\n\r\n").unwrap_or((&s, ""));
        (head.lines().next().unwrap_or("").to_string(), body.to_string())
    }

    #[test]
    fn serves_page_state_and_actions_with_a_pin() {
        let (tx, rx) = crossbeam_channel::unbounded::<Request>();
        let sh = RemoteShared::new(tx, Arc::new(|| {}));
        *sh.pin.lock_ok() = "4321".into();
        *sh.state.lock_ok() = r#"{"recording":false}"#.into();
        *sh.tally.lock_ok() = vec!["program".into(), "off".into()];
        let port = crate::util::free_tcp_port();
        let server = RemoteServer::start(port, sh.clone()).unwrap();
        // The app's side: answer requests.
        let worker = std::thread::spawn(move || {
            if let Ok(r) = rx.recv_timeout(Duration::from_secs(5)) {
                assert_eq!(r.action, Action::Scene(super::super::Target::Index(1)));
                r.reply.unwrap().send(Ok("Scene 2".into())).unwrap();
            }
        });
        let (status, body) = get(port, "/");
        assert!(status.contains("200") && body.contains("Cutaway Remote"));
        assert!(get(port, "/api/state").0.contains("401"));
        // The icon is public too, so a home-screen shortcut gets it.
        assert!(get(port, "/apple-touch-icon.png").0.contains("200"));
        let (status, body) = get(port, "/api/state?pin=4321");
        assert!(status.contains("200") && body.contains("recording"), "{status} {body}");
        assert_eq!(get(port, "/api/tally/1?pin=4321").1, "program");
        let (status, body) = get(port, "/api/action?do=scene%3A2&pin=4321");
        assert!(status.contains("200") && body == "Scene 2", "{status} {body}");
        assert!(get(port, "/api/action?do=explode&pin=4321").0.contains("400"));
        worker.join().unwrap();
        // An old PIN asked again and again (a phone after the PIN changed)
        // counts once, so the right one still gets in.
        for _ in 0..10 {
            assert!(get(port, "/api/state?pin=1111").0.contains("401"));
        }
        assert!(get(port, "/api/state?pin=4321").0.contains("200"));
        // Guessing: five different wrong PINs and even the right one waits.
        for g in ["0000", "0001", "0002", "0003", "0004"] {
            assert!(get(port, &format!("/api/state?pin={g}")).0.contains("401"));
        }
        assert!(get(port, "/api/state?pin=4321").0.contains("429"));
        drop(server);
    }

    #[test]
    fn no_pin_lets_nothing_in() {
        let (tx, _rx) = crossbeam_channel::unbounded::<Request>();
        let sh = RemoteShared::new(tx, Arc::new(|| {}));
        let port = crate::util::free_tcp_port();
        let server = RemoteServer::start(port, sh).unwrap();
        assert!(get(port, "/").0.contains("200"));
        assert!(get(port, "/api/state").0.contains("401"));
        assert!(get(port, "/api/state?pin=").0.contains("401"));
        drop(server);
    }

    #[test]
    fn websocket_pushes_state_and_runs_commands() {
        use super::super::state::SceneState;
        let (tx, rx) = crossbeam_channel::unbounded::<Request>();
        let sh = RemoteShared::new(tx, Arc::new(|| {}));
        let mut s = ShowState { program: "Wide".into(), ..Default::default() };
        s.scenes.push(SceneState { name: "Wide".into(), program: true, preview: false });
        sh.publish(&s);
        *sh.pin.lock_ok() = "2468".into();
        let port = crate::util::free_tcp_port();
        let server = RemoteServer::start(port, sh.clone()).unwrap();
        let worker = std::thread::spawn(move || {
            let r = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(r.action, Action::Take);
            r.reply.unwrap().send(Ok("Taken".into())).unwrap();
        });
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(c, "GET /api/ws?pin=2468 HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
        let mut head = Vec::new();
        let mut b = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            c.read_exact(&mut b).unwrap();
            head.push(b[0]);
        }
        let head = String::from_utf8(head).unwrap();
        assert!(head.contains("101") && head.contains("s3pPLMBiTxaQ9kYGzzhZRbK+xOo="), "{head}");
        let first = match ws::read_message(&mut c).unwrap() {
            ws::Message::Text(t) => t,
            m => panic!("{m:?}"),
        };
        assert!(first.contains("\"program\":\"Wide\""), "{first}");
        // A change is pushed.
        s.program = "Host".into();
        sh.publish(&s);
        let next = match ws::read_message(&mut c).unwrap() {
            ws::Message::Text(t) => t,
            m => panic!("{m:?}"),
        };
        assert!(next.contains("\"program\":\"Host\""), "{next}");
        // A command, answered with its id.
        c.write_all(&ws::client_frame(0x1, br#"{"do":"take","id":7}"#)).unwrap();
        loop {
            if let ws::Message::Text(t) = ws::read_message(&mut c).unwrap() {
                if t.contains("reply") {
                    assert!(t.contains("\"id\":7") && t.contains("Taken") && t.contains("\"ok\":true"), "{t}");
                    break;
                }
            }
        }
        // Nothing changes: a tick still comes, within a few seconds.
        loop {
            if let ws::Message::Text(t) = ws::read_message(&mut c).unwrap() {
                if t.contains("\"tick\"") {
                    break;
                }
            }
        }
        // Key pictures.
        let (status, _) = get(port, "/api/key/scene/1.png?size=72&pin=2468");
        assert!(status.contains("200"), "{status}");
        assert!(get(port, "/api/key/scene/9.png?pin=2468").0.contains("404"));
        c.write_all(&ws::client_frame(0x8, b"")).unwrap();
        worker.join().unwrap();
        drop(server);
    }
}
