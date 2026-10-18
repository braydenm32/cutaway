//! MPEG transport stream muxer (H.264 + AAC), for SRT.

use super::{h264, AudioConfig, Ns, VideoConfig};

const PAT_PID: u16 = 0;
const PMT_PID: u16 = 0x1000;
const VIDEO_PID: u16 = 0x100;
const AUDIO_PID: u16 = 0x101;

fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04C1_1DB7 } else { crc << 1 };
        }
    }
    crc
}

pub struct TsMuxer {
    cc: [u8; 3], // continuity counters: PAT 0, video 1, audio 2
    cc_pmt: u8,
    video: VideoConfig,
    audio: AudioConfig,
    /// Added to every timestamp so they're never negative.
    base: Ns,
}

fn pts_90k(ns: Ns) -> u64 {
    ((ns as i128 * 90_000 / 1_000_000_000) as i64).rem_euclid(1 << 33) as u64
}

fn write_ts(out: &mut Vec<u8>, marker: u8, ts: u64) {
    out.push((marker << 4) | (((ts >> 30) as u8 & 0x07) << 1) | 1);
    out.extend_from_slice(&((((ts >> 15) & 0x7FFF) << 1 | 1) as u16).to_be_bytes());
    out.extend_from_slice(&(((ts & 0x7FFF) << 1 | 1) as u16).to_be_bytes());
}

impl TsMuxer {
    pub fn new(video: VideoConfig, audio: AudioConfig) -> Self {
        TsMuxer { cc: [0; 3], cc_pmt: 0, video, audio, base: 1_000_000_000 }
    }

    fn section(table: &[u8]) -> Vec<u8> {
        let mut p = vec![0u8]; // pointer field
        p.extend_from_slice(table);
        p.extend_from_slice(&crc32_mpeg(table).to_be_bytes());
        p
    }

    /// PAT + PMT packets.
    pub fn tables(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut pat = vec![0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01];
        pat.extend_from_slice(&(0xE000 | PMT_PID).to_be_bytes());
        // section_length = 13 (5 header bytes after length + 4 program + 4 CRC)
        let pat = Self::section(&pat);
        let cc = self.cc[0];
        self.cc[0] = (cc + 1) & 0xF;
        out.extend_from_slice(&Self::packet(PAT_PID, true, cc, None, &pat).0);

        let mut pmt = vec![0x02, 0xB0, 0x00, 0x00, 0x01, 0xC1, 0x00, 0x00];
        pmt.extend_from_slice(&(0xE000 | VIDEO_PID).to_be_bytes()); // PCR PID
        pmt.extend_from_slice(&[0xF0, 0x00]);
        pmt.push(0x1B);
        pmt.extend_from_slice(&(0xE000 | VIDEO_PID).to_be_bytes());
        pmt.extend_from_slice(&[0xF0, 0x00]);
        pmt.push(0x0F);
        pmt.extend_from_slice(&(0xE000 | AUDIO_PID).to_be_bytes());
        pmt.extend_from_slice(&[0xF0, 0x00]);
        let len = (pmt.len() - 3 + 4) as u16;
        pmt[1] = 0xB0 | (len >> 8) as u8;
        pmt[2] = len as u8;
        let pmt = Self::section(&pmt);
        let cc = self.cc_pmt;
        self.cc_pmt = (cc + 1) & 0xF;
        out.extend_from_slice(&Self::packet(PMT_PID, true, cc, None, &pmt).0);
        out
    }

    /// One 188-byte packet carrying as much of `payload` as fits. Returns
    /// the packet and how many payload bytes it took.
    fn packet(pid: u16, start: bool, cc: u8, pcr: Option<u64>, payload: &[u8]) -> ([u8; 188], usize) {
        let mut p = [0xFFu8; 188];
        p[0] = 0x47;
        p[1] = ((start as u8) << 6) | ((pid >> 8) as u8 & 0x1F);
        p[2] = pid as u8;
        if pcr.is_none() && payload.len() >= 184 {
            p[3] = 0x10 | (cc & 0xF);
            p[4..].copy_from_slice(&payload[..184]);
            return (p, 184);
        }
        // Adaptation field: flags (+ PCR), then 0xFF stuffing to fill.
        let mut content = vec![if pcr.is_some() { 0x10 } else { 0x00 }];
        if let Some(pcr) = pcr {
            let base = pcr / 300;
            let ext = pcr % 300;
            content.extend_from_slice(&[
                (base >> 25) as u8,
                (base >> 17) as u8,
                (base >> 9) as u8,
                (base >> 1) as u8,
                (((base & 1) << 7) as u8) | 0x7E | ((ext >> 8) as u8 & 1),
                ext as u8,
            ]);
        }
        let space = 184 - 1 - content.len();
        let n = payload.len().min(space);
        let af_len = content.len() + (space - n);
        p[3] = 0x30 | (cc & 0xF);
        p[4] = af_len as u8;
        p[5..5 + content.len()].copy_from_slice(&content);
        let at = 5 + af_len;
        p[at..at + n].copy_from_slice(&payload[..n]);
        (p, n)
    }

    fn pes_packets(&mut self, pid: u16, idx: usize, pes: &[u8], pcr: Option<u64>) -> Vec<u8> {
        let mut out = Vec::new();
        let mut i = 0;
        let mut first = true;
        while i < pes.len() {
            let cc = self.cc[idx];
            self.cc[idx] = (cc + 1) & 0xF;
            let (pkt, n) = Self::packet(pid, first, cc, if first { pcr } else { None }, &pes[i..]);
            out.extend_from_slice(&pkt);
            i += n;
            first = false;
        }
        out
    }

    /// Mux one video frame (AVCC). Times are ns from stream start.
    pub fn video(&mut self, pts: Ns, dts: Ns, keyframe: bool, avcc: &[u8]) -> Vec<u8> {
        let mut es = vec![0, 0, 0, 1, 0x09, 0xF0];
        if keyframe {
            for ps in &self.video.parameter_sets {
                es.extend_from_slice(&[0, 0, 0, 1]);
                es.extend_from_slice(ps);
            }
        }
        for nal in h264::split_avcc(avcc) {
            let t = h264::h264_nal_type(nal);
            if t == h264::NAL_AUD || t == h264::NAL_SPS || t == h264::NAL_PPS {
                continue;
            }
            es.extend_from_slice(&[0, 0, 0, 1]);
            es.extend_from_slice(nal);
        }
        let (p, d) = (pts_90k(pts + self.base), pts_90k(dts + self.base));
        let mut pes = vec![0, 0, 1, 0xE0, 0, 0, 0x80];
        if p != d {
            pes.extend_from_slice(&[0xC0, 10]);
            write_ts(&mut pes, 3, p);
            write_ts(&mut pes, 1, d);
        } else {
            pes.extend_from_slice(&[0x80, 5]);
            write_ts(&mut pes, 2, p);
        }
        pes.extend_from_slice(&es);
        let mut out = if keyframe { self.tables() } else { Vec::new() };
        let pcr = (d.saturating_sub(9000)) * 300; // 100 ms behind DTS
        out.extend(self.pes_packets(VIDEO_PID, 1, &pes, Some(pcr)));
        out
    }

    /// Mux one raw AAC frame.
    pub fn audio(&mut self, pts: Ns, raw: &[u8]) -> Vec<u8> {
        let mut es = h264::adts_header(self.audio.rate, self.audio.channels, raw.len()).to_vec();
        es.extend_from_slice(raw);
        let len = (es.len() + 8) as u16;
        let mut pes = vec![0, 0, 1, 0xC0];
        pes.extend_from_slice(&len.to_be_bytes());
        pes.extend_from_slice(&[0x80, 0x80, 5]);
        write_ts(&mut pes, 2, pts_90k(pts + self.base));
        pes.extend_from_slice(&es);
        self.pes_packets(AUDIO_PID, 2, &pes, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::mp4::tests::{audio_config, fake_video_config};

    #[test]
    fn packets_are_aligned_and_parse() {
        let mut m = TsMuxer::new(fake_video_config(), audio_config());
        let mut out = Vec::new();
        for i in 0..60i64 {
            let t = i * 1_000_000_000 / 30;
            let frame = h264::to_avcc([&[0x65u8, 0x88, 0x84, i as u8, 1, 2, 3][..]]);
            out.extend(m.video(t, t, i % 30 == 0, &frame));
            out.extend(m.audio(t, &[0x21, 0x10, 0x04, 0x60, 0x8C, 0x1C]));
        }
        assert_eq!(out.len() % 188, 0);
        assert!(out.chunks(188).all(|p| p[0] == 0x47));
        let p = std::env::temp_dir().join(format!("cutaway-ts-{}.ts", std::process::id()));
        std::fs::write(&p, &out).unwrap();
        if let Ok(o) = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "stream=codec_name", "-of", "csv=p=0"])
            .arg(&p)
            .output()
        {
            let s = String::from_utf8_lossy(&o.stdout);
            assert!(s.contains("h264") && s.contains("aac"), "{s} {}", String::from_utf8_lossy(&o.stderr));
        }
    }

    #[test]
    fn crc_known_value() {
        // PAT for program 1 -> PMT 0x1000 (from a reference stream)
        let pat = [0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01, 0xF0, 0x00];
        assert_eq!(crc32_mpeg(&pat), 0x2AB104B2);
    }
}
