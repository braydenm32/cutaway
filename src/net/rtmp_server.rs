//! A small RTMP server that accepts one publisher (OBS, a phone app, an
//! encoder, another Cutaway) and hands over its audio and video.

use crate::util::LockExt;
use crate::media::flv::Amf;
use crate::media::{h264, Ns, VideoCodec};
use anyhow::{anyhow, bail, Context, Result};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// What arrives from a publisher.
#[derive(Debug, Clone)]
pub enum Incoming {
    /// The publisher started (stream key).
    Started(String),
    VideoConfig { codec: VideoCodec, parameter_sets: Vec<Vec<u8>> },
    /// AVCC NAL units.
    Video { pts: Ns, dts: Ns, keyframe: bool, data: Vec<u8> },
    AudioConfig { asc: Vec<u8> },
    /// One raw AAC frame.
    Audio { pts: Ns, data: Vec<u8> },
    Ended(String),
}

const MSG_SET_CHUNK: u8 = 1;
const MSG_ACK: u8 = 3;
const MSG_USER: u8 = 4;
const MSG_WINDOW: u8 = 5;
const MSG_PEER_BW: u8 = 6;
const MSG_AUDIO: u8 = 8;
const MSG_VIDEO: u8 = 9;
const MSG_COMMAND_AMF3: u8 = 17;
const MSG_COMMAND: u8 = 20;

#[derive(Default, Clone)]
struct InHeader {
    timestamp: u32,
    /// Last timestamp delta (a type-3 chunk starting a message repeats it).
    delta: u32,
    length: usize,
    kind: u8,
    stream: u32,
    payload: Vec<u8>,
}

struct Message {
    kind: u8,
    timestamp: u32,
    payload: Vec<u8>,
}

struct Session {
    conn: TcpStream,
    in_chunk: usize,
    in_buf: Vec<u8>,
    in_headers: std::collections::HashMap<u32, InHeader>,
    received: u64,
    acked: u64,
    window: u64,
    /// Unwrapping of 32-bit millisecond timestamps.
    last_ts: Option<u32>,
    ts_high: i64,
    video_codec: VideoCodec,
}

impl Session {
    fn send(&mut self, cs: u8, kind: u8, stream: u32, payload: &[u8]) -> Result<()> {
        let mut out = vec![cs & 0x3f, 0, 0, 0];
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
        out.push(kind);
        out.extend_from_slice(&stream.to_le_bytes());
        for (i, c) in payload.chunks(4096).enumerate() {
            if i > 0 {
                out.push(0xC0 | (cs & 0x3f));
            }
            out.extend_from_slice(c);
        }
        self.conn.write_all(&out)?;
        Ok(())
    }

    fn command(&mut self, stream: u32, values: &[Amf]) -> Result<()> {
        let mut body = Vec::new();
        for v in values {
            v.encode(&mut body);
        }
        self.send(3, MSG_COMMAND, stream, &body)
    }

    fn handshake(&mut self) -> Result<()> {
        let mut c0c1 = vec![0u8; 1537];
        self.conn.read_exact(&mut c0c1).context("no RTMP handshake")?;
        if c0c1[0] != 3 {
            bail!("not an RTMP client (version {})", c0c1[0]);
        }
        let mut s = vec![3u8];
        let mut s1 = vec![0u8; 1536];
        let mut x: u32 = 0x9e37_79b9;
        for b in s1.iter_mut().skip(8) {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            *b = x as u8;
        }
        s.extend_from_slice(&s1);
        s.extend_from_slice(&c0c1[1..]); // S2 echoes C1
        self.conn.write_all(&s)?;
        let mut c2 = vec![0u8; 1536];
        self.conn.read_exact(&mut c2)?;
        Ok(())
    }

    fn read_message(&mut self, stop: &AtomicBool) -> Result<Option<Message>> {
        loop {
            if let Some(m) = self.parse_one()? {
                return Ok(Some(m));
            }
            if stop.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let mut buf = [0u8; 65536];
            match self.conn.read(&mut buf) {
                Ok(0) => bail!("the publisher disconnected"),
                Ok(n) => {
                    self.in_buf.extend_from_slice(&buf[..n]);
                    self.received += n as u64;
                    if self.received - self.acked > self.window / 2 {
                        self.acked = self.received;
                        let seq = (self.received as u32).to_be_bytes();
                        self.send(2, MSG_ACK, 0, &seq)?;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {}
                Err(e) => bail!("connection lost: {e}"),
            }
        }
    }

    fn parse_one(&mut self) -> Result<Option<Message>> {
        loop {
            let b = &self.in_buf;
            if b.is_empty() {
                return Ok(None);
            }
            let fmt = b[0] >> 6;
            let (cs, mut i) = match b[0] & 0x3F {
                0 => {
                    if b.len() < 2 {
                        return Ok(None);
                    }
                    (64 + b[1] as u32, 2)
                }
                1 => {
                    if b.len() < 3 {
                        return Ok(None);
                    }
                    (64 + b[1] as u32 + 256 * b[2] as u32, 3)
                }
                n => (n as u32, 1),
            };
            let hlen = [11, 7, 3, 0][fmt as usize];
            if b.len() < i + hlen {
                return Ok(None);
            }
            let mut h = self.in_headers.get(&cs).cloned().unwrap_or_default();
            let fresh = h.payload.is_empty();
            let mut ts_field = None;
            if fmt <= 2 {
                ts_field = Some(u32::from_be_bytes([0, b[i], b[i + 1], b[i + 2]]));
            }
            if fmt <= 1 {
                h.length = u32::from_be_bytes([0, b[i + 3], b[i + 4], b[i + 5]]) as usize;
                h.kind = b[i + 6];
            }
            if fmt == 0 {
                h.stream = u32::from_le_bytes([b[i + 7], b[i + 8], b[i + 9], b[i + 10]]);
            }
            i += hlen;
            let mut ts_val = ts_field;
            if ts_field == Some(0xFF_FFFF) {
                if b.len() < i + 4 {
                    return Ok(None);
                }
                ts_val = Some(u32::from_be_bytes(b[i..i + 4].try_into().unwrap()));
                i += 4;
            }
            let remaining = h.length.saturating_sub(h.payload.len());
            let take = remaining.min(self.in_chunk);
            if b.len() < i + take {
                return Ok(None);
            }
            if fresh {
                match (fmt, ts_val) {
                    (0, Some(t)) => {
                        h.delta = t.wrapping_sub(h.timestamp);
                        h.timestamp = t;
                    }
                    (_, Some(t)) => {
                        h.delta = t;
                        h.timestamp = h.timestamp.wrapping_add(t);
                    }
                    // Type 3 starting a new message: same delta again.
                    (_, None) => h.timestamp = h.timestamp.wrapping_add(h.delta),
                }
            }
            h.payload.extend_from_slice(&b[i..i + take]);
            self.in_buf.drain(..i + take);
            if h.payload.len() >= h.length {
                let msg = Message { kind: h.kind, timestamp: h.timestamp, payload: std::mem::take(&mut h.payload) };
                self.in_headers.insert(cs, h);
                return Ok(Some(msg));
            }
            self.in_headers.insert(cs, h);
        }
    }

    fn ms(&mut self, ts: u32) -> i64 {
        if let Some(last) = self.last_ts {
            if ts < last && last - ts > 0x8000_0000 {
                self.ts_high += 1 << 32;
            }
        }
        self.last_ts = Some(ts);
        self.ts_high + ts as i64
    }
}

/// Listens on `port` and passes everything from each publisher to `out`
/// until `stop`. `key` (if not empty) must match the stream key.
pub fn serve(port: u16, key: &str, stop: &AtomicBool, out: &mut dyn FnMut(Incoming)) -> Result<()> {
    let listener = TcpListener::bind(("0.0.0.0", port)).with_context(|| format!("can't listen on port {port} (is something else using it?)"))?;
    listener.set_nonblocking(true)?;
    while !stop.load(Ordering::Relaxed) {
        let conn = match listener.accept() {
            Ok((c, _)) => c,
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(anyhow!("accepting a connection: {e}")),
        };
        conn.set_nonblocking(false)?;
        conn.set_read_timeout(Some(Duration::from_millis(200)))?;
        conn.set_nodelay(true).ok();
        let mut s = Session {
            conn,
            in_chunk: 128,
            in_buf: Vec::new(),
            in_headers: Default::default(),
            received: 0,
            acked: 0,
            window: 2_500_000,
            last_ts: None,
            ts_high: 0,
            video_codec: VideoCodec::H264,
        };
        let r = (|| -> Result<()> {
            s.conn.set_read_timeout(Some(Duration::from_secs(5)))?;
            s.handshake()?;
            s.conn.set_read_timeout(Some(Duration::from_millis(200)))?;
            let mut publishing: Option<String> = None;
            let result = loop {
                let Some(m) = s.read_message(stop)? else { break Ok(()) };
                match m.kind {
                    MSG_SET_CHUNK if m.payload.len() >= 4 => s.in_chunk = (u32::from_be_bytes(m.payload[..4].try_into().unwrap()) & 0x7FFF_FFFF).max(1) as usize,
                    MSG_WINDOW if m.payload.len() >= 4 => s.window = u32::from_be_bytes(m.payload[..4].try_into().unwrap()) as u64,
                    MSG_ACK | MSG_USER | MSG_PEER_BW => {}
                    MSG_COMMAND | MSG_COMMAND_AMF3 => {
                        let body = if m.kind == MSG_COMMAND_AMF3 { &m.payload[1.min(m.payload.len())..] } else { &m.payload[..] };
                        let v = Amf::decode_all(body);
                        let name = v.first().and_then(|x| x.as_str()).unwrap_or("").to_string();
                        let txn = v.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0);
                        match name.as_str() {
                            "connect" => {
                                s.send(2, MSG_WINDOW, 0, &2_500_000u32.to_be_bytes())?;
                                s.send(2, MSG_PEER_BW, 0, &[0x00, 0x26, 0x25, 0xA0, 2])?;
                                s.send(2, MSG_SET_CHUNK, 0, &4096u32.to_be_bytes())?;
                                s.command(
                                    0,
                                    &[
                                        Amf::str("_result"),
                                        Amf::Number(txn),
                                        Amf::Object(vec![("fmsVer".into(), Amf::str("FMS/3,0,1,123")), ("capabilities".into(), Amf::Number(31.0))]),
                                        Amf::Object(vec![
                                            ("level".into(), Amf::str("status")),
                                            ("code".into(), Amf::str("NetConnection.Connect.Success")),
                                            ("description".into(), Amf::str("Connection succeeded.")),
                                            ("objectEncoding".into(), Amf::Number(0.0)),
                                        ]),
                                    ],
                                )?;
                            }
                            "createStream" => s.command(0, &[Amf::str("_result"), Amf::Number(txn), Amf::Null, Amf::Number(1.0)])?,
                            "releaseStream" | "FCPublish" => s.command(0, &[Amf::str("_result"), Amf::Number(txn), Amf::Null, Amf::Null])?,
                            "publish" => {
                                let stream = v.get(3).and_then(|x| x.as_str()).unwrap_or("").to_string();
                                if !key.is_empty() && stream != key {
                                    s.command(
                                        1,
                                        &[
                                            Amf::str("onStatus"),
                                            Amf::Number(0.0),
                                            Amf::Null,
                                            Amf::Object(vec![("level".into(), Amf::str("error")), ("code".into(), Amf::str("NetStream.Publish.BadName")), ("description".into(), Amf::str("Wrong stream key."))]),
                                        ],
                                    )?;
                                    bail!("a publisher used the wrong stream key");
                                }
                                s.command(
                                    1,
                                    &[
                                        Amf::str("onStatus"),
                                        Amf::Number(0.0),
                                        Amf::Null,
                                        Amf::Object(vec![
                                            ("level".into(), Amf::str("status")),
                                            ("code".into(), Amf::str("NetStream.Publish.Start")),
                                            ("description".into(), Amf::str("Publishing.")),
                                        ]),
                                    ],
                                )?;
                                out(Incoming::Started(stream.clone()));
                                publishing = Some(stream);
                            }
                            "FCUnpublish" | "deleteStream" | "closeStream" => break Ok(()),
                            _ => {}
                        }
                    }
                    MSG_VIDEO => {
                        let t = s.ms(m.timestamp);
                        handle_video(&mut s, t, &m.payload, out);
                    }
                    MSG_AUDIO => {
                        let t = s.ms(m.timestamp);
                        handle_audio(t, &m.payload, out);
                    }
                    _ => {}
                }
            };
            if let Some(k) = publishing {
                out(Incoming::Ended(k));
            }
            result
        })();
        if let Err(e) = r {
            if !stop.load(Ordering::Relaxed) {
                out(Incoming::Ended(format!("{e:#}")));
            }
        }
    }
    Ok(())
}

fn si24(b: &[u8]) -> i64 {
    let v = ((b[0] as i32) << 16) | ((b[1] as i32) << 8) | b[2] as i32;
    (if v & 0x80_0000 != 0 { v - 0x100_0000 } else { v }) as i64
}

fn handle_video(s: &mut Session, t_ms: i64, b: &[u8], out: &mut dyn FnMut(Incoming)) {
    if b.len() < 5 {
        return;
    }
    let ms = 1_000_000i64;
    if b[0] & 0x80 != 0 {
        // Enhanced RTMP: FourCC codecs (HEVC from OBS and others).
        let frame_type = (b[0] >> 4) & 7;
        let packet = b[0] & 0x0f;
        let codec = match &b[1..5] {
            b"hvc1" => VideoCodec::Hevc,
            b"avc1" => VideoCodec::H264,
            _ => return,
        };
        s.video_codec = codec;
        match packet {
            0 => {
                let rec = &b[5..];
                let ps = if codec == VideoCodec::Hevc { h264::parse_hvcc(rec) } else { h264::parse_avcc(rec) };
                out(Incoming::VideoConfig { codec, parameter_sets: ps });
            }
            1 if b.len() >= 8 => {
                let cts = si24(&b[5..8]);
                out(Incoming::Video { pts: (t_ms + cts) * ms, dts: t_ms * ms, keyframe: frame_type == 1, data: b[8..].to_vec() });
            }
            3 => out(Incoming::Video { pts: t_ms * ms, dts: t_ms * ms, keyframe: frame_type == 1, data: b[5..].to_vec() }),
            _ => {}
        }
        return;
    }
    if b[0] & 0x0f != 7 {
        return; // only AVC in classic FLV
    }
    let keyframe = (b[0] >> 4) == 1;
    match b[1] {
        0 => out(Incoming::VideoConfig { codec: VideoCodec::H264, parameter_sets: h264::parse_avcc(&b[5..]) }),
        1 => {
            let cts = si24(&b[2..5]);
            out(Incoming::Video { pts: (t_ms + cts) * ms, dts: t_ms * ms, keyframe, data: b[5..].to_vec() });
        }
        _ => {}
    }
}

fn handle_audio(t_ms: i64, b: &[u8], out: &mut dyn FnMut(Incoming)) {
    if b.len() < 2 || b[0] >> 4 != 10 {
        return; // AAC only
    }
    match b[1] {
        0 => out(Incoming::AudioConfig { asc: b[2..].to_vec() }),
        1 => out(Incoming::Audio { pts: t_ms * 1_000_000, data: b[2..].to_vec() }),
        _ => {}
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn receives_from_ffmpeg() {
        if std::process::Command::new("ffmpeg").arg("-version").output().is_err() {
            return;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let got: Arc<Mutex<Vec<Incoming>>> = Arc::new(Mutex::new(Vec::new()));
        let (s2, g2) = (stop.clone(), got.clone());
        let server = std::thread::spawn(move || serve(19371, "secret", &s2, &mut |i| g2.lock_ok().push(i)));
        std::thread::sleep(Duration::from_millis(300));
        let status = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-re", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=2", "-f", "lavfi", "-i", "sine=duration=2"])
            .args(["-c:v", "libx264", "-g", "25", "-c:a", "aac", "-f", "flv", "rtmp://127.0.0.1:19371/live/secret"])
            .status()
            .unwrap();
        assert!(status.success());
        std::thread::sleep(Duration::from_millis(300));
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap().unwrap();
        let got = got.lock_ok();
        assert!(matches!(got.first(), Some(Incoming::Started(k)) if k == "secret"), "{:?}", got.first());
        let videos: Vec<&Incoming> = got.iter().filter(|i| matches!(i, Incoming::Video { .. })).collect();
        assert_eq!(videos.len(), 50);
        assert!(got.iter().any(|i| matches!(i, Incoming::VideoConfig { parameter_sets, .. } if parameter_sets.len() == 2)));
        assert!(got.iter().any(|i| matches!(i, Incoming::AudioConfig { .. })));
        assert!(got.iter().filter(|i| matches!(i, Incoming::Audio { .. })).count() > 80);
        assert!(matches!(got.last(), Some(Incoming::Ended(_))));
    }
}
