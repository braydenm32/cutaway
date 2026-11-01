//! OSC: control from show-control software (QLab, Bitfocus Companion,
//! TouchOSC, lighting desks) over UDP, with the show's state sent back so
//! buttons light up.
//!
//! Addresses are the commands with slashes (numbers count from 1):
//!
//! ```text
//! /cutaway/scene/3            take scene 3 (also /cutaway/scene 3, or "Host")
//! /cutaway/cut/3  /cutaway/preview/3  /cutaway/take  /cutaway/cut
//! /cutaway/source/2           source 2 full screen
//! /cutaway/record 1|0         on / off (no value: toggle); also live, auto,
//!                             captions, graphic/N, audio/N
//! /cutaway/marker ["name"]    /cutaway/transition/mix
//! /cutaway/gain/Mic -6.0      level in dB    /cutaway/fader/Mic 0.75 (0..1)
//! /cutaway/media/Clips/next
//! ```
//!
//! A button that sends 1 when pressed and 0 when released works for
//! everything: momentary commands (scene, take, marker...) ignore the 0.

use super::state::{clock, ShowState};
use super::{fader_db, Action, Request};
use crate::engine::Waker;
use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use std::collections::HashMap;
use std::net::{ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Blob(Vec<u8>),
    Nil,
}

impl Arg {
    fn number(&self) -> Option<f64> {
        match self {
            Arg::Int(i) => Some(*i as f64),
            Arg::Float(f) => Some(*f),
            Arg::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    pub addr: String,
    pub args: Vec<Arg>,
}

fn read_string(b: &[u8], pos: &mut usize) -> Option<String> {
    let start = *pos;
    let end = start + b.get(start..)?.iter().position(|&c| c == 0)?;
    let s = String::from_utf8_lossy(&b[start..end]).to_string();
    *pos = (end + 4) & !3;
    Some(s)
}

fn take<const N: usize>(b: &[u8], pos: &mut usize) -> Option<[u8; N]> {
    let v: [u8; N] = b.get(*pos..*pos + N)?.try_into().ok()?;
    *pos += N;
    Some(v)
}

/// Every message in a packet (bundles are opened).
pub fn decode(b: &[u8]) -> Vec<Message> {
    let mut out = Vec::new();
    decode_into(b, &mut out, 0);
    out
}

fn decode_into(b: &[u8], out: &mut Vec<Message>, depth: u8) {
    if depth > 4 {
        return;
    }
    if b.starts_with(b"#bundle\0") {
        let mut pos = 16;
        while let Some(size) = take::<4>(b, &mut pos) {
            let n = u32::from_be_bytes(size) as usize;
            let Some(el) = b.get(pos..pos + n) else { return };
            decode_into(el, out, depth + 1);
            pos += n;
        }
        return;
    }
    let mut pos = 0;
    let Some(addr) = read_string(b, &mut pos) else { return };
    if !addr.starts_with('/') {
        return;
    }
    let tags = if b.get(pos) == Some(&b',') { read_string(b, &mut pos).unwrap_or_default() } else { String::new() };
    let mut args = Vec::new();
    for t in tags.chars().skip(1) {
        let arg = match t {
            'i' => take::<4>(b, &mut pos).map(|v| Arg::Int(i32::from_be_bytes(v) as i64)),
            'h' => take::<8>(b, &mut pos).map(|v| Arg::Int(i64::from_be_bytes(v))),
            'f' => take::<4>(b, &mut pos).map(|v| Arg::Float(f32::from_be_bytes(v) as f64)),
            'd' => take::<8>(b, &mut pos).map(|v| Arg::Float(f64::from_be_bytes(v))),
            's' | 'S' => read_string(b, &mut pos).map(Arg::Str),
            'T' => Some(Arg::Bool(true)),
            'F' => Some(Arg::Bool(false)),
            'N' | 'I' => Some(Arg::Nil),
            'b' => take::<4>(b, &mut pos).and_then(|n| {
                let n = u32::from_be_bytes(n) as usize;
                let v = b.get(pos..pos + n)?.to_vec();
                pos = (pos + n + 3) & !3;
                Some(Arg::Blob(v))
            }),
            _ => None,
        };
        match arg {
            Some(a) => args.push(a),
            None => break,
        }
    }
    out.push(Message { addr, args });
}

fn put_string(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(s.as_bytes());
    out.push(0);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}

pub fn encode(m: &Message) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, &m.addr);
    let mut tags = String::from(",");
    let mut data = Vec::new();
    for a in &m.args {
        match a {
            Arg::Int(i) => {
                tags.push('i');
                data.extend_from_slice(&(*i as i32).to_be_bytes());
            }
            Arg::Float(f) => {
                tags.push('f');
                data.extend_from_slice(&(*f as f32).to_be_bytes());
            }
            Arg::Str(s) => {
                tags.push('s');
                put_string(&mut data, s);
            }
            Arg::Bool(b) => tags.push(if *b { 'T' } else { 'F' }),
            Arg::Nil => tags.push('N'),
            Arg::Blob(v) => {
                tags.push('b');
                data.extend_from_slice(&(v.len() as u32).to_be_bytes());
                data.extend_from_slice(v);
                while data.len() % 4 != 0 {
                    data.push(0);
                }
            }
        }
    }
    put_string(&mut out, &tags);
    out.extend(data);
    out
}

/// Commands that switch something on or off: a 1/0 value means on/off.
const TOGGLES: &[&str] = &["record", "rec", "live", "stream", "auto", "autoswitch", "captions", "cc", "graphic", "gfx", "overlay", "audio", "input", "talkback", "tb", "rotation", "rotate", "sponsors"];

/// The command an OSC message stands for (None: nothing to do, e.g. a
/// button release).
pub fn command_for(m: &Message) -> Option<Result<Action, String>> {
    let path = m.addr.trim_matches('/');
    let path = crate::util::strip_prefix_ci(path, "cutaway").map_or(path, |p| p.trim_start_matches('/'));
    let mut segs: Vec<String> = path.split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    if segs.is_empty() {
        return None;
    }
    let verb = segs.remove(0).to_lowercase();
    let strings: Vec<String> = m.args.iter().filter_map(|a| if let Arg::Str(s) = a { Some(s.clone()) } else { None }).collect();
    let number = m.args.iter().find_map(Arg::number);
    let mut target: Vec<String> = segs;
    target.extend(strings);
    let cmd = match verb.as_str() {
        "gain" => format!("gain:{}:{}", target.join(":"), number.unwrap_or(0.0)),
        "fader" => format!("gain:{}:{:.1}", target.join(":"), fader_db(number.unwrap_or(0.0))),
        v if TOGGLES.contains(&v) => {
            let mut parts = vec![v.to_string()];
            parts.extend(target);
            if let Some(n) = number {
                parts.push(if n >= 0.5 { "on".into() } else { "off".into() });
            }
            parts.join(":")
        }
        v => {
            // Momentary: a press (or no value) runs it, a release doesn't.
            if target.is_empty() {
                match number {
                    // "/cutaway/scene 3": the number is the scene.
                    Some(n) if matches!(v, "scene" | "cut" | "preview" | "pvw" | "source") && n >= 1.0 => target.push(format!("{}", n.round() as i64)),
                    Some(n) if n < 0.5 => return None,
                    _ => {}
                }
            } else if number.is_some_and(|n| n < 0.5) {
                return None;
            }
            let mut parts = vec![v.to_string()];
            parts.extend(target);
            parts.join(":")
        }
    };
    Some(Action::parse(&cmd))
}

/// The state as OSC messages (address → value), for feedback.
pub fn state_messages(s: &ShowState) -> Vec<Message> {
    let f = |v: bool| Arg::Float(if v { 1.0 } else { 0.0 });
    let msg = |addr: String, arg: Arg| Message { addr, args: vec![arg] };
    let mut v = vec![
        msg("/cutaway/program".into(), Arg::Str(s.program.clone())),
        msg("/cutaway/preview".into(), Arg::Str(s.preview.clone().unwrap_or_default())),
        msg("/cutaway/record".into(), f(s.recording)),
        msg("/cutaway/live".into(), f(s.live)),
        msg("/cutaway/auto".into(), f(s.auto)),
        msg("/cutaway/captions".into(), f(s.captions)),
        msg("/cutaway/transition".into(), Arg::Str(s.transition.clone())),
        msg("/cutaway/rec_time".into(), Arg::Str(if s.recording { clock(s.rec_seconds) } else { String::new() })),
        msg("/cutaway/live_time".into(), Arg::Str(if s.live { clock(s.live_seconds) } else { String::new() })),
        // As the phone and Stream Deck say it (live is on from Go live):
        // "off", "connecting", "live", "trouble" or "ending", and "1 of 2".
        msg("/cutaway/stream".into(), Arg::Str(s.stream.state.clone())),
        msg("/cutaway/stream/live".into(), Arg::Str(if s.stream.total > 0 { format!("{} of {}", s.stream.live_count, s.stream.total) } else { String::new() })),
    ];
    for t in ["cut", "mix", "wipe", "push", "dip"] {
        v.push(msg(format!("/cutaway/transition/{t}"), f(s.transition == t)));
    }
    for (i, sc) in s.scenes.iter().enumerate() {
        v.push(msg(format!("/cutaway/scene/{}", i + 1), f(sc.program)));
        v.push(msg(format!("/cutaway/preview/{}", i + 1), f(sc.preview)));
        v.push(msg(format!("/cutaway/scene/{}/name", i + 1), Arg::Str(sc.name.clone())));
    }
    for (i, src) in s.sources.iter().enumerate() {
        v.push(msg(format!("/cutaway/source/{}", i + 1), f(src.tally == "program")));
        v.push(msg(format!("/cutaway/tally/{}", i + 1), Arg::Str(src.tally.clone())));
    }
    for (i, g) in s.graphics.iter().enumerate() {
        v.push(msg(format!("/cutaway/graphic/{}", i + 1), f(g.on)));
    }
    for (i, a) in s.inputs.iter().enumerate() {
        v.push(msg(format!("/cutaway/audio/{}", i + 1), f(a.on)));
        // 0..1 for meters, in 2% steps so it isn't sent constantly.
        let level = (((a.level + 60.0) / 60.0).clamp(0.0, 1.0) * 50.0).round() / 50.0;
        v.push(msg(format!("/cutaway/level/{}", i + 1), Arg::Float(level as f64)));
    }
    v
}

/// Listens for OSC and sends state back.
pub struct Osc {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    feedback: Option<(UdpSocket, std::net::SocketAddr)>,
    sent: HashMap<String, Vec<Arg>>,
    last_full: Instant,
    pub port: u16,
}

impl Drop for Osc {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Osc {
    pub fn start(cfg: &crate::config::OscConfig, requests: Sender<Request>, waker: Waker) -> Result<Osc> {
        let sock = UdpSocket::bind(("0.0.0.0", cfg.port)).with_context(|| format!("can't use UDP port {} for OSC (is something else using it?)", cfg.port))?;
        sock.set_read_timeout(Some(Duration::from_millis(200)))?;
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        let thread = std::thread::Builder::new().name("osc".into()).spawn(move || {
            let mut buf = vec![0u8; 65536];
            while !s2.load(Ordering::Relaxed) {
                let Ok((n, _from)) = sock.recv_from(&mut buf) else { continue };
                for m in decode(&buf[..n]) {
                    let Some(parsed) = command_for(&m) else { continue };
                    match parsed {
                        Ok(action) => {
                            let _ = requests.send(Request { action, reply: None });
                            waker();
                        }
                        Err(e) => crate::logging::note(&format!("OSC {}: {e}", m.addr)),
                    }
                }
            }
        })?;
        let feedback = if cfg.feedback_host.trim().is_empty() {
            None
        } else {
            let addr = (cfg.feedback_host.trim(), cfg.feedback_port)
                .to_socket_addrs()
                .with_context(|| format!("can't find {}", cfg.feedback_host))?
                .find(|a| a.is_ipv4())
                .context("no IPv4 address for the OSC feedback host")?;
            Some((UdpSocket::bind(("0.0.0.0", 0))?, addr))
        };
        Ok(Osc { stop, thread: Some(thread), feedback, sent: HashMap::new(), last_full: Instant::now() - Duration::from_secs(60), port: cfg.port })
    }

    /// Send what changed since last time (everything every few seconds,
    /// for controllers that joined late).
    pub fn feedback(&mut self, s: &ShowState) {
        let Some((sock, to)) = &self.feedback else { return };
        let full = self.last_full.elapsed() > Duration::from_secs(5);
        if full {
            self.last_full = Instant::now();
        }
        for m in state_messages(s) {
            if !full && self.sent.get(&m.addr) == Some(&m.args) {
                continue;
            }
            let _ = sock.send_to(&encode(&m), to);
            self.sent.insert(m.addr, m.args);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::{MediaCmd, Target, Toggle};
    use super::*;

    fn m(addr: &str, args: Vec<Arg>) -> Message {
        Message { addr: addr.into(), args }
    }

    fn cmd(addr: &str, args: Vec<Arg>) -> Option<Action> {
        command_for(&m(addr, args)).map(|r| r.unwrap())
    }

    #[test]
    fn encodes_and_decodes() {
        let msg = m("/cutaway/gain/Mic", vec![Arg::Float(-6.0), Arg::Str("x".into()), Arg::Int(3), Arg::Bool(true)]);
        let b = encode(&msg);
        assert_eq!(b.len() % 4, 0);
        assert_eq!(decode(&b), vec![msg.clone()]);
        // A bundle with two messages.
        let mut bundle = b"#bundle\0".to_vec();
        bundle.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        for x in [&msg, &m("/cutaway/take", vec![])] {
            let e = encode(x);
            bundle.extend_from_slice(&(e.len() as u32).to_be_bytes());
            bundle.extend(e);
        }
        assert_eq!(decode(&bundle).len(), 2);
        assert!(decode(b"garbage").is_empty());
    }

    #[test]
    fn maps_addresses_to_commands() {
        assert_eq!(cmd("/cutaway/scene/3", vec![]), Some(Action::Scene(Target::Index(2))));
        assert_eq!(cmd("/cutaway/scene", vec![Arg::Int(3)]), Some(Action::Scene(Target::Index(2))));
        assert_eq!(cmd("/cutaway/scene", vec![Arg::Str("Host".into())]), Some(Action::Scene(Target::Name("Host".into()))));
        // Press runs it, release doesn't.
        assert_eq!(cmd("/cutaway/scene/3", vec![Arg::Float(1.0)]), Some(Action::Scene(Target::Index(2))));
        assert_eq!(cmd("/cutaway/scene/3", vec![Arg::Float(0.0)]), None);
        assert_eq!(cmd("/cutaway/take", vec![Arg::Float(0.0)]), None);
        assert_eq!(cmd("/cutaway/take", vec![Arg::Float(1.0)]), Some(Action::Take));
        assert_eq!(cmd("/cutaway/record", vec![]), Some(Action::Record(Toggle::Toggle)));
        assert_eq!(cmd("/cutaway/record", vec![Arg::Int(0)]), Some(Action::Record(Toggle::Off)));
        assert_eq!(cmd("/cutaway/graphic/2", vec![Arg::Float(1.0)]), Some(Action::Graphic(Target::Index(1), Toggle::On)));
        assert_eq!(cmd("/cutaway/audio", vec![Arg::Str("Mic 1".into()), Arg::Bool(false)]), Some(Action::Audio(Target::Name("Mic 1".into()), Toggle::Off)));
        assert_eq!(cmd("/cutaway/gain/Mic", vec![Arg::Float(-6.0)]), Some(Action::Gain(Target::Name("Mic".into()), -6.0)));
        assert!(matches!(cmd("/cutaway/fader/Mic", vec![Arg::Float(1.0)]), Some(Action::Gain(_, db)) if (db - 12.0).abs() < 0.2));
        assert_eq!(cmd("/cutaway/media/Clips/next", vec![]), Some(Action::Media(Target::Name("Clips".into()), MediaCmd::Next)));
        assert_eq!(cmd("/cutaway/marker", vec![Arg::Str("Guest arrives".into())]), Some(Action::Marker(Some("Guest arrives".into()))));
        assert_eq!(cmd("/cutaway/transition/wipe", vec![]), Some(Action::Transition(crate::config::TransitionMode::Wipe)));
        assert_eq!(cmd("/scene/2", vec![]), Some(Action::Scene(Target::Index(1))));
        assert!(command_for(&m("/cutaway/explode", vec![])).unwrap().is_err());
    }

    #[test]
    fn sends_changes_back() {
        let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let port = rx.local_addr().unwrap().port();
        let (tx, _rrx) = crossbeam_channel::unbounded();
        let cfg = crate::config::OscConfig { enabled: true, port: 0, feedback_host: "127.0.0.1".into(), feedback_port: port };
        let mut osc = Osc::start(&cfg, tx, Arc::new(|| {})).unwrap();
        let mut s = ShowState { program: "Wide".into(), recording: true, ..Default::default() };
        s.scenes.push(super::super::state::SceneState { name: "Wide".into(), program: true, preview: false });
        osc.feedback(&s);
        let mut got = HashMap::new();
        let mut buf = [0u8; 2048];
        while let Ok(n) = rx.recv(&mut buf) {
            for msg in decode(&buf[..n]) {
                got.insert(msg.addr, msg.args);
            }
            if got.contains_key("/cutaway/scene/1") && got.contains_key("/cutaway/record") {
                break;
            }
        }
        assert_eq!(got["/cutaway/record"], vec![Arg::Float(1.0)]);
        assert_eq!(got["/cutaway/scene/1"], vec![Arg::Float(1.0)]);
        assert_eq!(got["/cutaway/program"], vec![Arg::Str("Wide".into())]);
        // Unchanged state sends nothing new.
        rx.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
        while rx.recv(&mut buf).is_ok() {}
        osc.feedback(&s);
        assert!(rx.recv(&mut buf).is_err());
        s.recording = false;
        osc.feedback(&s);
        let n = rx.recv(&mut buf).unwrap();
        assert_eq!(decode(&buf[..n])[0], m("/cutaway/record", vec![Arg::Float(0.0)]));
    }
}
