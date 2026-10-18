//! WAV writer: 16/24-bit PCM or 32-bit float, any channel count, and RF64
//! when a recording passes 4 GB (an 8-channel 24-bit take gets there in
//! about an hour). The header is refreshed every second, so a crash leaves
//! a playable file.

use super::mirror::MirrorFile;
use anyhow::Result;
use std::fs::OpenOptions;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Broadcast WAV time stamp, so DAWs and editors place the file at the
/// time of day it was recorded.
#[derive(Clone, Debug)]
pub struct Bext {
    pub description: String,
    /// "YYYY-MM-DD" and "HH:MM:SS".
    pub date: String,
    pub time: String,
    /// Samples since midnight at the first sample.
    pub time_reference: u64,
}

fn bext_chunk(b: &Bext) -> Vec<u8> {
    fn field(out: &mut Vec<u8>, s: &str, len: usize) {
        let bytes = s.as_bytes();
        let n = bytes.len().min(len);
        out.extend_from_slice(&bytes[..n]);
        out.extend(std::iter::repeat_n(0u8, len - n));
    }
    let mut c = Vec::with_capacity(602);
    field(&mut c, &b.description, 256);
    field(&mut c, "Cutaway", 32); // originator
    field(&mut c, "", 32); // originator reference
    field(&mut c, &b.date, 10);
    field(&mut c, &b.time, 8);
    c.extend_from_slice(&(b.time_reference as u32).to_le_bytes());
    c.extend_from_slice(&((b.time_reference >> 32) as u32).to_le_bytes());
    c.extend_from_slice(&1u16.to_le_bytes()); // version
    c.extend(std::iter::repeat_n(0u8, 64 + 10 + 180)); // UMID, loudness, reserved
    c
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleFormat {
    I16,
    I24,
    F32,
}

impl SampleFormat {
    pub fn from_bits(bits: u32) -> Self {
        match bits {
            16 => SampleFormat::I16,
            32 => SampleFormat::F32,
            _ => SampleFormat::I24,
        }
    }
    fn bytes(self) -> usize {
        match self {
            SampleFormat::I16 => 2,
            SampleFormat::I24 => 3,
            SampleFormat::F32 => 4,
        }
    }
}

// RIFF(12) + JUNK(8+28) + fmt(8+40) + data(8)
const JUNK_AT: u64 = 12;
const JUNK_LEN: u32 = 28;

pub struct WavWriter {
    out: BufWriter<MirrorFile>,
    format: SampleFormat,
    channels: u16,
    rate: u32,
    data_at: u64,
    data_len: u64,
    since_header: u64,
    scratch: Vec<u8>,
}

fn fmt_chunk(format: SampleFormat, channels: u16, rate: u32) -> Vec<u8> {
    let bytes = format.bytes() as u16;
    let mut c = Vec::new();
    c.extend_from_slice(&0xFFFEu16.to_le_bytes()); // WAVE_FORMAT_EXTENSIBLE
    c.extend_from_slice(&channels.to_le_bytes());
    c.extend_from_slice(&rate.to_le_bytes());
    c.extend_from_slice(&(rate * (bytes * channels) as u32).to_le_bytes());
    c.extend_from_slice(&(bytes * channels).to_le_bytes());
    c.extend_from_slice(&(bytes * 8).to_le_bytes());
    c.extend_from_slice(&22u16.to_le_bytes());
    c.extend_from_slice(&(bytes * 8).to_le_bytes()); // valid bits
    let mask: u32 = match channels {
        1 => 0x4,
        2 => 0x3,
        _ => 0, // no speaker positions for interface inputs
    };
    c.extend_from_slice(&mask.to_le_bytes());
    // KSDATAFORMAT_SUBTYPE_PCM / _IEEE_FLOAT
    let tag: u16 = if format == SampleFormat::F32 { 3 } else { 1 };
    c.extend_from_slice(&tag.to_le_bytes());
    c.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x10, 0x00, 0x80, 0x00, 0x00, 0xAA, 0x00, 0x38, 0x9B, 0x71]);
    c
}

impl WavWriter {
    #[cfg(test)]
    pub fn create(path: &Path, format: SampleFormat, channels: u16, rate: u32) -> Result<Self> {
        Self::create_with(path, None, format, channels, rate, None)
    }

    /// Write `path` (plus a backup copy if given), with an optional
    /// broadcast-WAV time stamp.
    pub fn create_with(path: &Path, backup: Option<&Path>, format: SampleFormat, channels: u16, rate: u32, bext: Option<&Bext>) -> Result<Self> {
        let file = MirrorFile::create(path, backup)?;
        let mut out = BufWriter::with_capacity(1 << 20, file);
        let fmt = fmt_chunk(format, channels, rate);
        let mut h = Vec::new();
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(b"WAVE");
        h.extend_from_slice(b"JUNK");
        h.extend_from_slice(&JUNK_LEN.to_le_bytes());
        h.extend_from_slice(&[0u8; JUNK_LEN as usize]);
        h.extend_from_slice(b"fmt ");
        h.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        h.extend_from_slice(&fmt);
        if let Some(b) = bext {
            let c = bext_chunk(b);
            h.extend_from_slice(b"bext");
            h.extend_from_slice(&(c.len() as u32).to_le_bytes());
            h.extend_from_slice(&c);
        }
        h.extend_from_slice(b"data");
        h.extend_from_slice(&0u32.to_le_bytes());
        out.write_all(&h)?;
        Ok(WavWriter { out, format, channels, rate, data_at: h.len() as u64, data_len: 0, since_header: 0, scratch: Vec::new() })
    }

    pub fn channels(&self) -> u16 {
        self.channels
    }

    /// A problem with the backup copy since last asked.
    pub fn take_backup_error(&mut self) -> Option<String> {
        self.out.get_mut().take_backup_error()
    }

    /// Append interleaved samples (-1.0..1.0).
    pub fn write(&mut self, samples: &[f32]) -> Result<()> {
        self.scratch.clear();
        match self.format {
            SampleFormat::I16 => {
                for s in samples {
                    let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
                    self.scratch.extend_from_slice(&v.to_le_bytes());
                }
            }
            SampleFormat::I24 => {
                for s in samples {
                    let v = (s.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32;
                    self.scratch.extend_from_slice(&v.to_le_bytes()[..3]);
                }
            }
            SampleFormat::F32 => {
                for s in samples {
                    self.scratch.extend_from_slice(&s.to_le_bytes());
                }
            }
        }
        self.out.write_all(&self.scratch)?;
        self.data_len += self.scratch.len() as u64;
        self.since_header += self.scratch.len() as u64;
        let second = self.rate as u64 * self.channels as u64 * self.format.bytes() as u64;
        if self.since_header >= second {
            self.since_header = 0;
            self.update_header()?;
        }
        Ok(())
    }

    fn update_header(&mut self) -> Result<()> {
        self.out.flush()?;
        let f = self.out.get_mut();
        let end = f.stream_position()?;
        write_sizes(f, self.data_at, self.data_len, self.channels, self.format.bytes())?;
        f.seek(SeekFrom::Start(end))?;
        Ok(())
    }

    /// Finish the file and return its size.
    pub fn finish(mut self) -> Result<u64> {
        // Pad to an even length as RIFF requires.
        if self.data_len % 2 == 1 {
            self.out.write_all(&[0])?;
        }
        self.update_header()?;
        let mut f = self.out.into_inner().map_err(|e| e.into_error())?;
        f.sync_all()?;
        Ok(f.metadata()?.len())
    }
}

/// Write the RIFF and data sizes (switching to RF64 past 4 GB).
fn write_sizes(f: &mut (impl Write + Seek), data_at: u64, data_len: u64, channels: u16, bytes: usize) -> std::io::Result<()> {
    let riff_len = data_at - 8 + data_len + (data_len & 1);
    if riff_len > u32::MAX as u64 {
        f.seek(SeekFrom::Start(0))?;
        f.write_all(b"RF64")?;
        f.write_all(&u32::MAX.to_le_bytes())?;
        f.seek(SeekFrom::Start(JUNK_AT))?;
        f.write_all(b"ds64")?;
        f.write_all(&JUNK_LEN.to_le_bytes())?;
        f.write_all(&riff_len.to_le_bytes())?;
        f.write_all(&data_len.to_le_bytes())?;
        let frames = data_len / (channels as u64 * bytes as u64).max(1);
        f.write_all(&frames.to_le_bytes())?;
        f.write_all(&0u32.to_le_bytes())?; // table length
        f.seek(SeekFrom::Start(data_at - 4))?;
        f.write_all(&u32::MAX.to_le_bytes())?;
    } else {
        f.seek(SeekFrom::Start(4))?;
        f.write_all(&(riff_len as u32).to_le_bytes())?;
        f.seek(SeekFrom::Start(data_at - 4))?;
        f.write_all(&(data_len as u32).to_le_bytes())?;
    }
    Ok(())
}

/// Reads the WAV files Cutaway writes (PCM 16/24/32-bit or float, RIFF or
/// RF64), as interleaved f32.
pub struct WavReader {
    file: std::io::BufReader<std::fs::File>,
    pub channels: u16,
    pub rate: u32,
    bytes: usize,
    float: bool,
    /// Bytes of sample data left.
    left: u64,
    pub frames: u64,
}

impl WavReader {
    pub fn open(path: &Path) -> Result<Self> {
        use anyhow::{bail, Context};
        let mut f = std::io::BufReader::new(std::fs::File::open(path).with_context(|| format!("can't open {}", path.display()))?);
        let mut head = [0u8; 12];
        f.read_exact(&mut head)?;
        if !(head.starts_with(b"RIFF") || head.starts_with(b"RF64")) || &head[8..12] != b"WAVE" {
            bail!("{} isn't a WAV file", path.display());
        }
        let file_len = f.get_ref().metadata()?.len();
        let (mut channels, mut rate, mut bits, mut tag) = (0u16, 0u32, 0u16, 1u16);
        let mut ds64_data: Option<u64> = None;
        loop {
            let mut ch = [0u8; 8];
            f.read_exact(&mut ch).context("no audio data in the file")?;
            let size = u32::from_le_bytes(ch[4..8].try_into().unwrap()) as u64;
            match &ch[..4] {
                b"fmt " => {
                    let mut b = vec![0u8; size as usize];
                    f.read_exact(&mut b)?;
                    tag = u16::from_le_bytes([b[0], b[1]]);
                    channels = u16::from_le_bytes([b[2], b[3]]);
                    rate = u32::from_le_bytes(b[4..8].try_into().unwrap());
                    bits = u16::from_le_bytes([b[14], b[15]]);
                    if tag == 0xFFFE && b.len() >= 26 {
                        tag = u16::from_le_bytes([b[24], b[25]]);
                    }
                    if size % 2 == 1 {
                        f.seek_relative(1)?;
                    }
                }
                b"ds64" => {
                    let mut b = vec![0u8; size as usize];
                    f.read_exact(&mut b)?;
                    ds64_data = b.get(8..16).map(|v| u64::from_le_bytes(v.try_into().unwrap()));
                }
                b"data" => {
                    let pos = f.stream_position()?;
                    let len = if size == u32::MAX as u64 { ds64_data.unwrap_or(file_len - pos) } else { size };
                    let len = len.min(file_len - pos);
                    if channels == 0 || bits == 0 {
                        bail!("the WAV file has no format");
                    }
                    let bytes = (bits / 8) as usize;
                    let frames = len / (bytes as u64 * channels as u64);
                    return Ok(WavReader { file: f, channels, rate, bytes, float: tag == 3, left: len, frames });
                }
                _ => {
                    f.seek_relative((size + (size & 1)) as i64)?;
                }
            }
        }
    }

    /// Up to `max_frames` frames (empty at the end).
    pub fn read(&mut self, max_frames: usize) -> Result<Vec<f32>> {
        let frame_bytes = self.bytes * self.channels as usize;
        let want = ((max_frames * frame_bytes) as u64).min(self.left - self.left % frame_bytes as u64) as usize;
        let mut raw = vec![0u8; want];
        self.file.read_exact(&mut raw)?;
        self.left -= want as u64;
        let out = match (self.bytes, self.float) {
            (4, true) => raw.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect(),
            (2, _) => raw.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).collect(),
            (3, _) => raw.chunks_exact(3).map(|b| (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0).collect(),
            (4, false) => raw.chunks_exact(4).map(|b| i32::from_le_bytes(b.try_into().unwrap()) as f32 / 2_147_483_648.0).collect(),
            _ => anyhow::bail!("unsupported WAV sample size"),
        };
        Ok(out)
    }
}

/// Fix the sizes in a WAV file that wasn't finished (crash). Returns true if
/// anything was changed.
pub fn repair(path: &Path) -> Result<bool> {
    let mut f = OpenOptions::new().read(true).write(true).open(path)?;
    let len = f.metadata()?.len();
    let mut head = vec![0u8; 4096.min(len as usize)];
    f.read_exact(&mut head)?;
    if head.len() < 12 || !(head.starts_with(b"RIFF") || head.starts_with(b"RF64")) || &head[8..12] != b"WAVE" {
        return Ok(false);
    }
    // Walk chunks to the data chunk, remembering the format.
    let mut pos = 12usize;
    let (mut channels, mut bytes) = (2u16, 3usize);
    let data_at = loop {
        if pos + 8 > head.len() {
            return Ok(false);
        }
        let id = &head[pos..pos + 4];
        let size = u32::from_le_bytes(head[pos + 4..pos + 8].try_into().unwrap()) as usize;
        if id == b"fmt " && pos + 24 <= head.len() {
            channels = u16::from_le_bytes([head[pos + 10], head[pos + 11]]);
            bytes = (u16::from_le_bytes([head[pos + 22], head[pos + 23]]) / 8).max(1) as usize;
        }
        if id == b"data" {
            break (pos + 8) as u64;
        }
        pos += 8 + size + (size & 1);
    };
    let data_len = len - data_at;
    let before = head.clone();
    write_sizes(&mut f, data_at, data_len, channels, bytes)?;
    f.seek(SeekFrom::Start(0))?;
    let mut after = vec![0u8; before.len()];
    f.read_exact(&mut after)?;
    Ok(before != after)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cutaway-wav-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn writes_valid_wav() {
        for (fmt, codec) in [(SampleFormat::I16, "pcm_s16le"), (SampleFormat::I24, "pcm_s24le"), (SampleFormat::F32, "pcm_f32le")] {
            let p = tmp(&format!("{codec}.wav"));
            let mut w = WavWriter::create(&p, fmt, 4, 48000).unwrap();
            let block: Vec<f32> = (0..4800 * 4).map(|i| ((i / 4) as f32 * 0.01).sin() * 0.5).collect();
            for _ in 0..25 {
                w.write(&block).unwrap();
            }
            w.finish().unwrap();
            if let Ok(out) = std::process::Command::new("ffprobe")
                .args(["-v", "error", "-show_entries", "stream=codec_name,channels:format=duration", "-of", "csv=p=0"])
                .arg(&p)
                .output()
            {
                let s = String::from_utf8_lossy(&out.stdout);
                assert!(s.contains(codec) && s.contains(",4") && s.contains("2.5"), "{s}");
            }
        }
    }

    #[test]
    fn reads_back_what_it_wrote() {
        for fmt in [SampleFormat::I16, SampleFormat::I24, SampleFormat::F32] {
            let p = tmp(&format!("rt{fmt:?}.wav"));
            let b = Bext { description: "x".into(), date: "2026-10-18".into(), time: "09:00:00".into(), time_reference: 0 };
            let mut w = WavWriter::create_with(&p, None, fmt, 2, 48000, Some(&b)).unwrap();
            let block: Vec<f32> = (0..9600).map(|i| ((i / 2) as f32 * 0.01).sin() * 0.5).collect();
            w.write(&block).unwrap();
            w.finish().unwrap();
            let mut r = WavReader::open(&p).unwrap();
            assert_eq!((r.channels, r.rate, r.frames), (2, 48000, 4800));
            let mut got = Vec::new();
            loop {
                let chunk = r.read(1000).unwrap();
                if chunk.is_empty() {
                    break;
                }
                got.extend(chunk);
            }
            assert_eq!(got.len(), 9600);
            let err = got.iter().zip(&block).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(err < 1e-4, "{fmt:?} {err}");
            std::fs::remove_file(&p).ok();
        }
    }

    #[test]
    fn broadcast_wav_time_stamp() {
        let p = tmp("bext.wav");
        let b = Bext { description: "Cutaway take".into(), date: "2026-10-17".into(), time: "21:30:05".into(), time_reference: 77_405 * 48_000 };
        let mut w = WavWriter::create_with(&p, None, SampleFormat::I24, 2, 48000, Some(&b)).unwrap();
        w.write(&vec![0.1; 9600]).unwrap();
        w.finish().unwrap();
        if let Ok(o) = std::process::Command::new("ffprobe").args(["-v", "error", "-show_entries", "format_tags:stream=duration", "-of", "compact"]).arg(&p).output() {
            let out = String::from_utf8_lossy(&o.stdout);
            assert!(out.contains("time_reference=3715440000"), "{out}");
            assert!(out.contains("date=2026-10-17") && out.contains("creation_time=21:30:05"), "{out}");
            assert!(out.contains("duration=0.100000"), "{out}");
        }
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn repair_fixes_unfinished_header() {
        let p = tmp("crash.wav");
        let mut w = WavWriter::create(&p, SampleFormat::I24, 2, 48000).unwrap();
        w.write(&vec![0.1; 9600]).unwrap(); // 0.1 s: header not refreshed yet
        w.out.flush().unwrap();
        std::mem::forget(w); // crash: never finished
        assert!(repair(&p).unwrap());
        let b = std::fs::read(&p).unwrap();
        let data_len = u32::from_le_bytes(b[b.len() - 9600 * 3 - 4..b.len() - 9600 * 3].try_into().unwrap());
        assert_eq!(data_len as usize, 9600 * 3);
        assert!(!repair(&p).unwrap());
    }
}
