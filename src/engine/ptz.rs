//! Pan, tilt and zoom.
//!
//! - Network PTZ cameras over VISCA (over IP with Sony's header, or plain
//!   over TCP or UDP), each with a small thread so a slow camera never
//!   holds the desk up.
//! - NDI PTZ cameras through NDI's own controls (the receiver does it).
//! - Virtual PTZ cameras: a shot cut from a wide (ideally 4K) camera that
//!   pans, tilts and zooms like a real one, with presets it glides to.
//!
//! All three take the same [`Move`]s, so the PTZ panel, Stream Deck keys,
//! the remote and commands work the same for any of them.

use crate::util::LockExt;
use super::sources::{Running, SourceSlot};
use crate::config::{PtzConfig, PtzControl, SourceConfig};
use crate::media::{Ns, VideoFrame};
use anyhow::{anyhow, Result};
use crossbeam_channel::{bounded, unbounded, Receiver, RecvTimeoutError, Sender};
use std::io::Write;
use std::net::{TcpStream, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// A move for a PTZ camera. Speeds are -1..1 (right, up and in are
/// positive); presets count from 0.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Move {
    Drive { pan: f32, tilt: f32 },
    Zoom(f32),
    Stop,
    Home,
    Recall(usize),
    Store(usize),
}

/// One camera's controls.
pub struct PtzCamera {
    /// Real cameras: moves go to their thread (or the NDI receiver).
    tx: Option<Sender<Move>>,
    /// For the NDI receiver to pick up.
    pub ndi_rx: Option<Receiver<Move>>,
    /// Virtual cameras.
    pub virt: Option<Arc<VirtualPtz>>,
    pub status: Arc<Mutex<String>>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
    /// Speed (0.1–1) and whether pan and tilt are flipped.
    options: Mutex<(f32, bool, bool)>,
    /// What it was made from (to know when it must be remade).
    pub made_for: (PtzControl, String, u8),
}

impl Drop for PtzCamera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx = None;
        if let Some(t) = self.thread.lock_ok().take() {
            let _ = t.join();
        }
    }
}

impl PtzCamera {
    /// Controls for a source, if it has any.
    pub fn for_source(cfg: &SourceConfig) -> Option<Arc<PtzCamera>> {
        let status = Arc::new(Mutex::new(String::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let p = &cfg.ptz;
        let base = |kind| PtzCamera {
            tx: None,
            ndi_rx: None,
            virt: None,
            status: status.clone(),
            stop: stop.clone(),
            thread: Mutex::new(None),
            options: Mutex::new((p.speed.clamp(0.1, 1.0), p.invert_pan, p.invert_tilt)),
            made_for: (if cfg.virtual_ptz_of().is_some() { PtzControl::None } else { kind }, p.address.trim().to_string(), p.camera),
        };
        if cfg.virtual_ptz_of().is_some() {
            let mut cam = base(PtzControl::None);
            cam.virt = Some(VirtualPtz::new(p));
            *status.lock_ok() = "virtual".into();
            return Some(Arc::new(cam));
        }
        match p.control {
            PtzControl::None => None,
            PtzControl::Ndi => {
                if !cfg.device.trim().to_lowercase().starts_with("ndi:") {
                    *status.lock_ok() = "NDI control needs an NDI source".into();
                }
                let (tx, rx) = unbounded();
                let mut cam = base(PtzControl::Ndi);
                cam.tx = Some(tx);
                cam.ndi_rx = Some(rx);
                Some(Arc::new(cam))
            }
            kind => {
                let (tx, rx) = unbounded();
                let mut cam = base(kind);
                cam.tx = Some(tx);
                let (addr, camera, st, s2) = (p.address.trim().to_string(), p.camera.clamp(1, 7), status.clone(), stop.clone());
                *status.lock_ok() = if addr.is_empty() { "no address set".into() } else { format!("{addr} (not used yet)") };
                let t = std::thread::Builder::new().name(format!("ptz {}", cfg.name)).spawn(move || visca_worker(kind, &addr, camera, rx, st, s2)).ok();
                *cam.thread.lock_ok() = t;
                Some(Arc::new(cam))
            }
        }
    }

    /// Move the camera. Speeds are scaled by the camera's speed setting and
    /// flipped if it's mounted upside down.
    pub fn send(&self, m: Move) {
        let (speed, flip_pan, flip_tilt) = *self.options.lock_ok();
        let m = match m {
            Move::Drive { pan, tilt } => Move::Drive {
                pan: pan.clamp(-1.0, 1.0) * speed * if flip_pan { -1.0 } else { 1.0 },
                tilt: tilt.clamp(-1.0, 1.0) * speed * if flip_tilt { -1.0 } else { 1.0 },
            },
            Move::Zoom(z) => Move::Zoom(z.clamp(-1.0, 1.0) * speed),
            other => other,
        };
        if let Some(v) = &self.virt {
            v.apply(m);
        } else if let Some(tx) = &self.tx {
            let _ = tx.send(m);
        }
    }

    pub fn status(&self) -> String {
        self.status.lock_ok().clone()
    }

    /// New settings that don't need it remade (speed, flips, a virtual
    /// camera's presets and limits).
    pub fn configure(&self, p: &PtzConfig) {
        *self.options.lock_ok() = (p.speed.clamp(0.1, 1.0), p.invert_pan, p.invert_tilt);
        if let Some(v) = &self.virt {
            v.configure(p);
        }
    }

    /// Whether these settings need a new connection (or a new kind).
    pub fn needs_remaking(&self, cfg: &SourceConfig) -> bool {
        let kind = if cfg.virtual_ptz_of().is_some() { PtzControl::None } else { cfg.ptz.control };
        self.made_for != (kind, cfg.ptz.address.trim().to_string(), cfg.ptz.camera) || self.virt.is_some() != cfg.virtual_ptz_of().is_some()
    }
}

// ---------------------------------------------------------------------------
// VISCA

/// The VISCA bytes for a move (camera address 1–7).
pub fn visca_bytes(camera: u8, m: Move) -> Vec<Vec<u8>> {
    let a = 0x80 | camera.clamp(1, 7);
    match m {
        Move::Drive { pan, tilt } => {
            // Pan speeds 1–0x18, tilt 1–0x14.
            let ps = ((pan.abs() * 0x18 as f32).round() as u8).clamp(1, 0x18);
            let ts = ((tilt.abs() * 0x14 as f32).round() as u8).clamp(1, 0x14);
            let pd = if pan > 0.01 { 0x02 } else if pan < -0.01 { 0x01 } else { 0x03 };
            let td = if tilt > 0.01 { 0x01 } else if tilt < -0.01 { 0x02 } else { 0x03 };
            vec![vec![a, 0x01, 0x06, 0x01, ps, ts, pd, td, 0xff]]
        }
        Move::Zoom(z) => {
            let p = ((z.abs() * 7.0).round() as u8).min(7);
            let b = if z > 0.01 { 0x20 | p } else if z < -0.01 { 0x30 | p } else { 0x00 };
            vec![vec![a, 0x01, 0x04, 0x07, b, 0xff]]
        }
        Move::Stop => vec![vec![a, 0x01, 0x06, 0x01, 0x01, 0x01, 0x03, 0x03, 0xff], vec![a, 0x01, 0x04, 0x07, 0x00, 0xff]],
        Move::Home => vec![vec![a, 0x01, 0x06, 0x04, 0xff]],
        Move::Recall(n) => vec![vec![a, 0x01, 0x04, 0x3f, 0x02, n.min(127) as u8, 0xff]],
        Move::Store(n) => vec![vec![a, 0x01, 0x04, 0x3f, 0x01, n.min(127) as u8, 0xff]],
    }
}

/// Sony's VISCA-over-IP header around a command.
pub fn visca_ip_packet(payload_type: [u8; 2], seq: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(8 + payload.len());
    p.extend_from_slice(&payload_type);
    p.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

enum Link {
    Udp(UdpSocket),
    Tcp(TcpStream),
}

fn connect(kind: PtzControl, addr: &str) -> Result<Link> {
    let with_port = if addr.contains(':') { addr.to_string() } else { format!("{addr}:{}", kind.default_port()) };
    let sa = with_port.to_socket_addrs().map_err(|e| anyhow!("{addr}: {e}"))?.next().ok_or_else(|| anyhow!("{addr}: no address"))?;
    Ok(match kind {
        PtzControl::ViscaTcp => {
            let s = TcpStream::connect_timeout(&sa, Duration::from_secs(2)).map_err(|e| anyhow!("couldn't reach {with_port}: {e}"))?;
            s.set_nodelay(true).ok();
            s.set_write_timeout(Some(Duration::from_secs(2))).ok();
            Link::Tcp(s)
        }
        _ => {
            let s = UdpSocket::bind("0.0.0.0:0")?;
            s.connect(sa)?;
            s.set_nonblocking(true).ok();
            Link::Udp(s)
        }
    })
}

fn visca_worker(kind: PtzControl, addr: &str, camera: u8, rx: Receiver<Move>, status: Arc<Mutex<String>>, stop: Arc<AtomicBool>) {
    let mut link: Option<Link> = None;
    let mut seq: u32 = 0;
    let set = |s: String| *status.lock_ok() = s;
    while !stop.load(Ordering::Relaxed) {
        let m = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(m) => m,
            Err(RecvTimeoutError::Timeout) => {
                // Drain replies (acks and completions) so buffers don't fill.
                if let Some(Link::Udp(s)) = &link {
                    let mut buf = [0u8; 64];
                    while s.recv(&mut buf).is_ok() {}
                }
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if addr.is_empty() {
            set("no address set".into());
            continue;
        }
        if link.is_none() {
            match connect(kind, addr) {
                Ok(l) => {
                    if kind == PtzControl::ViscaIp {
                        // Start the sequence numbers afresh.
                        if let Link::Udp(s) = &l {
                            let _ = s.send(&visca_ip_packet([0x02, 0x00], 0, &[0x01]));
                        }
                        seq = 1;
                    }
                    link = Some(l);
                    set(format!("connected to {addr}"));
                }
                Err(e) => {
                    set(format!("{e:#}"));
                    continue;
                }
            }
        }
        let mut failed = None;
        for cmd in visca_bytes(camera, m) {
            let r = match link.as_mut().unwrap() {
                Link::Udp(s) if kind == PtzControl::ViscaIp => {
                    let r = s.send(&visca_ip_packet([0x01, 0x00], seq, &cmd));
                    seq = seq.wrapping_add(1);
                    r.map(|_| ())
                }
                Link::Udp(s) => s.send(&cmd).map(|_| ()),
                Link::Tcp(s) => s.write_all(&cmd),
            };
            if let Err(e) = r {
                failed = Some(e);
                break;
            }
        }
        if let Some(e) = failed {
            set(format!("lost {addr}: {e}"));
            link = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Virtual PTZ

/// A virtual camera's position: the centre of the shot (0..1 across and
/// down the source) and how far in (1 = the whole picture).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shot {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}

impl Default for Shot {
    fn default() -> Self {
        Shot { x: 0.5, y: 0.5, zoom: 1.0 }
    }
}

struct VirtState {
    pos: Shot,
    drive: [f32; 3],
    goal: Option<Shot>,
    vel: [f32; 3],
    presets: Vec<Shot>,
    max_zoom: f32,
    glide: f32,
}

/// Live state of a virtual PTZ camera.
pub struct VirtualPtz {
    st: Mutex<VirtState>,
}

impl VirtualPtz {
    pub fn new(cfg: &PtzConfig) -> Arc<VirtualPtz> {
        let v = Arc::new(VirtualPtz {
            st: Mutex::new(VirtState { pos: Shot::default(), drive: [0.0; 3], goal: None, vel: [0.0; 3], presets: Vec::new(), max_zoom: 3.0, glide: 1.5 }),
        });
        v.configure(cfg);
        // It starts on preset 1 (where it's meant to be), not the whole picture.
        {
            let mut st = v.st.lock_ok();
            if let Some(&first) = st.presets.first() {
                st.pos = first;
            }
        }
        v
    }

    /// Presets and limits from the settings.
    pub fn configure(&self, cfg: &PtzConfig) {
        let mut st = self.st.lock_ok();
        st.presets = cfg.presets.iter().map(|p| Shot { x: p.x, y: p.y, zoom: p.zoom.max(1.0) }).collect();
        st.max_zoom = cfg.max_zoom.clamp(1.0, 4.0);
        st.glide = cfg.glide_seconds.clamp(0.1, 10.0);
    }

    pub fn position(&self) -> Shot {
        self.st.lock_ok().pos
    }

    fn apply(&self, m: Move) {
        let mut st = self.st.lock_ok();
        match m {
            Move::Drive { pan, tilt } => {
                st.drive[0] = pan;
                st.drive[1] = tilt;
                st.goal = None;
            }
            Move::Zoom(z) => {
                st.drive[2] = z;
                st.goal = None;
            }
            Move::Stop => st.drive = [0.0; 3],
            Move::Home => {
                st.drive = [0.0; 3];
                st.goal = Some(Shot::default());
            }
            Move::Recall(n) => {
                if let Some(p) = st.presets.get(n).copied() {
                    st.drive = [0.0; 3];
                    st.goal = Some(p);
                }
            }
            Move::Store(n) => {
                let p = st.pos;
                if n >= st.presets.len() {
                    st.presets.resize(n + 1, Shot::default());
                }
                st.presets[n] = p;
            }
        }
    }

    /// Move on by `dt` seconds; returns where the shot is.
    pub fn step(&self, dt: f32) -> Shot {
        let mut st = self.st.lock_ok();
        let dt = dt.clamp(0.0, 0.25);
        let max_zoom = st.max_zoom;
        if st.drive != [0.0; 3] {
            // Full speed crosses half the visible picture a second; zoom
            // doubles in about a second and a half.
            let z = st.pos.zoom;
            st.pos.x += st.drive[0] * 0.5 / z * dt;
            st.pos.y -= st.drive[1] * 0.5 / z * dt;
            st.pos.zoom = (z * (st.drive[2] * 0.45 * dt).exp()).clamp(1.0, max_zoom);
            st.vel = [0.0; 3];
        } else if let Some(goal) = st.goal {
            // A critically damped spring: eases in and out.
            let omega = 4.75 / st.glide;
            let steps = ((omega * dt) / 0.25).ceil().max(1.0) as usize;
            let h = dt / steps as f32;
            let target = [goal.x, goal.y, goal.zoom.ln()];
            for _ in 0..steps {
                let mut cur = [st.pos.x, st.pos.y, st.pos.zoom.max(1.0).ln()];
                for k in 0..3 {
                    let acc = omega * omega * (target[k] - cur[k]) - 2.0 * omega * st.vel[k];
                    st.vel[k] += acc * h;
                    cur[k] += st.vel[k] * h;
                }
                st.pos = Shot { x: cur[0], y: cur[1], zoom: cur[2].exp() };
            }
            let close = (st.pos.x - goal.x).abs() < 1e-4 && (st.pos.y - goal.y).abs() < 1e-4 && (st.pos.zoom - goal.zoom).abs() < 1e-3;
            if close {
                st.pos = goal;
                st.goal = None;
                st.vel = [0.0; 3];
            }
        }
        st.pos.zoom = st.pos.zoom.clamp(1.0, max_zoom);
        // Keep the centre where the shot stays inside the picture.
        let half = 0.5 / st.pos.zoom;
        st.pos.x = st.pos.x.clamp(half, 1.0 - half);
        st.pos.y = st.pos.y.clamp(half, 1.0 - half);
        st.pos
    }
}

/// The part of a `src`-sized picture a shot shows, for an `out`-shaped
/// output: x, y, width, height in source pixels.
pub fn shot_rect(s: Shot, src: (u32, u32), out: (u32, u32)) -> [f32; 4] {
    let (sw, sh) = (src.0.max(2) as f32, src.1.max(2) as f32);
    let aspect = out.0.max(2) as f32 / out.1.max(2) as f32;
    // The biggest rectangle of the output's shape that fits.
    let (bw, bh) = if sw / sh > aspect { (sh * aspect, sh) } else { (sw, sw / aspect) };
    let (w, h) = (bw / s.zoom.max(1.0), bh / s.zoom.max(1.0));
    let cx = (s.x * sw).clamp(w / 2.0, sw - w / 2.0);
    let cy = (s.y * sh).clamp(h / 2.0, sh - h / 2.0);
    [cx - w / 2.0, cy - h / 2.0, w, h]
}

static WATCHER_IDS: AtomicU64 = AtomicU64::new(1 << 49);

struct VirtualRunning {
    stop: Arc<AtomicBool>,
    parent: Arc<SourceSlot>,
    watcher: u64,
    thread: Option<JoinHandle<()>>,
}

impl Drop for VirtualRunning {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.parent.remove_watcher(self.watcher);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Run a virtual PTZ camera cut from `parent`.
pub fn start_virtual(cfg: &SourceConfig, parent_name: &str, parent: Arc<SourceSlot>, slot: Arc<SourceSlot>, virt: Arc<VirtualPtz>) -> Result<(Box<dyn Running>, String)> {
    let (w, h) = (cfg.width, cfg.height);
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = bounded::<VideoFrame>(3);
    let watcher = WATCHER_IDS.fetch_add(1, Ordering::Relaxed);
    parent.add_watcher(watcher, tx);
    let s2 = stop.clone();
    let thread = std::thread::Builder::new().name(format!("virtual ptz {}", slot.index)).spawn(move || {
        let mut last: Option<Ns> = None;
        while !s2.load(Ordering::Relaxed) {
            let mut frame = match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(f) => f,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            while let Ok(f) = rx.try_recv() {
                frame = f;
            }
            let dt = last.map(|l| (frame.pts - l) as f32 / 1e9).unwrap_or(0.0);
            last = Some(frame.pts);
            let shot = virt.step(dt);
            slot.push(frame.crop_scaled(shot_rect(shot, (frame.width, frame.height), (w, h)), w, h));
        }
    })?;
    Ok((Box::new(VirtualRunning { stop, parent, watcher, thread: Some(thread) }), format!("virtual PTZ in {parent_name}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visca_commands() {
        assert_eq!(visca_bytes(1, Move::Home), vec![vec![0x81, 0x01, 0x06, 0x04, 0xff]]);
        assert_eq!(visca_bytes(1, Move::Recall(2)), vec![vec![0x81, 0x01, 0x04, 0x3f, 0x02, 0x02, 0xff]]);
        assert_eq!(visca_bytes(2, Move::Store(0)), vec![vec![0x82, 0x01, 0x04, 0x3f, 0x01, 0x00, 0xff]]);
        // Full speed right and up.
        assert_eq!(visca_bytes(1, Move::Drive { pan: 1.0, tilt: 1.0 }), vec![vec![0x81, 0x01, 0x06, 0x01, 0x18, 0x14, 0x02, 0x01, 0xff]]);
        // Slow left only.
        assert_eq!(visca_bytes(1, Move::Drive { pan: -0.1, tilt: 0.0 }), vec![vec![0x81, 0x01, 0x06, 0x01, 0x02, 0x01, 0x01, 0x03, 0xff]]);
        assert_eq!(visca_bytes(1, Move::Zoom(1.0)), vec![vec![0x81, 0x01, 0x04, 0x07, 0x27, 0xff]]);
        assert_eq!(visca_bytes(1, Move::Zoom(-0.5)), vec![vec![0x81, 0x01, 0x04, 0x07, 0x34, 0xff]]);
        assert_eq!(visca_bytes(1, Move::Stop).len(), 2);
        assert_eq!(visca_ip_packet([0x01, 0x00], 5, &[0x81, 0x01, 0x06, 0x04, 0xff]), vec![0x01, 0x00, 0x00, 0x05, 0, 0, 0, 5, 0x81, 0x01, 0x06, 0x04, 0xff]);
    }

    #[test]
    fn talks_visca_over_ip() {
        // A stand-in camera on this computer.
        let cam = UdpSocket::bind("127.0.0.1:0").unwrap();
        cam.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let cfg = SourceConfig {
            name: "PTZ".into(),
            ptz: PtzConfig { control: PtzControl::ViscaIp, address: cam.local_addr().unwrap().to_string(), speed: 1.0, ..Default::default() },
            ..Default::default()
        };
        let p = PtzCamera::for_source(&cfg).unwrap();
        p.send(Move::Recall(3));
        let mut buf = [0u8; 64];
        // First the sequence reset, then the command with sequence 1.
        let n = cam.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], &[0x02, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0x01]);
        let n = cam.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], &[0x01, 0x00, 0x00, 0x07, 0, 0, 0, 1, 0x81, 0x01, 0x04, 0x3f, 0x02, 0x03, 0xff]);
        p.send(Move::Home);
        let n = cam.recv(&mut buf).unwrap();
        assert_eq!(&buf[4..8], &[0, 0, 0, 2], "sequence numbers count up");
        assert_eq!(&buf[8..n], &[0x81, 0x01, 0x06, 0x04, 0xff]);
        assert!(p.status().contains("connected"), "{}", p.status());
        // Upside-down mounting flips the directions (and speed scales).
        let cfg = SourceConfig { ptz: PtzConfig { control: PtzControl::ViscaUdp, address: cam.local_addr().unwrap().to_string(), speed: 0.5, invert_pan: true, ..Default::default() }, ..Default::default() };
        let p = PtzCamera::for_source(&cfg).unwrap();
        p.send(Move::Drive { pan: 1.0, tilt: 0.0 });
        let n = cam.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], &[0x81, 0x01, 0x06, 0x01, 0x0c, 0x01, 0x01, 0x03, 0xff], "plain VISCA, half speed, to the left");
    }

    #[test]
    fn talks_visca_over_tcp() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let cfg = SourceConfig { ptz: PtzConfig { control: PtzControl::ViscaTcp, address: listener.local_addr().unwrap().to_string(), ..Default::default() }, ..Default::default() };
        let p = PtzCamera::for_source(&cfg).unwrap();
        p.send(Move::Zoom(0.0));
        let (mut conn, _) = listener.accept().unwrap();
        conn.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut buf = [0u8; 6];
        std::io::Read::read_exact(&mut conn, &mut buf).unwrap();
        assert_eq!(buf, [0x81, 0x01, 0x04, 0x07, 0x00, 0xff]);
    }

    #[test]
    fn virtual_ptz_moves_and_glides_to_presets() {
        let cfg = PtzConfig { max_zoom: 3.0, glide_seconds: 1.0, presets: vec![crate::config::PtzPreset { name: "Left".into(), x: 0.25, y: 0.4, zoom: 2.0 }], ..Default::default() };
        let v = VirtualPtz::new(&cfg);
        // It starts on preset 1; then from the whole picture.
        assert_eq!(v.step(0.1), Shot { x: 0.25, y: 0.4, zoom: 2.0 });
        v.st.lock_ok().pos = Shot::default();
        assert_eq!(v.step(0.1), Shot::default());
        // Zooming in, then panning right.
        v.apply(Move::Zoom(1.0));
        for _ in 0..30 {
            v.step(0.1);
        }
        let z = v.position().zoom;
        assert!(z > 2.5 && z <= 3.0, "zoom {z}");
        v.apply(Move::Drive { pan: 1.0, tilt: 0.0 });
        let x0 = v.position().x;
        v.step(0.5);
        assert!(v.position().x > x0, "moved right");
        v.apply(Move::Stop);
        let still = v.position();
        v.step(0.5);
        assert_eq!(v.position(), still, "stops");
        // A preset: glides there and settles.
        v.apply(Move::Recall(0));
        let half = v.step(0.2);
        assert!(half.x < still.x && half.x > 0.25, "on the way: {half:?}");
        for _ in 0..40 {
            v.step(0.1);
        }
        let p = v.position();
        assert!((p.x - 0.25).abs() < 1e-3 && (p.y - 0.4).abs() < 1e-3 && (p.zoom - 2.0).abs() < 1e-2, "{p:?}");
        // Store where it is as preset 2, go home, come back.
        v.apply(Move::Store(1));
        v.apply(Move::Home);
        for _ in 0..40 {
            v.step(0.1);
        }
        assert_eq!(v.position(), Shot::default());
        v.apply(Move::Recall(1));
        for _ in 0..40 {
            v.step(0.1);
        }
        assert!((v.position().x - 0.25).abs() < 1e-3);
        // Never past the picture's edge.
        v.apply(Move::Drive { pan: -1.0, tilt: 1.0 });
        for _ in 0..100 {
            v.step(0.1);
        }
        let p = v.position();
        assert!((p.x - 0.5 / p.zoom).abs() < 1e-4 && (p.y - 0.5 / p.zoom).abs() < 1e-4, "{p:?}");
    }

    #[test]
    fn shot_rectangles() {
        // The whole of a 16:9 picture for a 16:9 output.
        assert_eq!(shot_rect(Shot::default(), (3840, 2160), (1920, 1080)), [0.0, 0.0, 3840.0, 2160.0]);
        // Twice as close, on the left third.
        let r = shot_rect(Shot { x: 0.25, y: 0.5, zoom: 2.0 }, (3840, 2160), (1920, 1080));
        assert_eq!(r, [0.0, 540.0, 1920.0, 1080.0]);
        // A 9:16 output from a 16:9 picture: a tall slice.
        let r = shot_rect(Shot::default(), (1920, 1080), (1080, 1920));
        assert!((r[2] - 607.5).abs() < 0.1 && r[3] == 1080.0);
    }
}
