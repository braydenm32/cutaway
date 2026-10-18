//! FLV tag bodies and metadata, as carried in RTMP messages.

use super::{AudioConfig, VideoConfig};

/// AMF0 values (just what RTMP publishing needs).
#[derive(Clone, Debug, PartialEq)]
pub enum Amf {
    Number(f64),
    Bool(bool),
    Str(String),
    Object(Vec<(String, Amf)>),
    EcmaArray(Vec<(String, Amf)>),
    Null,
    Undefined,
}

impl Amf {
    pub fn str(s: &str) -> Amf {
        Amf::Str(s.to_string())
    }

    pub fn encode(&self, out: &mut Vec<u8>) {
        fn key(out: &mut Vec<u8>, k: &str) {
            out.extend_from_slice(&(k.len() as u16).to_be_bytes());
            out.extend_from_slice(k.as_bytes());
        }
        match self {
            Amf::Number(n) => {
                out.push(0);
                out.extend_from_slice(&n.to_be_bytes());
            }
            Amf::Bool(b) => {
                out.push(1);
                out.push(*b as u8);
            }
            Amf::Str(s) => {
                out.push(2);
                key(out, s);
            }
            Amf::Object(props) | Amf::EcmaArray(props) => {
                if let Amf::EcmaArray(_) = self {
                    out.push(8);
                    out.extend_from_slice(&(props.len() as u32).to_be_bytes());
                } else {
                    out.push(3);
                }
                for (k, v) in props {
                    key(out, k);
                    v.encode(out);
                }
                out.extend_from_slice(&[0, 0, 9]);
            }
            Amf::Null => out.push(5),
            Amf::Undefined => out.push(6),
        }
    }

    /// Decode one value; returns it and the bytes used.
    pub fn decode(b: &[u8]) -> Option<(Amf, usize)> {
        fn string(b: &[u8]) -> Option<(String, usize)> {
            let len = u16::from_be_bytes([*b.first()?, *b.get(1)?]) as usize;
            let s = std::str::from_utf8(b.get(2..2 + len)?).ok()?.to_string();
            Some((s, 2 + len))
        }
        fn props(b: &[u8]) -> Option<(Vec<(String, Amf)>, usize)> {
            let mut i = 0;
            let mut out = Vec::new();
            loop {
                if b.get(i..i + 3)? == [0, 0, 9] {
                    return Some((out, i + 3));
                }
                let (k, n) = string(&b[i..])?;
                i += n;
                let (v, n) = Amf::decode(&b[i..])?;
                i += n;
                out.push((k, v));
            }
        }
        match *b.first()? {
            0 => Some((Amf::Number(f64::from_be_bytes(b.get(1..9)?.try_into().ok()?)), 9)),
            1 => Some((Amf::Bool(*b.get(1)? != 0), 2)),
            2 => string(&b[1..]).map(|(s, n)| (Amf::Str(s), n + 1)),
            3 => props(&b[1..]).map(|(p, n)| (Amf::Object(p), n + 1)),
            5 => Some((Amf::Null, 1)),
            6 => Some((Amf::Undefined, 1)),
            8 => props(&b[5..]).map(|(p, n)| (Amf::EcmaArray(p), n + 5)),
            _ => None,
        }
    }

    pub fn decode_all(mut b: &[u8]) -> Vec<Amf> {
        let mut out = Vec::new();
        while let Some((v, n)) = Amf::decode(b) {
            out.push(v);
            b = &b[n..];
        }
        out
    }

    pub fn get(&self, key: &str) -> Option<&Amf> {
        match self {
            Amf::Object(p) | Amf::EcmaArray(p) => p.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Amf::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Amf::Number(n) => Some(*n),
            _ => None,
        }
    }
}

/// `@setDataFrame onMetaData` body.
pub fn metadata(video: &VideoConfig, fps: u32, video_kbps: u32, audio: &AudioConfig, audio_kbps: u32) -> Vec<u8> {
    let mut out = Vec::new();
    Amf::str("@setDataFrame").encode(&mut out);
    Amf::str("onMetaData").encode(&mut out);
    Amf::EcmaArray(vec![
        ("duration".into(), Amf::Number(0.0)),
        ("width".into(), Amf::Number(video.width as f64)),
        ("height".into(), Amf::Number(video.height as f64)),
        ("videodatarate".into(), Amf::Number(video_kbps as f64)),
        ("framerate".into(), Amf::Number(fps as f64)),
        ("videocodecid".into(), Amf::Number(7.0)),
        ("audiodatarate".into(), Amf::Number(audio_kbps as f64)),
        ("audiosamplerate".into(), Amf::Number(audio.rate as f64)),
        ("audiosamplesize".into(), Amf::Number(16.0)),
        ("stereo".into(), Amf::Bool(audio.channels > 1)),
        ("audiocodecid".into(), Amf::Number(10.0)),
        ("encoder".into(), Amf::str("Cutaway")),
        ("filesize".into(), Amf::Number(0.0)),
    ])
    .encode(&mut out);
    out
}

pub fn video_sequence_header(cfg: &VideoConfig) -> Vec<u8> {
    let mut v = vec![0x17, 0x00, 0, 0, 0];
    v.extend_from_slice(&cfg.record);
    v
}

/// Video tag body for one AVCC frame. `cts_ms` is pts - dts.
pub fn video_frame(avcc: &[u8], keyframe: bool, cts_ms: i32) -> Vec<u8> {
    let mut v = Vec::with_capacity(avcc.len() + 5);
    v.push(if keyframe { 0x17 } else { 0x27 });
    v.push(0x01);
    v.extend_from_slice(&cts_ms.to_be_bytes()[1..]);
    v.extend_from_slice(avcc);
    v
}

pub fn audio_sequence_header(cfg: &AudioConfig) -> Vec<u8> {
    let mut v = vec![0xAF, 0x00];
    v.extend_from_slice(&cfg.asc);
    v
}

pub fn audio_frame(raw_aac: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(raw_aac.len() + 2);
    v.extend_from_slice(&[0xAF, 0x01]);
    v.extend_from_slice(raw_aac);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amf_roundtrip() {
        let v = Amf::Object(vec![("app".into(), Amf::str("live2")), ("n".into(), Amf::Number(3.0)), ("ok".into(), Amf::Bool(true))]);
        let mut b = Vec::new();
        Amf::str("connect").encode(&mut b);
        Amf::Number(1.0).encode(&mut b);
        v.encode(&mut b);
        Amf::Null.encode(&mut b);
        let back = Amf::decode_all(&b);
        assert_eq!(back, vec![Amf::str("connect"), Amf::Number(1.0), v, Amf::Null]);
        assert_eq!(back[2].get("app").and_then(|a| a.as_str()), Some("live2"));
    }
}
