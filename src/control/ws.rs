//! WebSocket (RFC 6455), just what the remote API needs: the opening
//! handshake and text messages both ways.

use std::io::{self, Read, Write};

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// Biggest message accepted from a client.
const MAX_MESSAGE: usize = 1 << 20;

/// The `Sec-WebSocket-Accept` value for a client's key.
pub fn accept_key(key: &str) -> String {
    base64(&sha1(format!("{}{GUID}", key.trim()).as_bytes()))
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_be_bytes());
    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, v) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

#[derive(Debug, PartialEq)]
pub enum Message {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close,
}

/// Read one whole message (joining fragments). Blocks.
pub fn read_message(r: &mut impl Read) -> io::Result<Message> {
    let mut data = Vec::new();
    let mut kind = None;
    loop {
        let mut head = [0u8; 2];
        r.read_exact(&mut head)?;
        let fin = head[0] & 0x80 != 0;
        let opcode = head[0] & 0x0f;
        let masked = head[1] & 0x80 != 0;
        let mut len = (head[1] & 0x7f) as u64;
        if len == 126 {
            let mut b = [0u8; 2];
            r.read_exact(&mut b)?;
            len = u16::from_be_bytes(b) as u64;
        } else if len == 127 {
            let mut b = [0u8; 8];
            r.read_exact(&mut b)?;
            len = u64::from_be_bytes(b);
        }
        if len as usize + data.len() > MAX_MESSAGE {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "message too big"));
        }
        let mut mask = [0u8; 4];
        if masked {
            r.read_exact(&mut mask)?;
        }
        let mut payload = vec![0u8; len as usize];
        r.read_exact(&mut payload)?;
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i % 4];
            }
        }
        match opcode {
            0x8 => return Ok(Message::Close),
            0x9 => return Ok(Message::Ping(payload)),
            0xA => return Ok(Message::Pong),
            0x0 => data.extend_from_slice(&payload),
            0x1 | 0x2 => {
                kind = Some(opcode);
                data = payload;
            }
            _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "unknown frame")),
        }
        if fin {
            return Ok(match kind {
                Some(0x1) => Message::Text(String::from_utf8_lossy(&data).to_string()),
                _ => Message::Binary(data),
            });
        }
    }
}

/// Write one unmasked frame (server to client).
pub fn write_frame(w: &mut impl Write, opcode: u8, payload: &[u8]) -> io::Result<()> {
    let mut head = vec![0x80 | opcode];
    match payload.len() {
        n if n < 126 => head.push(n as u8),
        n if n < 65536 => {
            head.push(126);
            head.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            head.push(127);
            head.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    head.extend_from_slice(payload);
    w.write_all(&head)
}

pub fn write_text(w: &mut impl Write, text: &str) -> io::Result<()> {
    write_frame(w, 0x1, text.as_bytes())
}

/// A masked frame, as a client sends it (for tests).
#[cfg(test)]
pub fn client_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x12u8, 0x34, 0x56, 0x78];
    let mut out = vec![0x80 | opcode];
    match payload.len() {
        n if n < 126 => out.push(0x80 | n as u8),
        n => {
            out.push(0x80 | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_matches_the_rfc_example() {
        assert_eq!(accept_key("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        let h = sha1(b"abc");
        assert_eq!(h[..4], [0xa9, 0x99, 0x3e, 0x36]);
    }

    #[test]
    fn frames_round_trip() {
        let mut buf = client_frame(0x1, b"scene:2");
        buf.extend(client_frame(0x9, b"hi"));
        let big = "x".repeat(70_000);
        let mut r = std::io::Cursor::new(buf);
        assert_eq!(read_message(&mut r).unwrap(), Message::Text("scene:2".into()));
        assert_eq!(read_message(&mut r).unwrap(), Message::Ping(b"hi".to_vec()));
        let mut out = Vec::new();
        write_text(&mut out, &big).unwrap();
        assert_eq!(out[1], 127);
        assert_eq!(read_message(&mut std::io::Cursor::new(out)).unwrap(), Message::Text(big));
    }
}
