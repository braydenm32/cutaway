//! A small RTMP/RTMPS publishing client.
//!
//! It speaks the same command sequence as OBS and FFmpeg (connect,
//! releaseStream, FCPublish, createStream, publish) so it works with
//! YouTube, Twitch, Facebook, Kick and ordinary media servers.

use crate::media::flv::Amf;
use anyhow::{anyhow, bail, Context, Result};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct RtmpUrl {
    pub tls: bool,
    pub host: String,
    pub port: u16,
    pub app: String,
    pub stream: String,
    pub tc_url: String,
}

/// rtmp[s]://host[:port]/app[/more]/stream-key
pub fn parse_url(url: &str) -> Result<RtmpUrl> {
    let url = url.trim();
    let (tls, rest) = if let Some(r) = url.strip_prefix("rtmps://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("rtmp://") {
        (false, r)
    } else {
        bail!("the address must start with rtmp:// or rtmps://");
    };
    let (hostport, path) = rest.split_once('/').ok_or_else(|| anyhow!("the address has no application path"))?;
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|_| anyhow!("bad port \"{p}\""))?),
        None => (hostport.to_string(), if tls { 443 } else { 1935 }),
    };
    let path = path.trim_end_matches('/');
    let (app, stream) = path.rsplit_once('/').ok_or_else(|| anyhow!("the address needs an application and a stream key"))?;
    if host.is_empty() || app.is_empty() || stream.is_empty() {
        bail!("incomplete RTMP address");
    }
    let default_port = if tls { 443 } else { 1935 };
    let tc_host = if port == default_port { host.clone() } else { format!("{host}:{port}") };
    let tc_url = format!("{}://{}/{}", if tls { "rtmps" } else { "rtmp" }, tc_host, app);
    Ok(RtmpUrl { tls, host, port, app: app.to_string(), stream: stream.to_string(), tc_url })
}

enum Conn {
    Plain(TcpStream),
    Tls(Box<native_tls::TlsStream<TcpStream>>),
}

impl Conn {
    fn tcp(&self) -> &TcpStream {
        match self {
            Conn::Plain(s) => s,
            Conn::Tls(s) => s.get_ref(),
        }
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
        }
    }
}

const CS_CONTROL: u8 = 2;
const CS_COMMAND: u8 = 3;
const CS_AUDIO: u8 = 4;
const CS_DATA: u8 = 5;
const CS_VIDEO: u8 = 6;

const MSG_SET_CHUNK: u8 = 1;
const MSG_ACK: u8 = 3;
const MSG_USER: u8 = 4;
const MSG_WINDOW: u8 = 5;
const MSG_PEER_BW: u8 = 6;
const MSG_AUDIO: u8 = 8;
const MSG_VIDEO: u8 = 9;
const MSG_DATA: u8 = 18;
const MSG_COMMAND: u8 = 20;

const OUT_CHUNK: usize = 4096;

#[derive(Default, Clone)]
struct InHeader {
    timestamp: u32,
    length: usize,
    kind: u8,
    stream: u32,
    payload: Vec<u8>,
}

pub struct RtmpPublisher {
    conn: Conn,
    url: RtmpUrl,
    stream_id: u32,
    in_chunk: usize,
    in_buf: Vec<u8>,
    in_headers: std::collections::HashMap<u32, InHeader>,
    window: u32,
    received: u64,
    acked: u64,
    sent: u64,
    txn: f64,
}

struct Message {
    kind: u8,
    payload: Vec<u8>,
}

impl RtmpPublisher {
    /// Connect, handshake and start publishing. `timeout` covers the whole
    /// sequence.
    /// Check that the server answers and accepts the application (without
    /// publishing anything).
    pub fn probe(url: &str, timeout: Duration) -> Result<()> {
        let mut p = Self::open(url, timeout)?;
        let _ = p.conn.flush();
        Ok(())
    }

    pub fn connect(url: &str, timeout: Duration) -> Result<RtmpPublisher> {
        let mut p = Self::open(url, timeout)?;
        let deadline = Instant::now() + timeout;
        let key = p.url.stream.clone();
        p.command("releaseStream", &[Amf::Null, Amf::Str(key.clone())], 0)?;
        p.command("FCPublish", &[Amf::Null, Amf::Str(key.clone())], 0)?;
        let create = p.command("createStream", &[Amf::Null], 0)?;
        let reply = p.wait_result(create, deadline).context("createStream failed")?;
        p.stream_id = reply.get(3).and_then(|v| v.as_f64()).unwrap_or(1.0) as u32;
        let sid = p.stream_id;
        p.command("publish", &[Amf::Null, Amf::Str(key), Amf::str("live")], sid)?;
        // Wait for NetStream.Publish.Start.
        loop {
            if Instant::now() > deadline {
                bail!("timed out waiting for the server to accept the stream");
            }
            let Some(m) = p.read_message(deadline)? else { continue };
            if m.kind == MSG_COMMAND {
                let v = Amf::decode_all(&m.payload);
                let name = v.first().and_then(|x| x.as_str()).unwrap_or("");
                if name == "onStatus" {
                    let info = v.get(3);
                    let code = info.and_then(|i| i.get("code")).and_then(|c| c.as_str()).unwrap_or("").to_string();
                    if code == "NetStream.Publish.Start" {
                        break;
                    }
                    if code.contains("Error") || code.contains("BadName") || code.contains("Failed") || code.contains("Rejected") {
                        let desc = info.and_then(|i| i.get("description")).and_then(|c| c.as_str()).unwrap_or("");
                        bail!("the server rejected the stream ({code}{}{desc})", if desc.is_empty() { "" } else { ": " });
                    }
                } else if name == "_error" {
                    bail!("the server rejected the stream key");
                }
            }
        }
        p.conn.tcp().set_read_timeout(Some(Duration::from_millis(1)))?;
        Ok(p)
    }

    /// Connect, handshake and `connect` to the application.
    fn open(url: &str, timeout: Duration) -> Result<RtmpPublisher> {
        let u = parse_url(url)?;
        let deadline = Instant::now() + timeout;
        let addr = (u.host.as_str(), u.port)
            .to_socket_addrs()
            .with_context(|| format!("can't find {}", u.host))?
            .next()
            .ok_or_else(|| anyhow!("can't find {}", u.host))?;
        let tcp = TcpStream::connect_timeout(&addr, timeout).with_context(|| format!("can't connect to {}:{}", u.host, u.port))?;
        tcp.set_nodelay(true).ok();
        tcp.set_read_timeout(Some(timeout))?;
        tcp.set_write_timeout(Some(Duration::from_secs(10)))?;
        let conn = if u.tls {
            let connector = native_tls::TlsConnector::new().context("TLS setup")?;
            Conn::Tls(Box::new(connector.connect(&u.host, tcp).map_err(|e| anyhow!("secure connection failed: {e}"))?))
        } else {
            Conn::Plain(tcp)
        };
        let mut p = RtmpPublisher {
            conn,
            url: u,
            stream_id: 0,
            in_chunk: 128,
            in_buf: Vec::new(),
            in_headers: Default::default(),
            window: 2_500_000,
            received: 0,
            acked: 0,
            sent: 0,
            txn: 1.0,
        };
        p.handshake()?;
        p.send_message(CS_CONTROL, MSG_SET_CHUNK, 0, 0, &(OUT_CHUNK as u32).to_be_bytes())?;

        let tc = p.url.tc_url.clone();
        let app = p.url.app.clone();
        let connect = p.command(
            "connect",
            &[Amf::Object(vec![
                ("app".into(), Amf::Str(app)),
                ("type".into(), Amf::str("nonprivate")),
                ("flashVer".into(), Amf::str("FMLE/3.0 (compatible; FMSc/1.0)")),
                ("swfUrl".into(), Amf::Str(tc.clone())),
                ("tcUrl".into(), Amf::Str(tc)),
            ])],
            0,
        )?;
        p.wait_result(connect, deadline).context("the server refused the connection")?;
        Ok(p)
    }

    fn handshake(&mut self) -> Result<()> {
        let mut c0c1 = vec![3u8];
        c0c1.extend_from_slice(&[0; 8]);
        let mut x: u32 = 0x1234_5678 ^ (std::process::id().wrapping_mul(2654435761));
        for _ in 0..1528 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            c0c1.push(x as u8);
        }
        self.conn.write_all(&c0c1)?;
        let mut s = vec![0u8; 1 + 1536 * 2];
        self.conn.read_exact(&mut s).context("no handshake reply (is this an RTMP server?)")?;
        if s[0] != 3 {
            bail!("unsupported RTMP version {}", s[0]);
        }
        self.conn.write_all(&s[1..1537])?; // C2 echoes S1
        Ok(())
    }

    fn command(&mut self, name: &str, args: &[Amf], stream: u32) -> Result<f64> {
        let id = if name == "publish" || name == "FCPublish" || name == "releaseStream" || name == "connect" || name == "createStream" {
            self.txn += 1.0;
            self.txn
        } else {
            0.0
        };
        let mut body = Vec::new();
        Amf::str(name).encode(&mut body);
        Amf::Number(id).encode(&mut body);
        for a in args {
            a.encode(&mut body);
        }
        self.send_message(CS_COMMAND, MSG_COMMAND, 0, stream, &body)?;
        Ok(id)
    }

    fn wait_result(&mut self, txn: f64, deadline: Instant) -> Result<Vec<Amf>> {
        loop {
            if Instant::now() > deadline {
                bail!("timed out waiting for the server");
            }
            let Some(m) = self.read_message(deadline)? else { continue };
            if m.kind != MSG_COMMAND {
                continue;
            }
            let v = Amf::decode_all(&m.payload);
            let name = v.first().and_then(|x| x.as_str()).unwrap_or("");
            let id = v.get(1).and_then(|x| x.as_f64()).unwrap_or(-1.0);
            if (id - txn).abs() < 0.5 {
                if name == "_result" {
                    return Ok(v);
                }
                if name == "_error" {
                    let desc = v.get(3).and_then(|i| i.get("description")).and_then(|d| d.as_str()).unwrap_or("error");
                    bail!("{desc}");
                }
            }
        }
    }

    fn send_message(&mut self, cs: u8, kind: u8, timestamp: u32, stream: u32, payload: &[u8]) -> Result<()> {
        let mut out = Vec::with_capacity(payload.len() + 16 + payload.len() / OUT_CHUNK);
        let ext = timestamp >= 0xFF_FFFF;
        out.push(cs & 0x3F);
        let ts = if ext { 0xFF_FFFF } else { timestamp };
        out.extend_from_slice(&ts.to_be_bytes()[1..]);
        out.extend_from_slice(&(payload.len() as u32).to_be_bytes()[1..]);
        out.push(kind);
        out.extend_from_slice(&stream.to_le_bytes());
        if ext {
            out.extend_from_slice(&timestamp.to_be_bytes());
        }
        for (i, chunk) in payload.chunks(OUT_CHUNK).enumerate() {
            if i > 0 {
                out.push(0xC0 | (cs & 0x3F));
                if ext {
                    out.extend_from_slice(&timestamp.to_be_bytes());
                }
            }
            out.extend_from_slice(chunk);
        }
        self.conn.write_all(&out).map_err(|e| match e.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => anyhow!("the connection is too slow (send timed out)"),
            _ => anyhow!("connection lost: {e}"),
        })?;
        self.sent += out.len() as u64;
        Ok(())
    }

    /// Read whatever is available and return one complete message, if any.
    fn read_message(&mut self, deadline: Instant) -> Result<Option<Message>> {
        loop {
            if let Some(m) = self.parse_one()? {
                if let Some(m) = self.handle_control(m)? {
                    return Ok(Some(m));
                }
                continue;
            }
            let mut buf = [0u8; 16384];
            match self.conn.read(&mut buf) {
                Ok(0) => bail!("the server closed the connection"),
                Ok(n) => {
                    self.in_buf.extend_from_slice(&buf[..n]);
                    self.received += n as u64;
                    if self.received - self.acked > self.window as u64 / 2 {
                        self.acked = self.received;
                        let seq = (self.received as u32).to_be_bytes();
                        self.send_message(CS_CONTROL, MSG_ACK, 0, 0, &seq)?;
                    }
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                }
                Err(e) => bail!("connection lost: {e}"),
            }
        }
    }

    /// Parse buffered chunks until one message is complete (or more data
    /// is needed).
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
            if ts_field == Some(0xFF_FFFF) {
                if b.len() < i + 4 {
                    return Ok(None);
                }
                i += 4;
            }
            let remaining = h.length.saturating_sub(h.payload.len());
            let take = remaining.min(self.in_chunk);
            if b.len() < i + take {
                return Ok(None);
            }
            if let Some(t) = ts_field {
                if fresh {
                    h.timestamp = if fmt == 0 { t } else { h.timestamp.wrapping_add(t) };
                }
            }
            h.payload.extend_from_slice(&b[i..i + take]);
            self.in_buf.drain(..i + take);
            if h.payload.len() >= h.length {
                let msg = Message { kind: h.kind, payload: std::mem::take(&mut h.payload) };
                self.in_headers.insert(cs, h);
                return Ok(Some(msg));
            }
            self.in_headers.insert(cs, h);
        }
    }

    /// Handle protocol control messages; pass others on.
    fn handle_control(&mut self, m: Message) -> Result<Option<Message>> {
        match m.kind {
            MSG_SET_CHUNK if m.payload.len() >= 4 => {
                self.in_chunk = (u32::from_be_bytes(m.payload[..4].try_into().unwrap()) & 0x7FFF_FFFF).max(1) as usize;
                Ok(None)
            }
            MSG_WINDOW if m.payload.len() >= 4 => {
                self.window = u32::from_be_bytes(m.payload[..4].try_into().unwrap());
                Ok(None)
            }
            MSG_PEER_BW if m.payload.len() >= 4 => {
                let size = m.payload[..4].to_vec();
                self.send_message(CS_CONTROL, MSG_WINDOW, 0, 0, &size)?;
                Ok(None)
            }
            MSG_USER if m.payload.len() >= 6 => {
                if u16::from_be_bytes([m.payload[0], m.payload[1]]) == 6 {
                    // Ping request: answer with a ping response.
                    let mut r = vec![0, 7];
                    r.extend_from_slice(&m.payload[2..6]);
                    self.send_message(CS_CONTROL, MSG_USER, 0, 0, &r)?;
                }
                Ok(None)
            }
            MSG_ACK => Ok(None),
            _ => Ok(Some(m)),
        }
    }

    /// Answer pings, notice errors and disconnects. Call regularly.
    pub fn poll(&mut self) -> Result<()> {
        while let Some(m) = self.read_message(Instant::now())? {
            if m.kind == MSG_COMMAND {
                let v = Amf::decode_all(&m.payload);
                if v.first().and_then(|x| x.as_str()) == Some("onStatus") {
                    let code = v.get(3).and_then(|i| i.get("code")).and_then(|c| c.as_str()).unwrap_or("");
                    if code.contains("Error") || code.contains("Failed") || code.contains("Unpublish") {
                        bail!("the server ended the stream ({code})");
                    }
                }
            }
        }
        Ok(())
    }

    pub fn send_metadata(&mut self, body: &[u8]) -> Result<()> {
        let sid = self.stream_id;
        self.send_message(CS_DATA, MSG_DATA, 0, sid, body)
    }

    pub fn send_video(&mut self, ts_ms: u32, body: &[u8]) -> Result<()> {
        let sid = self.stream_id;
        self.send_message(CS_VIDEO, MSG_VIDEO, ts_ms, sid, body)
    }

    pub fn send_audio(&mut self, ts_ms: u32, body: &[u8]) -> Result<()> {
        let sid = self.stream_id;
        self.send_message(CS_AUDIO, MSG_AUDIO, ts_ms, sid, body)
    }

    pub fn bytes_sent(&self) -> u64 {
        self.sent
    }

    /// End the broadcast politely.
    pub fn close(mut self) {
        let key = self.url.stream.clone();
        let sid = self.stream_id;
        let _ = self.command("FCUnpublish", &[Amf::Null, Amf::Str(key)], 0);
        let _ = self.command("deleteStream", &[Amf::Null, Amf::Number(sid as f64)], 0);
        let _ = self.conn.flush();
        if let Conn::Tls(mut s) = self.conn {
            let _ = s.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let u = parse_url("rtmp://a.rtmp.youtube.com/live2/abcd-efgh").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.app.as_str(), u.stream.as_str()), ("a.rtmp.youtube.com", 1935, "live2", "abcd-efgh"));
        assert_eq!(u.tc_url, "rtmp://a.rtmp.youtube.com/live2");
        let u = parse_url("rtmps://live-api-s.facebook.com:443/rtmp/FB-123?s_bl=1").unwrap();
        assert!(u.tls);
        assert_eq!((u.port, u.app.as_str(), u.stream.as_str()), (443, "rtmp", "FB-123?s_bl=1"));
        assert_eq!(u.tc_url, "rtmps://live-api-s.facebook.com/rtmp");
        let u = parse_url("rtmp://10.0.0.2:1936/app/inst/show").unwrap();
        assert_eq!((u.app.as_str(), u.stream.as_str()), ("app/inst", "show"));
        assert_eq!(u.tc_url, "rtmp://10.0.0.2:1936/app/inst");
        assert!(parse_url("rtmp://host/onlyapp").is_err());
        assert!(parse_url("http://x/a/b").is_err());
    }
}
