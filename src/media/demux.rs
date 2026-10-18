//! Reading MP4 / MOV files (for the media player): the sample tables of the
//! first video and audio tracks, from a regular or fragmented file.

use super::{h264, AudioConfig, Ns, ProRes, VideoCodec, VideoConfig, NS_PER_SEC};
use anyhow::{bail, Context, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy, Debug)]
pub struct SampleInfo {
    pub offset: u64,
    pub size: u32,
    /// Decode time in track ticks.
    pub dts: i64,
    /// Composition offset in ticks.
    pub cts: i32,
    #[cfg_attr(not(test), allow(dead_code))]
    pub sync: bool,
}

#[derive(Clone, Debug)]
pub enum TrackKind {
    Video(VideoConfig),
    Audio(AudioConfig),
    Other,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: u32,
    pub kind: TrackKind,
    pub timescale: u32,
    pub samples: Vec<SampleInfo>,
    /// Media time shown first (edit list), in ticks.
    pub start: i64,
    has_edit: bool,
    defaults: (u32, u32, u32), // trex: duration, size, flags
}

impl Track {
    /// Presentation time of sample `i`, from the start of the file.
    pub fn pts(&self, i: usize) -> Ns {
        let s = &self.samples[i];
        self.ticks_to_ns(s.dts + s.cts as i64 - self.start)
    }

    pub fn ticks_to_ns(&self, t: i64) -> Ns {
        (t as i128 * NS_PER_SEC as i128 / self.timescale.max(1) as i128) as Ns
    }

    /// Length of the track.
    pub fn duration(&self) -> Ns {
        let Some(last) = self.samples.last() else { return 0 };
        let step = if self.samples.len() > 1 { (last.dts - self.samples[self.samples.len() - 2].dts).max(0) } else { 0 };
        let end = self.samples.iter().map(|s| s.dts + s.cts as i64).max().unwrap_or(0);
        self.ticks_to_ns(end + step - self.start)
    }

    /// Frames per second (from the average sample duration).
    #[cfg(test)]
    pub fn fps(&self) -> f64 {
        let n = self.samples.len();
        if n < 2 {
            return 30.0;
        }
        let span = (self.samples[n - 1].dts - self.samples[0].dts) as f64 / self.timescale.max(1) as f64;
        if span <= 0.0 {
            30.0
        } else {
            (n - 1) as f64 / span
        }
    }
}

pub struct Mp4Reader {
    file: File,
    pub tracks: Vec<Track>,
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}
fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}
fn be64(b: &[u8], at: usize) -> u64 {
    u64::from_be_bytes(b[at..at + 8].try_into().unwrap())
}

/// Boxes in `data`: (type, payload).
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= data.len() {
        let mut size = be32(data, i) as usize;
        let kind: [u8; 4] = data[i + 4..i + 8].try_into().unwrap();
        let mut hdr = 8;
        if size == 1 && i + 16 <= data.len() {
            size = be64(data, i + 8) as usize;
            hdr = 16;
        } else if size == 0 {
            size = data.len() - i;
        }
        if size < hdr || i + size > data.len() {
            break;
        }
        out.push((kind, &data[i + hdr..i + size]));
        i += size;
    }
    out
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data).into_iter().find(|(k, _)| k == kind).map(|(_, b)| b)
}

/// First box of `kind` anywhere below `data` (searching sample entries too).
fn find_deep<'a>(data: &'a [u8], kind: &[u8; 4], depth: usize) -> Option<&'a [u8]> {
    if depth > 6 {
        return None;
    }
    // Scan every offset: sample entries have fixed headers before their children.
    let mut i = 0;
    while i + 8 <= data.len() {
        if &data[i + 4..i + 8] == kind {
            let size = be32(data, i) as usize;
            if size >= 8 && i + size <= data.len() {
                return Some(&data[i + 8..i + size]);
            }
        }
        i += 1;
    }
    None
}

/// MPEG-4 descriptor length ("expandable" size).
fn desc_len(b: &[u8], i: &mut usize) -> usize {
    let mut n = 0usize;
    for _ in 0..4 {
        let Some(&c) = b.get(*i) else { break };
        *i += 1;
        n = (n << 7) | (c & 0x7f) as usize;
        if c & 0x80 == 0 {
            break;
        }
    }
    n
}

/// The AudioSpecificConfig inside an `esds` payload.
fn asc_from_esds(esds: &[u8]) -> Option<Vec<u8>> {
    let b = esds.get(4..)?; // version/flags
    let mut i = 0;
    while i < b.len() {
        let tag = b[i];
        i += 1;
        let len = desc_len(b, &mut i);
        match tag {
            0x03 => {
                // ES_ID, flags (+ optional fields).
                let flags = *b.get(i + 2)?;
                i += 3;
                if flags & 0x80 != 0 {
                    i += 2;
                }
                if flags & 0x40 != 0 {
                    i += 1 + *b.get(i)? as usize;
                }
                if flags & 0x20 != 0 {
                    i += 2;
                }
            }
            0x04 => i += 13,
            0x05 => return b.get(i..i + len).map(|s| s.to_vec()),
            _ => i += len,
        }
    }
    None
}

/// Rate and channels from an AudioSpecificConfig.
pub fn parse_asc(asc: &[u8]) -> Option<(u32, u16)> {
    const RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];
    if asc.len() < 2 {
        return None;
    }
    let mut padded = [0u8; 8];
    for (i, b) in asc.iter().take(8).enumerate() {
        padded[i] = *b;
    }
    let bits = u64::from_be_bytes(padded);
    let mut pos = 0u32;
    let mut take = |n: u32| {
        let v = (bits >> (64 - pos - n)) & ((1u64 << n) - 1);
        pos += n;
        v as u32
    };
    if take(5) == 31 {
        take(6);
    }
    let idx = take(4);
    let rate = if idx == 15 { take(24) } else { *RATES.get(idx as usize)? };
    let ch = take(4) as u16;
    Some((rate, if ch == 0 { 2 } else { ch.min(8) }))
}

fn parse_stsd(stsd: &[u8], handler: &[u8; 4]) -> TrackKind {
    let Some(entry) = boxes(stsd.get(8..).unwrap_or(&[])).into_iter().next() else { return TrackKind::Other };
    let (fourcc, body) = entry;
    match handler {
        b"vide" if body.len() >= 78 => {
            let (w, h) = (be16(body, 24) as u32, be16(body, 26) as u32);
            let ext = &body[78..];
            let codec = match &fourcc {
                b"avc1" | b"avc3" => VideoCodec::H264,
                b"hvc1" | b"hev1" => VideoCodec::Hevc,
                b"apco" => VideoCodec::ProRes(ProRes::Proxy),
                b"apcs" => VideoCodec::ProRes(ProRes::Lt),
                b"apcn" => VideoCodec::ProRes(ProRes::Standard),
                b"apch" | b"ap4h" | b"ap4x" => VideoCodec::ProRes(ProRes::Hq),
                _ => return TrackKind::Other,
            };
            let (record, parameter_sets) = match codec {
                VideoCodec::H264 => match child(ext, b"avcC") {
                    Some(r) => (r.to_vec(), h264::parse_avcc(r)),
                    None => return TrackKind::Other,
                },
                VideoCodec::Hevc => match child(ext, b"hvcC") {
                    Some(r) => (r.to_vec(), h264::parse_hvcc(r)),
                    None => return TrackKind::Other,
                },
                VideoCodec::ProRes(_) => (Vec::new(), Vec::new()),
            };
            let mut cfg = VideoConfig { codec, width: w, height: h, record, parameter_sets };
            if let VideoCodec::ProRes(_) = codec {
                // Keep the exact four-character code for the decoder.
                cfg.record = fourcc.to_vec();
            }
            TrackKind::Video(cfg)
        }
        b"soun" if &fourcc == b"mp4a" && body.len() >= 28 => {
            let channels = be16(body, 16);
            let rate = be32(body, 24) >> 16;
            let asc = find_deep(body, b"esds", 0).and_then(asc_from_esds);
            match asc {
                Some(asc) => {
                    let (r, c) = parse_asc(&asc).unwrap_or((rate, channels));
                    TrackKind::Audio(AudioConfig { rate: r, channels: c, asc, priming: 0 })
                }
                None => TrackKind::Other,
            }
        }
        _ => TrackKind::Other,
    }
}

/// Sample tables of a regular (non-fragmented) track.
fn parse_stbl(stbl: &[u8]) -> Result<Vec<SampleInfo>> {
    let stsz = child(stbl, b"stsz").context("stsz")?;
    let fixed = be32(stsz, 4);
    let count = be32(stsz, 8) as usize;
    let sizes: Vec<u32> = if fixed != 0 { vec![fixed; count] } else { (0..count).map(|i| be32(stsz, 12 + i * 4)).collect() };
    let offsets: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
        (0..be32(stco, 4) as usize).map(|i| be32(stco, 8 + i * 4) as u64).collect()
    } else if let Some(co64) = child(stbl, b"co64") {
        (0..be32(co64, 4) as usize).map(|i| be64(co64, 8 + i * 8)).collect()
    } else {
        bail!("no chunk offsets")
    };
    let stsc = child(stbl, b"stsc").context("stsc")?;
    let stsc: Vec<(u32, u32)> = (0..be32(stsc, 4) as usize).map(|i| (be32(stsc, 8 + i * 12), be32(stsc, 12 + i * 12))).collect();
    let stts = child(stbl, b"stts").context("stts")?;
    let mut durations = Vec::with_capacity(count);
    for i in 0..be32(stts, 4) as usize {
        let (n, d) = (be32(stts, 8 + i * 8), be32(stts, 12 + i * 8));
        durations.extend(std::iter::repeat_n(d, n as usize));
    }
    let mut ctts = Vec::new();
    if let Some(c) = child(stbl, b"ctts") {
        for i in 0..be32(c, 4) as usize {
            let (n, off) = (be32(c, 8 + i * 8), be32(c, 12 + i * 8) as i32);
            ctts.extend(std::iter::repeat_n(off, n as usize));
        }
    }
    let sync: Option<std::collections::HashSet<u32>> = child(stbl, b"stss").map(|s| (0..be32(s, 4) as usize).map(|i| be32(s, 8 + i * 4)).collect());

    let mut out = Vec::with_capacity(count);
    let mut sample = 0usize;
    let mut dts = 0i64;
    for (ci, &chunk_off) in offsets.iter().enumerate() {
        let chunk_no = ci as u32 + 1;
        let per = stsc.iter().rev().find(|e| e.0 <= chunk_no).map(|e| e.1).unwrap_or(1);
        let mut off = chunk_off;
        for _ in 0..per {
            if sample >= count {
                break;
            }
            out.push(SampleInfo {
                offset: off,
                size: sizes[sample],
                dts,
                cts: ctts.get(sample).copied().unwrap_or(0),
                sync: sync.as_ref().is_none_or(|s| s.contains(&(sample as u32 + 1))),
            });
            off += sizes[sample] as u64;
            dts += *durations.get(sample).unwrap_or(&0) as i64;
            sample += 1;
        }
    }
    Ok(out)
}

impl Mp4Reader {
    pub fn open(path: &Path) -> Result<Mp4Reader> {
        let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
        let len = file.metadata()?.len();
        // Walk the top level, reading only moov and moof boxes.
        let mut pos = 0u64;
        let mut tracks: Vec<Track> = Vec::new();
        let mut hdr = [0u8; 16];
        while pos + 8 <= len {
            file.seek(SeekFrom::Start(pos))?;
            file.read_exact(&mut hdr[..8])?;
            let mut size = u32::from_be_bytes(hdr[..4].try_into().unwrap()) as u64;
            let kind: [u8; 4] = hdr[4..8].try_into().unwrap();
            let mut head = 8u64;
            if size == 1 {
                file.read_exact(&mut hdr[8..16])?;
                size = u64::from_be_bytes(hdr[8..16].try_into().unwrap());
                head = 16;
            } else if size == 0 {
                size = len - pos;
            }
            if size < head {
                break;
            }
            if &kind == b"moov" || &kind == b"moof" {
                let mut body = vec![0u8; (size - head) as usize];
                file.read_exact(&mut body)?;
                if &kind == b"moov" {
                    tracks = Self::parse_moov(&body)?;
                } else {
                    Self::parse_moof(&body, pos, &mut tracks);
                }
            }
            pos += size;
        }
        // Without an edit list, the earliest presented sample starts the track.
        for t in tracks.iter_mut().filter(|t| !t.has_edit) {
            t.start = t.samples.iter().map(|s| s.dts + s.cts as i64).min().unwrap_or(0);
        }
        if tracks.iter().all(|t| matches!(t.kind, TrackKind::Other)) {
            bail!("{}: no H.264, HEVC, ProRes or AAC tracks found", path.display());
        }
        Ok(Mp4Reader { file, tracks })
    }

    fn parse_moov(moov: &[u8]) -> Result<Vec<Track>> {
        let mut trex = std::collections::HashMap::new();
        if let Some(mvex) = child(moov, b"mvex") {
            for (k, b) in boxes(mvex) {
                if &k == b"trex" && b.len() >= 24 {
                    trex.insert(be32(b, 4), (be32(b, 12), be32(b, 16), be32(b, 20)));
                }
            }
        }
        let mut out = Vec::new();
        for (k, trak) in boxes(moov) {
            if &k != b"trak" {
                continue;
            }
            let Some(tkhd) = child(trak, b"tkhd") else { continue };
            let id = if tkhd[0] == 1 { be32(tkhd, 20) } else { be32(tkhd, 12) };
            let Some(mdia) = child(trak, b"mdia") else { continue };
            let Some(mdhd) = child(mdia, b"mdhd") else { continue };
            let timescale = if mdhd[0] == 1 { be32(mdhd, 20) } else { be32(mdhd, 12) };
            let handler: [u8; 4] = child(mdia, b"hdlr").map(|h| h[8..12].try_into().unwrap()).unwrap_or(*b"none");
            let Some(stbl) = child(mdia, b"minf").and_then(|m| child(m, b"stbl")) else { continue };
            let kind = child(stbl, b"stsd").map(|s| parse_stsd(s, &handler)).unwrap_or(TrackKind::Other);
            let samples = parse_stbl(stbl).unwrap_or_default();
            // Edit list: the first non-empty edit's media time.
            let mut start = 0i64;
            let has_edit = child(trak, b"edts").is_some();
            if let Some(elst) = child(trak, b"edts").and_then(|e| child(e, b"elst")) {
                let v1 = elst[0] == 1;
                let n = be32(elst, 4) as usize;
                for i in 0..n {
                    let mt = if v1 { be64(elst, 8 + i * 20 + 8) as i64 } else { be32(elst, 8 + i * 12 + 4) as i32 as i64 };
                    if mt >= 0 {
                        start = mt;
                        break;
                    }
                }
            }
            out.push(Track { id, kind, timescale, samples, start, has_edit, defaults: trex.get(&id).copied().unwrap_or((0, 0, 0)) });
        }
        Ok(out)
    }

    /// Adds a fragment's samples to its tracks.
    fn parse_moof(moof: &[u8], moof_pos: u64, tracks: &mut [Track]) {
        for (k, traf) in boxes(moof) {
            if &k != b"traf" {
                continue;
            }
            let Some(tfhd) = child(traf, b"tfhd") else { continue };
            let flags = be32(tfhd, 0) & 0xFFFFFF;
            let id = be32(tfhd, 4);
            let Some(track) = tracks.iter_mut().find(|t| t.id == id) else { continue };
            let mut i = 8;
            let mut base = moof_pos;
            if flags & 1 != 0 {
                base = be64(tfhd, i);
                i += 8;
            }
            if flags & 2 != 0 {
                i += 4;
            }
            let (mut d_dur, mut d_size, mut d_flags) = track.defaults;
            if flags & 8 != 0 {
                d_dur = be32(tfhd, i);
                i += 4;
            }
            if flags & 0x10 != 0 {
                d_size = be32(tfhd, i);
                i += 4;
            }
            if flags & 0x20 != 0 {
                d_flags = be32(tfhd, i);
            }
            let mut dts = child(traf, b"tfdt")
                .map(|t| if t[0] == 1 { be64(t, 4) as i64 } else { be32(t, 4) as i64 })
                .unwrap_or_else(|| track.samples.last().map(|s| s.dts + d_dur as i64).unwrap_or(0));
            for (k, trun) in boxes(traf) {
                if &k != b"trun" {
                    continue;
                }
                let tf = be32(trun, 0) & 0xFFFFFF;
                let count = be32(trun, 4) as usize;
                let mut j = 8;
                let mut offset = base;
                if tf & 1 != 0 {
                    offset = (base as i64 + be32(trun, j) as i32 as i64) as u64;
                    j += 4;
                }
                let mut first_flags = None;
                if tf & 4 != 0 {
                    first_flags = Some(be32(trun, j));
                    j += 4;
                }
                for n in 0..count {
                    let mut dur = d_dur;
                    let mut size = d_size;
                    let mut sflags = if n == 0 { first_flags.unwrap_or(d_flags) } else { d_flags };
                    let mut cts = 0i32;
                    if tf & 0x100 != 0 {
                        dur = be32(trun, j);
                        j += 4;
                    }
                    if tf & 0x200 != 0 {
                        size = be32(trun, j);
                        j += 4;
                    }
                    if tf & 0x400 != 0 {
                        sflags = be32(trun, j);
                        j += 4;
                    }
                    if tf & 0x800 != 0 {
                        cts = be32(trun, j) as i32;
                        j += 4;
                    }
                    let sync = sflags & 0x0001_0000 == 0;
                    track.samples.push(SampleInfo { offset, size, dts, cts, sync });
                    offset += size as u64;
                    dts += dur as i64;
                }
                base = offset;
            }
        }
    }

    pub fn video(&self) -> Option<usize> {
        self.tracks.iter().position(|t| matches!(t.kind, TrackKind::Video(_)) && !t.samples.is_empty())
    }

    pub fn audio(&self) -> Option<usize> {
        self.tracks.iter().position(|t| matches!(t.kind, TrackKind::Audio(_)) && !t.samples.is_empty())
    }

    pub fn read(&mut self, track: usize, index: usize) -> Result<Vec<u8>> {
        let s = self.tracks[track].samples[index];
        let mut buf = vec![0u8; s.size as usize];
        self.file.seek(SeekFrom::Start(s.offset))?;
        self.file.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Length of the longest track.
    pub fn duration(&self) -> Ns {
        self.tracks.iter().filter(|t| !matches!(t.kind, TrackKind::Other)).map(|t| t.duration()).max().unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::mp4::{tests::audio_config, tests::fake_video_config, Brand, Mp4Writer};

    #[test]
    fn reads_back_what_we_write() {
        let p = std::env::temp_dir().join(format!("cutaway-demux-{}.mp4", std::process::id()));
        let mut w = Mp4Writer::create(&p, Brand::Mp4).unwrap();
        let v = w.add_video(&fake_video_config(), 30);
        let a = w.add_audio(&audio_config());
        for i in 0..90i64 {
            w.write(v, i * NS_PER_SEC / 30, &vec![i as u8; 100 + i as usize], i % 30 == 0).unwrap();
            if i % 2 == 0 {
                w.write(a, (i / 2) * 1024 * NS_PER_SEC / 48000, &[7u8; 50], true).unwrap();
            }
        }
        w.finish().unwrap();
        let mut r = Mp4Reader::open(&p).unwrap();
        let vi = r.video().unwrap();
        let ai = r.audio().unwrap();
        assert_eq!(r.tracks[vi].samples.len(), 90);
        assert!((r.tracks[vi].fps() - 30.0).abs() < 0.01);
        assert!(r.tracks[vi].samples[30].sync && !r.tracks[vi].samples[31].sync);
        assert_eq!(r.read(vi, 5).unwrap(), vec![5u8; 105]);
        assert_eq!(r.tracks[vi].pts(30), NS_PER_SEC);
        match &r.tracks[ai].kind {
            TrackKind::Audio(c) => assert_eq!((c.rate, c.channels), (48000, 2)),
            other => panic!("{other:?}"),
        }
        assert_eq!(r.read(ai, 3).unwrap(), vec![7u8; 50]);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn reads_ffmpeg_files() {
        // Regular, fragmented and QuickTime files from ffmpeg, if it's installed.
        let dir = std::env::temp_dir().join(format!("cutaway-demux-ff-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let cases: [(&str, &[&str]); 3] = [
            ("plain.mp4", &["-c:v", "libx264", "-bf", "2", "-c:a", "aac"]),
            ("frag.mp4", &["-c:v", "libx264", "-c:a", "aac", "-movflags", "frag_keyframe+empty_moov"]),
            ("prores.mov", &["-c:v", "prores_ks", "-profile:v", "2", "-c:a", "aac"]),
        ];
        for (name, args) in cases {
            let out = dir.join(name);
            let ok = std::process::Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=2", "-f", "lavfi", "-i", "sine=duration=2:sample_rate=44100"])
                .args(args)
                .arg(&out)
                .status()
                .is_ok_and(|s| s.success());
            if !ok {
                eprintln!("skipping {name}: ffmpeg unavailable");
                continue;
            }
            let mut r = Mp4Reader::open(&out).unwrap();
            let vi = r.video().unwrap_or_else(|| panic!("{name}: no video"));
            let ai = r.audio().unwrap_or_else(|| panic!("{name}: no audio"));
            assert_eq!(r.tracks[vi].samples.len(), 50, "{name}");
            assert!((r.tracks[vi].fps() - 25.0).abs() < 0.1, "{name}");
            let d = r.tracks[vi].duration();
            assert!((d - 2 * NS_PER_SEC).abs() < 50_000_000, "{name}: {d}");
            match &r.tracks[ai].kind {
                TrackKind::Audio(c) => assert_eq!(c.rate, 44100, "{name}"),
                other => panic!("{name}: {other:?}"),
            }
            let first = r.read(vi, 0).unwrap();
            if name.ends_with(".mov") {
                assert_eq!(&first[4..8], b"icpf", "{name}: a ProRes frame");
            } else {
                // AVCC: first NAL length fits the sample.
                assert!(be32(&first, 0) as usize + 4 <= first.len(), "{name}");
                // With B-frames, presentation times aren't in decode order.
                if name == "plain.mp4" {
                    let pts: Vec<Ns> = (0..6).map(|i| r.tracks[vi].pts(i)).collect();
                    assert!(pts.windows(2).any(|w| w[1] < w[0]), "{pts:?}");
                    assert_eq!(*pts.iter().min().unwrap(), 0, "edit list applied: {pts:?}");
                }
            }
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
