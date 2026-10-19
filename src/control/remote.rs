//! The phone/tablet remote and HTTP API: a small web server built in.
//!
//! * `/` — the remote (scenes, record, go live, markers, graphics, audio,
//!   media players, tally links).
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

use super::keys::{self, KeyKind, KeySources};
use super::state::ShowState;
use super::{ws, Action, Request};
use crate::engine::{FrameSlot, Waker};
use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Sender};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const PAGE: &str = include_str!("remote.html");
const PROMPTER_PAGE: &str = include_str!("prompter.html");

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
    jpeg: Mutex<Option<(u64, Arc<Vec<u8>>)>>,
    /// The state, and a counter that moves whenever it changes.
    show: Mutex<(u64, ShowState)>,
    changed: Condvar,
    /// What key pictures are drawn from.
    pub keys: Mutex<KeySources>,
    /// The teleprompter's script as JSON ({version, text, segments}).
    pub prompter: Mutex<String>,
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
            show: Mutex::new((0, ShowState::default())),
            changed: Condvar::new(),
            keys: Mutex::new(KeySources::default()),
            prompter: Mutex::new("{}".into()),
        })
    }

    /// New state from the app; wakes WebSocket clients if anything changed.
    pub fn publish(&self, s: &ShowState) {
        let mut show = self.show.lock().unwrap();
        if show.1 == *s {
            return;
        }
        show.1 = s.clone();
        show.0 += 1;
        *self.state.lock().unwrap() = serde_json::to_string(s).unwrap_or_else(|_| "{}".into());
        *self.tally.lock().unwrap() = s.sources.iter().map(|x| x.tally.clone()).collect();
        self.changed.notify_all();
    }

    fn snapshot(&self) -> (u64, ShowState) {
        self.show.lock().unwrap().clone()
    }

    /// Wait (up to `timeout`) for the state to move past `seen`.
    fn wait_change(&self, seen: u64, timeout: Duration) -> u64 {
        let show = self.show.lock().unwrap();
        let (show, _) = self.changed.wait_timeout_while(show, timeout, |s| s.0 == seen).unwrap();
        show.0
    }

    fn key_png(&self, kind: KeyKind, size: u32, label: bool) -> Option<Vec<u8>> {
        let src = self.keys.lock().unwrap().clone();
        let state = self.snapshot().1;
        keys::png(&keys::render(&src, &state, kind, size, label)?)
    }

    /// Run a command for a controller and wait for the answer.
    fn run(&self, cmd: &str) -> Result<String, String> {
        let action = Action::parse(cmd)?;
        let (tx, rx) = bounded(1);
        self.requests.send(Request { action, reply: Some(tx) }).map_err(|_| "Cutaway is closing".to_string())?;
        (self.waker)();
        rx.recv_timeout(Duration::from_secs(5)).unwrap_or_else(|_| Err("Cutaway didn't answer".into()))
    }

    fn program_jpeg(&self) -> Option<Arc<Vec<u8>>> {
        let slot = self.program.lock().unwrap().clone()?;
        let (generation, frame) = slot.latest()?;
        if let Some((g, j)) = &*self.jpeg.lock().unwrap() {
            if *g == generation {
                return Some(j.clone());
            }
        }
        let img = image::RgbaImage::from_raw(frame.width as u32, frame.height as u32, frame.rgba.clone())?;
        let img = if img.width() > 640 {
            let h = (640 * img.height() / img.width()).max(2);
            image::imageops::resize(&img, 640, h, image::imageops::FilterType::Triangle)
        } else {
            img
        };
        let rgb = image::DynamicImage::ImageRgba8(img).to_rgb8();
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 70).encode_image(&rgb).ok()?;
        let j = Arc::new(out);
        *self.jpeg.lock().unwrap() = Some((generation, j.clone()));
        Some(j)
    }
}

pub struct RemoteServer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub port: u16,
}

impl Drop for RemoteServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl RemoteServer {
    pub fn start(port: u16, shared: Arc<RemoteShared>) -> Result<RemoteServer> {
        let listener = TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("can't use port {port} for the remote (is something else using it?)"))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let thread = std::thread::Builder::new().name("remote".into()).spawn(move || {
            while !s2.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((conn, _)) => {
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
        Ok(RemoteServer { stop, thread: Some(thread), port })
    }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn query(q: &str) -> Vec<(String, String)> {
    q.split('&').filter(|kv| !kv.is_empty()).map(|kv| {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        (percent_decode(k), percent_decode(v))
    }).collect()
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
    let mut upgrade = false;
    for l in lines {
        let lower = l.to_lowercase();
        if lower.starts_with("upgrade:") && lower.contains("websocket") {
            upgrade = true;
        } else if lower.starts_with("sec-websocket-key:") {
            ws_key = Some(l[18..].trim().to_string());
        }
        if let Some(v) = lower.strip_prefix("cookie:") {
            for c in v.split(';') {
                if let Some(p) = c.trim().strip_prefix("cutaway_pin=") {
                    cookie_pin = Some(p.to_string());
                }
            }
        } else if lower.starts_with("x-cutaway-pin:") {
            header_pin = Some(l[14..].trim().to_string());
        }
    }
    if method == "OPTIONS" {
        return respond(&mut conn, "204 No Content", "text/plain", b"", "Access-Control-Allow-Headers: X-Cutaway-Pin\r\n");
    }
    // The page itself is public (it asks for the PIN); everything else isn't.
    let pin = sh.pin.lock().unwrap().clone();
    let given = param("pin").or(header_pin).or(cookie_pin).unwrap_or_default();
    let authed = pin.is_empty() || given == pin;
    if path == "/prompter" {
        return respond(&mut conn, "200 OK", "text/html; charset=utf-8", PROMPTER_PAGE.as_bytes(), "");
    }
    if path == "/" || path == "/index.html" {
        let cookie = if authed && !pin.is_empty() { format!("Set-Cookie: cutaway_pin={pin}; Path=/; Max-Age=31536000\r\n") } else { String::new() };
        return respond(&mut conn, "200 OK", "text/html; charset=utf-8", PAGE.as_bytes(), &cookie);
    }
    if !authed {
        return respond(&mut conn, "401 Unauthorized", "text/plain", b"PIN needed", "");
    }
    if path == "/api/ws" {
        return match (upgrade, ws_key) {
            (true, Some(key)) => websocket(conn, &key, sh),
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
            let s = sh.state.lock().unwrap().clone();
            respond(&mut conn, "200 OK", "application/json", s.as_bytes(), "")
        }
        "/api/prompter" => {
            let s = sh.prompter.lock().unwrap().clone();
            respond(&mut conn, "200 OK", "application/json", s.as_bytes(), "")
        }
        "/api/program.jpg" => match sh.program_jpeg() {
            Some(j) => respond(&mut conn, "200 OK", "image/jpeg", &j, ""),
            None => respond(&mut conn, "503 Service Unavailable", "text/plain", b"no picture yet", ""),
        },
        p if p.starts_with("/api/tally/") => {
            let n: usize = p[11..].parse().unwrap_or(0);
            let t = sh.tally.lock().unwrap().get(n.wrapping_sub(1)).cloned().unwrap_or_else(|| "off".into());
            respond(&mut conn, "200 OK", "text/plain", t.as_bytes(), "")
        }
        "/api/action" => {
            let Some(cmd) = param("do").or_else(|| param("action")) else {
                return respond(&mut conn, "400 Bad Request", "text/plain", b"add ?do=<command>", "");
            };
            if let Err(e) = Action::parse(&cmd) {
                return respond(&mut conn, "400 Bad Request", "text/plain", e.as_bytes(), "");
            }
            match sh.run(&cmd) {
                Ok(msg) => respond(&mut conn, "200 OK", "text/plain", msg.as_bytes(), ""),
                Err(e) => respond(&mut conn, "409 Conflict", "text/plain", e.as_bytes(), ""),
            }
        }
        _ => respond(&mut conn, "404 Not Found", "text/plain", b"not found", ""),
    }
}

/// A WebSocket client: state pushed as it changes, commands answered.
fn websocket(conn: TcpStream, key: &str, sh: &RemoteShared) -> std::io::Result<()> {
    let mut w = conn.try_clone()?;
    write!(w, "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", ws::accept_key(key))?;
    conn.set_read_timeout(None)?;
    let writer = Arc::new(Mutex::new(w));
    let closed = Arc::new(AtomicBool::new(false));
    let send_state = |writer: &Mutex<TcpStream>, s: &ShowState| -> std::io::Result<()> {
        let msg = serde_json::json!({ "type": "state", "state": s }).to_string();
        ws::write_text(&mut *writer.lock().unwrap(), &msg)
    };
    std::thread::scope(|scope| {
        // Reader: commands in, answers out.
        let (w2, c2) = (writer.clone(), closed.clone());
        let mut r = conn;
        scope.spawn(move || {
            loop {
                match ws::read_message(&mut r) {
                    Ok(ws::Message::Text(t)) => {
                        let (cmd, id) = match serde_json::from_str::<serde_json::Value>(&t) {
                            Ok(v) if v.is_object() => {
                                if v.get("get").and_then(|g| g.as_str()) == Some("state") {
                                    let _ = send_state(&w2, &sh.snapshot().1);
                                    continue;
                                }
                                (v.get("do").and_then(|d| d.as_str()).unwrap_or("").to_string(), v.get("id").cloned())
                            }
                            _ => (t.trim().to_string(), None),
                        };
                        let result = sh.run(&cmd);
                        let reply = serde_json::json!({
                            "type": "reply",
                            "id": id,
                            "command": cmd,
                            "ok": result.is_ok(),
                            "message": match &result { Ok(m) | Err(m) => m },
                        });
                        if ws::write_text(&mut *w2.lock().unwrap(), &reply.to_string()).is_err() {
                            break;
                        }
                    }
                    Ok(ws::Message::Ping(p)) => {
                        let _ = ws::write_frame(&mut *w2.lock().unwrap(), 0xA, &p);
                    }
                    Ok(ws::Message::Close) | Err(_) => break,
                    Ok(_) => {}
                }
            }
            c2.store(true, Ordering::Relaxed);
            let _ = ws::write_frame(&mut *w2.lock().unwrap(), 0x8, &[]);
            let _ = w2.lock().unwrap().shutdown(std::net::Shutdown::Both);
        });
        // Writer: the state whenever it changes (at most 10 times a second).
        let mut seen = u64::MAX;
        let mut last_ping = Instant::now();
        while !closed.load(Ordering::Relaxed) {
            let (generation, state) = sh.snapshot();
            if generation != seen {
                seen = generation;
                if send_state(&writer, &state).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            if last_ping.elapsed() > Duration::from_secs(15) {
                last_ping = Instant::now();
                if ws::write_frame(&mut *writer.lock().unwrap(), 0x9, b"cutaway").is_err() {
                    break;
                }
            }
            sh.wait_change(seen, Duration::from_millis(500));
        }
        closed.store(true, Ordering::Relaxed);
        let _ = writer.lock().unwrap().shutdown(std::net::Shutdown::Both);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        *sh.pin.lock().unwrap() = "4321".into();
        *sh.state.lock().unwrap() = r#"{"recording":false}"#.into();
        *sh.tally.lock().unwrap() = vec!["program".into(), "off".into()];
        let server = RemoteServer::start(18791, sh.clone()).unwrap();
        // The app's side: answer requests.
        let worker = std::thread::spawn(move || {
            if let Ok(r) = rx.recv_timeout(Duration::from_secs(5)) {
                assert_eq!(r.action, Action::Scene(super::super::Target::Index(1)));
                r.reply.unwrap().send(Ok("Scene 2".into())).unwrap();
            }
        });
        let (status, body) = get(18791, "/");
        assert!(status.contains("200") && body.contains("Cutaway Remote"));
        assert!(get(18791, "/api/state").0.contains("401"));
        let (status, body) = get(18791, "/api/state?pin=4321");
        assert!(status.contains("200") && body.contains("recording"), "{status} {body}");
        assert_eq!(get(18791, "/api/tally/1?pin=4321").1, "program");
        let (status, body) = get(18791, "/api/action?do=scene%3A2&pin=4321");
        assert!(status.contains("200") && body == "Scene 2", "{status} {body}");
        assert!(get(18791, "/api/action?do=explode&pin=4321").0.contains("400"));
        worker.join().unwrap();
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
        let server = RemoteServer::start(18792, sh.clone()).unwrap();
        let worker = std::thread::spawn(move || {
            let r = rx.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(r.action, Action::Take);
            r.reply.unwrap().send(Ok("Taken".into())).unwrap();
        });
        let mut c = TcpStream::connect(("127.0.0.1", 18792)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(c, "GET /api/ws HTTP/1.1\r\nHost: x\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n").unwrap();
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
        // Key pictures.
        let (status, _) = get(18792, "/api/key/scene/1.png?size=72");
        assert!(status.contains("200"), "{status}");
        assert!(get(18792, "/api/key/scene/9.png").0.contains("404"));
        c.write_all(&ws::client_frame(0x8, b"")).unwrap();
        worker.join().unwrap();
        drop(server);
    }
}
