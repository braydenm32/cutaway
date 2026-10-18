//! MPEG-TS demuxing (for SRT input): H.264 / HEVC video and ADTS AAC audio,
//! with 90 kHz timestamps unwrapped to nanoseconds.

use super::{h264, Ns};

/// An elementary-stream unit pulled out of the transport stream.
#[derive(Debug, Clone)]
pub enum TsUnit {
    /// One access unit: NAL units without start codes.
    Video { hevc: bool, pts: Ns, dts: Ns, nals: Vec<Vec<u8>> },
    /// One raw AAC frame and its configuration.
    Audio { pts: Ns, rate: u32, channels: u16, frame: Vec<u8> },
}

#[derive(Default)]
struct Pes {
    buf: Vec<u8>,
    started: bool,
}

/// Turns 33-bit wrapping 90 kHz times into a continuous count.
#[derive(Default, Clone, Copy)]
struct Unwrap {
    last: Option<i64>,
    offset: i64,
}

impl Unwrap {
    fn ns(&mut self, t: i64) -> Ns {
        const WRAP: i64 = 1 << 33;
        if let Some(last) = self.last {
            if t - last < -WRAP / 2 {
                self.offset += WRAP;
            } else if t - last > WRAP / 2 {
                self.offset -= WRAP;
            }
        }
        self.last = Some(t);
        ((t + self.offset) as i128 * 1_000_000_000 / 90_000) as Ns
    }
}

pub struct TsDemuxer {
    pmt_pid: Option<u16>,
    /// pid -> stream type
    streams: Vec<(u16, u8)>,
    pes: std::collections::HashMap<u16, Pes>,
    pending: Vec<u8>,
    clock: Unwrap,
}

impl Default for TsDemuxer {
    fn default() -> Self {
        Self::new()
    }
}

const ST_H264: u8 = 0x1B;
const ST_HEVC: u8 = 0x24;
const ST_AAC: u8 = 0x0F;

fn pts_at(b: &[u8]) -> i64 {
    (((b[0] as i64 >> 1) & 7) << 30) | ((b[1] as i64) << 22) | (((b[2] as i64) >> 1) << 15) | ((b[3] as i64) << 7) | ((b[4] as i64) >> 1)
}

impl TsDemuxer {
    pub fn new() -> Self {
        TsDemuxer { pmt_pid: None, streams: Vec::new(), pes: Default::default(), pending: Vec::new(), clock: Unwrap::default() }
    }

    /// Feed bytes (any amount); returns the units completed.
    pub fn push(&mut self, data: &[u8]) -> Vec<TsUnit> {
        self.pending.extend_from_slice(data);
        let mut out = Vec::new();
        let mut i = 0;
        while i + 188 <= self.pending.len() {
            if self.pending[i] != 0x47 {
                i += 1; // resync
                continue;
            }
            let pkt: [u8; 188] = self.pending[i..i + 188].try_into().unwrap();
            self.packet(&pkt, &mut out);
            i += 188;
        }
        self.pending.drain(..i);
        out
    }

    /// Emit whatever is still buffered (end of stream).
    #[cfg(test)]
    pub fn flush(&mut self) -> Vec<TsUnit> {
        let mut out = Vec::new();
        let pids: Vec<u16> = self.pes.keys().copied().collect();
        for pid in pids {
            if let Some(p) = self.pes.remove(&pid) {
                if p.started {
                    self.finish_pes(pid, &p.buf, &mut out);
                }
            }
        }
        out
    }

    fn packet(&mut self, p: &[u8; 188], out: &mut Vec<TsUnit>) {
        let pusi = p[1] & 0x40 != 0;
        let pid = (((p[1] & 0x1f) as u16) << 8) | p[2] as u16;
        let afc = (p[3] >> 4) & 3;
        let mut at = 4;
        if afc == 2 || afc == 0 {
            return;
        }
        if afc == 3 {
            at += 1 + p[4] as usize;
        }
        if at >= 188 {
            return;
        }
        let payload = &p[at..];
        if pid == 0 {
            if pusi {
                let s = &payload[1 + payload[0] as usize..];
                // PAT: first program's PMT pid.
                if s.len() >= 12 {
                    let len = (((s[1] & 0x0f) as usize) << 8) | s[2] as usize;
                    let mut j = 8;
                    while j + 4 <= 3 + len - 4 && j + 4 <= s.len() {
                        let prog = ((s[j] as u16) << 8) | s[j + 1] as u16;
                        let pmt = (((s[j + 2] & 0x1f) as u16) << 8) | s[j + 3] as u16;
                        if prog != 0 {
                            self.pmt_pid = Some(pmt);
                            break;
                        }
                        j += 4;
                    }
                }
            }
            return;
        }
        if Some(pid) == self.pmt_pid {
            if pusi {
                let s = &payload[1 + payload[0] as usize..];
                if s.len() >= 12 {
                    let len = (((s[1] & 0x0f) as usize) << 8) | s[2] as usize;
                    let info = (((s[10] & 0x0f) as usize) << 8) | s[11] as usize;
                    let mut j = 12 + info;
                    let end = (3 + len).saturating_sub(4).min(s.len());
                    let mut streams = Vec::new();
                    while j + 5 <= end {
                        let st = s[j];
                        let epid = (((s[j + 1] & 0x1f) as u16) << 8) | s[j + 2] as u16;
                        let es_info = (((s[j + 3] & 0x0f) as usize) << 8) | s[j + 4] as usize;
                        if matches!(st, ST_H264 | ST_HEVC | ST_AAC) {
                            streams.push((epid, st));
                        }
                        j += 5 + es_info;
                    }
                    self.streams = streams;
                }
            }
            return;
        }
        if !self.streams.iter().any(|s| s.0 == pid) {
            return;
        }
        if pusi {
            if let Some(prev) = self.pes.remove(&pid) {
                if prev.started {
                    self.finish_pes(pid, &prev.buf, out);
                }
            }
            self.pes.insert(pid, Pes { buf: payload.to_vec(), started: true });
        } else if let Some(pes) = self.pes.get_mut(&pid) {
            pes.buf.extend_from_slice(payload);
        }
    }

    fn finish_pes(&mut self, pid: u16, b: &[u8], out: &mut Vec<TsUnit>) {
        if b.len() < 9 || b[0] != 0 || b[1] != 0 || b[2] != 1 {
            return;
        }
        let flags = b[7];
        let hdr = 9 + b[8] as usize;
        if hdr > b.len() {
            return;
        }
        let pts = if flags & 0x80 != 0 { Some(pts_at(&b[9..14])) } else { None };
        let dts = if flags & 0x40 != 0 { Some(pts_at(&b[14..19])) } else { pts };
        let Some(pts) = pts else { return };
        let data = &b[hdr..];
        let st = self.streams.iter().find(|s| s.0 == pid).map(|s| s.1).unwrap_or(0);
        let dts_ns = self.clock.ns(dts.unwrap_or(pts));
        let pts_ns = dts_ns + ((pts - dts.unwrap_or(pts)) as i128 * 1_000_000_000 / 90_000) as Ns;
        match st {
            ST_H264 | ST_HEVC => {
                let nals: Vec<Vec<u8>> = h264::split_annexb(data).into_iter().filter(|n| !n.is_empty()).map(|n| n.to_vec()).collect();
                if !nals.is_empty() {
                    out.push(TsUnit::Video { hevc: st == ST_HEVC, pts: pts_ns, dts: dts_ns, nals });
                }
            }
            ST_AAC => {
                // ADTS frames, 1024 samples each.
                let mut i = 0;
                let mut n = 0i64;
                while i + 7 <= data.len() {
                    if data[i] != 0xFF || data[i + 1] & 0xF0 != 0xF0 {
                        i += 1;
                        continue;
                    }
                    let protection_absent = data[i + 1] & 1 == 1;
                    let sf = (data[i + 2] >> 2) & 0x0f;
                    let ch = (((data[i + 2] & 1) << 2) | (data[i + 3] >> 6)) as u16;
                    let len = (((data[i + 3] & 3) as usize) << 11) | ((data[i + 4] as usize) << 3) | ((data[i + 5] >> 5) as usize);
                    let head = if protection_absent { 7 } else { 9 };
                    if len < head || i + len > data.len() {
                        break;
                    }
                    const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
                    let rate = RATES.get(sf as usize).copied().unwrap_or(48000);
                    let t = pts_ns + n * 1024 * 1_000_000_000 / rate as i64;
                    out.push(TsUnit::Audio { pts: t, rate, channels: if ch == 0 { 2 } else { ch }, frame: data[i + head..i + len].to_vec() });
                    n += 1;
                    i += len;
                }
            }
            _ => {}
        }
    }
}

/// Keyframe test for an access unit.
pub fn is_keyframe(hevc: bool, nals: &[Vec<u8>]) -> bool {
    nals.iter().any(|n| {
        if hevc {
            let t = (n.first().copied().unwrap_or(0) >> 1) & 0x3f;
            (16..=21).contains(&t)
        } else {
            h264::h264_nal_type(n) == h264::NAL_IDR
        }
    })
}

/// Parameter sets in an access unit (SPS/PPS, plus VPS for HEVC).
pub fn parameter_sets(hevc: bool, nals: &[Vec<u8>]) -> Vec<Vec<u8>> {
    nals.iter()
        .filter(|n| {
            if hevc {
                let t = (n.first().copied().unwrap_or(0) >> 1) & 0x3f;
                (32..=34).contains(&t)
            } else {
                let t = h264::h264_nal_type(n);
                t == h264::NAL_SPS || t == h264::NAL_PPS
            }
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demuxes_ffmpeg_ts() {
        let dir = std::env::temp_dir().join(format!("cutaway-tsdemux-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("in.ts");
        let ok = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=2", "-f", "lavfi", "-i", "sine=duration=2:sample_rate=48000"])
            .args(["-c:v", "libx264", "-g", "25", "-c:a", "aac", "-f", "mpegts"])
            .arg(&out)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("skipping: ffmpeg unavailable");
            return;
        }
        let data = std::fs::read(&out).unwrap();
        let mut d = TsDemuxer::new();
        let mut units = Vec::new();
        // Odd chunk sizes, like network reads.
        for c in data.chunks(1316 + 7) {
            units.extend(d.push(c));
        }
        units.extend(d.flush());
        let video: Vec<&TsUnit> = units.iter().filter(|u| matches!(u, TsUnit::Video { .. })).collect();
        let audio: Vec<&TsUnit> = units.iter().filter(|u| matches!(u, TsUnit::Audio { .. })).collect();
        assert_eq!(video.len(), 50);
        assert!((90..=96).contains(&audio.len()), "{}", audio.len());
        let TsUnit::Video { nals, hevc, .. } = video[0] else { unreachable!() };
        assert!(is_keyframe(*hevc, nals));
        assert_eq!(parameter_sets(*hevc, nals).len(), 2);
        let (TsUnit::Video { dts: a, .. }, TsUnit::Video { dts: b, .. }) = (video[0], video[25]) else { unreachable!() };
        assert!((b - a - 1_000_000_000).abs() < 1_000_000, "{a} {b}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unwraps_33_bit_time() {
        let mut u = Unwrap::default();
        let a = u.ns((1 << 33) - 90_000);
        let b = u.ns(0);
        assert_eq!(b - a, 1_000_000_000);
    }
}
