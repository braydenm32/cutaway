//! Crash-safe "hybrid" MP4/MOV writer.
//!
//! While recording, the file is a *fragmented* MP4: a header (`moov` with
//! `mvex`) followed by a `moof`+`mdat` pair every second. If the app or the
//! machine dies, everything up to the last complete second is playable.
//!
//! When recording stops normally, the file is turned into a regular MP4: a
//! complete `moov` describing every sample is appended at the end, and the
//! fragment headers are renamed to `free` so ordinary readers skip them. The
//! result opens in every editor. [`recover`] does the same conversion for a
//! file left behind by a crash.

use super::mirror::MirrorFile;
use super::{AudioConfig, Ns, VideoCodec, VideoConfig};
use anyhow::{bail, Context, Result};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Where a file is written: a plain file (crash recovery) or a mirrored one.
trait Out: Write + Seek {
    fn set_len(&mut self, len: u64) -> io::Result<()>;
    fn sync_data(&mut self) -> io::Result<()>;
    fn sync_all(&mut self) -> io::Result<()>;
}

impl Out for File {
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        File::set_len(self, len)
    }
    fn sync_data(&mut self) -> io::Result<()> {
        File::sync_data(self)
    }
    fn sync_all(&mut self) -> io::Result<()> {
        File::sync_all(self)
    }
}

impl Out for MirrorFile {
    fn set_len(&mut self, len: u64) -> io::Result<()> {
        MirrorFile::set_len(self, len)
    }
    fn sync_data(&mut self) -> io::Result<()> {
        MirrorFile::sync_data(self)
    }
    fn sync_all(&mut self) -> io::Result<()> {
        MirrorFile::sync_all(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Brand {
    Mp4,
    Mov,
}

const MOVIE_TIMESCALE: u32 = 1000;
pub const VIDEO_TIMESCALE: u32 = 90000;

// ---- box building --------------------------------------------------------------

fn bx(name: &[u8; 4], content: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(content.len() + 8);
    v.extend_from_slice(&((content.len() + 8) as u32).to_be_bytes());
    v.extend_from_slice(name);
    v.extend_from_slice(content);
    v
}

fn full(name: &[u8; 4], version: u8, flags: u32, content: &[u8]) -> Vec<u8> {
    let mut c = Vec::with_capacity(content.len() + 4);
    c.push(version);
    c.extend_from_slice(&flags.to_be_bytes()[1..]);
    c.extend_from_slice(content);
    bx(name, &c)
}

struct W(Vec<u8>);
impl W {
    fn new() -> Self {
        W(Vec::new())
    }
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(b);
        self
    }
    fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.extend(std::iter::repeat_n(0u8, n));
        self
    }
    fn matrix(&mut self) -> &mut Self {
        for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
            self.u32(v);
        }
        self
    }
}

fn ftyp(brand: Brand) -> Vec<u8> {
    let mut w = W::new();
    match brand {
        Brand::Mp4 => {
            w.bytes(b"isom").u32(0x200);
            for b in [b"isom", b"iso2", b"avc1", b"mp41"] {
                w.bytes(b);
            }
        }
        Brand::Mov => {
            w.bytes(b"qt  ").u32(0x200).bytes(b"qt  ");
        }
    }
    bx(b"ftyp", &w.0)
}

fn esds(cfg: &AudioConfig) -> Vec<u8> {
    fn desc(tag: u8, body: &[u8]) -> Vec<u8> {
        let mut v = vec![tag];
        // 4-byte "expandable" length, as written by most muxers.
        let n = body.len() as u32;
        v.extend_from_slice(&[0x80 | ((n >> 21) & 0x7f) as u8, 0x80 | ((n >> 14) & 0x7f) as u8, 0x80 | ((n >> 7) & 0x7f) as u8, (n & 0x7f) as u8]);
        v.extend_from_slice(body);
        v
    }
    let dsi = desc(0x05, &cfg.asc);
    let mut dcd = W::new();
    dcd.u8(0x40).u8(0x15).bytes(&[0, 0x18, 0]).u32(320_000).u32(0).bytes(&dsi);
    let dcd = desc(0x04, &dcd.0);
    let sl = desc(0x06, &[0x02]);
    let mut es = W::new();
    es.u16(1).u8(0).bytes(&dcd).bytes(&sl);
    full(b"esds", 0, 0, &desc(0x03, &es.0))
}

/// The ES descriptor inside `esds` (what AudioToolbox takes as an AAC
/// decoder's "magic cookie").
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn es_descriptor(cfg: &AudioConfig) -> Vec<u8> {
    esds(cfg)[12..].to_vec()
}

fn audio_sample_entry(cfg: &AudioConfig) -> Vec<u8> {
    let mut w = W::new();
    w.zeros(6).u16(1); // reserved, data_reference_index
    w.zeros(8); // version/revision/vendor (QuickTime) == reserved (ISO)
    w.u16(cfg.channels).u16(16).u16(0).u16(0).u32(cfg.rate << 16);
    w.bytes(&esds(cfg));
    bx(b"mp4a", &w.0)
}

fn video_sample_entry(cfg: &VideoConfig, brand: Brand) -> Vec<u8> {
    let mut w = W::new();
    w.zeros(6).u16(1);
    w.u16(0).u16(0).zeros(12);
    w.u16(cfg.width as u16).u16(cfg.height as u16);
    w.u32(0x0048_0000).u32(0x0048_0000).u32(0).u16(1);
    let mut name = [0u8; 32];
    let label = match cfg.codec {
        VideoCodec::H264 => &b"H.264"[..],
        VideoCodec::Hevc => &b"HEVC"[..],
        VideoCodec::ProRes(p) => p.label().as_bytes(),
    };
    name[0] = label.len() as u8;
    name[1..1 + label.len()].copy_from_slice(label);
    w.bytes(&name).u16(0x18).u16(0xFFFF);
    match cfg.codec {
        VideoCodec::H264 => w.bytes(&bx(b"avcC", &cfg.record)),
        VideoCodec::Hevc => w.bytes(&bx(b"hvcC", &cfg.record)),
        // Progressive, one field.
        VideoCodec::ProRes(_) => w.bytes(&bx(b"fiel", &[1, 0])),
    };
    // BT.709, video range.
    let colr = match brand {
        Brand::Mp4 => bx(b"colr", &[b'n', b'c', b'l', b'x', 0, 1, 0, 1, 0, 1, 0x00]),
        Brand::Mov => bx(b"colr", &[b'n', b'c', b'l', b'c', 0, 1, 0, 1, 0, 1]),
    };
    w.bytes(&colr).bytes(&bx(b"pasp", &[0, 0, 0, 1, 0, 0, 0, 1]));
    let fourcc = match cfg.codec {
        VideoCodec::H264 => b"avc1",
        VideoCodec::Hevc => b"hvc1",
        VideoCodec::ProRes(p) => p.fourcc(),
    };
    let mut entry = bx(fourcc, &w.0);
    if cfg.codec.is_prores() {
        // QuickTime video sample description: vendor 'appl', high quality.
        entry[8 + 8 + 4..8 + 8 + 8].copy_from_slice(b"appl");
        entry[8 + 8 + 8..8 + 8 + 12].copy_from_slice(&0x200u32.to_be_bytes());
        entry[8 + 8 + 12..8 + 8 + 16].copy_from_slice(&0x200u32.to_be_bytes());
    }
    entry
}

// ---- track model ----------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handler {
    Video,
    Audio,
    /// QuickTime timecode track.
    Timecode,
    /// QuickTime chapter (text) track.
    Chapters,
}

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub size: u32,
    pub duration: u32,
    pub sync: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Chunk {
    pub offset: u64,
    pub samples: u32,
}

/// Everything needed to write a track's final `trak` box.
#[derive(Clone, Debug)]
pub struct TrackInfo {
    pub id: u32,
    pub handler: Handler,
    pub timescale: u32,
    pub width: u32,
    pub height: u32,
    /// The complete `stsd` box.
    pub stsd: Vec<u8>,
    /// Encoder delay to hide with an edit list (track timescale).
    pub priming: u32,
    /// Time the track starts at, if not 0 (track timescale).
    pub start: u64,
    /// Present at most this much (hides the padding in the last AAC frame).
    pub trim_to: Option<u64>,
    /// Track references (`tmcd`, `chap`) to other track ids.
    pub tref: Vec<([u8; 4], u32)>,
    pub samples: Vec<Sample>,
    pub chunks: Vec<Chunk>,
    /// Shown as the track's name in editors.
    pub name: Option<String>,
    /// Played by default (extra audio tracks are off, so players don't mix
    /// them all together).
    pub enabled: bool,
    /// Tracks in the same group are alternatives: a player picks one.
    pub alt_group: u16,
}

impl TrackInfo {
    fn new(id: u32, handler: Handler, timescale: u32, width: u32, height: u32, stsd: Vec<u8>, priming: u32) -> Self {
        TrackInfo { id, handler, timescale, width, height, stsd, priming, start: 0, trim_to: None, tref: Vec::new(), samples: Vec::new(), chunks: Vec::new(), name: None, enabled: true, alt_group: 0 }
    }
}

impl TrackInfo {
    /// Presentation duration (without the start offset), track timescale.
    fn duration(&self) -> u64 {
        let d = self.samples.iter().map(|s| s.duration as u64).sum::<u64>().saturating_sub(self.priming as u64);
        self.trim_to.map_or(d, |t| d.min(t))
    }

    fn to_movie(&self, ticks: u64) -> u64 {
        ticks * MOVIE_TIMESCALE as u64 / self.timescale as u64
    }

    /// End of the track on the movie timeline.
    fn movie_end(&self) -> u64 {
        self.to_movie(self.start + self.duration())
    }
}

fn hdlr(handler: Handler, brand: Brand, track_name: Option<&str>) -> Vec<u8> {
    let (kind, name): (&[u8; 4], &str) = match handler {
        Handler::Video => (b"vide", "VideoHandler"),
        Handler::Audio => (b"soun", "SoundHandler"),
        Handler::Timecode => (b"tmcd", "TimeCodeHandler"),
        Handler::Chapters => (b"text", "SubtitleHandler"),
    };
    let name = track_name.unwrap_or(name);
    let name = crate::util::truncate_str(name, 200);
    let mut w = W::new();
    match brand {
        Brand::Mp4 => {
            w.u32(0).bytes(kind).zeros(12).bytes(name.as_bytes()).u8(0);
        }
        Brand::Mov => {
            w.bytes(b"mhlr").bytes(kind).zeros(12).u8(name.len() as u8).bytes(name.as_bytes());
        }
    }
    full(b"hdlr", 0, 0, &w.0)
}

fn trak(t: &TrackInfo, brand: Brand, fragmented: bool) -> Vec<u8> {
    let media_dur = t.samples.iter().map(|s| s.duration as u64).sum::<u64>();
    let movie_dur = t.to_movie(t.duration());

    let mut tk = W::new();
    tk.u32(0).u32(0).u32(t.id).u32(0).u32(t.movie_end().min(u32::MAX as u64) as u32).zeros(8);
    tk.u16(0).u16(t.alt_group).u16(if t.handler == Handler::Audio { 0x0100 } else { 0 }).u16(0).matrix();
    tk.u32(t.width << 16).u32(t.height << 16);
    // Timecode and chapter tracks are part of the movie but not "enabled"
    // (they aren't shown), as QuickTime and FFmpeg write them.
    let flags = match t.handler {
        Handler::Video | Handler::Audio if t.enabled => 7,
        Handler::Video | Handler::Audio => 6,
        Handler::Timecode | Handler::Chapters => 2,
    };
    let tkhd = full(b"tkhd", 0, flags, &tk.0);

    // Edit list: an empty stretch for a late start, then the media from
    // after the encoder delay.
    let mut edits: Vec<(u32, u32)> = Vec::new();
    if t.start > 0 {
        edits.push((t.to_movie(t.start).min(u32::MAX as u64) as u32, u32::MAX));
    }
    if t.priming > 0 || t.start > 0 {
        edits.push((movie_dur.min(u32::MAX as u64) as u32, t.priming));
    }
    let edts = if edits.is_empty() {
        None
    } else {
        let mut e = W::new();
        e.u32(edits.len() as u32);
        for (d, m) in edits {
            e.u32(d).u32(m).u32(0x0001_0000);
        }
        Some(bx(b"edts", &full(b"elst", 0, 0, &e.0)))
    };

    let mdhd = if media_dur > u32::MAX as u64 {
        let mut m = W::new();
        m.u64(0).u64(0).u32(t.timescale).u64(media_dur).u16(0x55c4).u16(0);
        full(b"mdhd", 1, 0, &m.0)
    } else {
        let mut m = W::new();
        m.u32(0).u32(0).u32(t.timescale).u32(media_dur as u32).u16(0x55c4).u16(0);
        full(b"mdhd", 0, 0, &m.0)
    };

    let mhd = match t.handler {
        Handler::Video => full(b"vmhd", 0, 1, &[0; 8]),
        Handler::Audio => full(b"smhd", 0, 0, &[0; 4]),
        Handler::Timecode if brand == Brand::Mp4 => full(b"nmhd", 0, 0, &[]),
        Handler::Timecode => gmhd(true),
        Handler::Chapters => gmhd(false),
    };
    let dinf = bx(b"dinf", &full(b"dref", 0, 0, &[&1u32.to_be_bytes()[..], &full(b"url ", 0, 1, &[])].concat()));

    // Sample tables.
    let mut stts = W::new();
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for s in &t.samples {
        match runs.last_mut() {
            Some((n, d)) if *d == s.duration => *n += 1,
            _ => runs.push((1, s.duration)),
        }
    }
    stts.u32(runs.len() as u32);
    for (n, d) in &runs {
        stts.u32(*n).u32(*d);
    }
    let mut stsc = W::new();
    let mut entries: Vec<(u32, u32)> = Vec::new();
    for (i, c) in t.chunks.iter().enumerate() {
        if entries.last().map(|e| e.1) != Some(c.samples) {
            entries.push((i as u32 + 1, c.samples));
        }
    }
    stsc.u32(entries.len() as u32);
    for (first, n) in &entries {
        stsc.u32(*first).u32(*n).u32(1);
    }
    let mut stsz = W::new();
    stsz.u32(0).u32(t.samples.len() as u32);
    for s in &t.samples {
        stsz.u32(s.size);
    }
    let big = t.chunks.iter().any(|c| c.offset > u32::MAX as u64);
    let mut stco = W::new();
    stco.u32(t.chunks.len() as u32);
    for c in &t.chunks {
        if big {
            stco.u64(c.offset);
        } else {
            stco.u32(c.offset as u32);
        }
    }
    let mut stbl = W::new();
    stbl.bytes(&t.stsd).bytes(&full(b"stts", 0, 0, &stts.0));
    if t.handler == Handler::Video && !fragmented && t.samples.iter().any(|s| !s.sync) {
        let mut stss = W::new();
        let syncs: Vec<u32> = t.samples.iter().enumerate().filter(|(_, s)| s.sync).map(|(i, _)| i as u32 + 1).collect();
        stss.u32(syncs.len() as u32);
        for n in syncs {
            stss.u32(n);
        }
        stbl.bytes(&full(b"stss", 0, 0, &stss.0));
    }
    stbl.bytes(&full(b"stsc", 0, 0, &stsc.0)).bytes(&full(b"stsz", 0, 0, &stsz.0));
    stbl.bytes(&full(if big { b"co64" } else { b"stco" }, 0, 0, &stco.0));

    let mut minf = W::new();
    minf.bytes(&mhd);
    if brand == Brand::Mov {
        let mut dh = W::new();
        dh.bytes(b"dhlr").bytes(b"url ").zeros(12).u8(0);
        minf.bytes(&full(b"hdlr", 0, 0, &dh.0));
    }
    minf.bytes(&dinf).bytes(&bx(b"stbl", &stbl.0));

    let mut mdia = W::new();
    mdia.bytes(&mdhd).bytes(&hdlr(t.handler, brand, t.name.as_deref())).bytes(&bx(b"minf", &minf.0));

    let mut tr = W::new();
    tr.bytes(&tkhd);
    if let Some(e) = edts {
        tr.bytes(&e);
    }
    if !t.tref.is_empty() && !fragmented {
        let mut refs = W::new();
        for (kind, id) in &t.tref {
            refs.bytes(&bx(kind, &id.to_be_bytes()));
        }
        tr.bytes(&bx(b"tref", &refs.0));
    }
    tr.bytes(&bx(b"mdia", &mdia.0));
    if let Some(name) = &t.name {
        // QuickTime/Final Cut read the track name from here.
        tr.bytes(&bx(b"udta", &bx(b"name", &name.as_bytes()[..name.len().min(200)])));
    }
    bx(b"trak", &tr.0)
}

/// Generic media header for timecode and chapter tracks (QuickTime).
fn gmhd(timecode: bool) -> Vec<u8> {
    let mut g = W::new();
    g.bytes(&full(b"gmin", 0, 0, &[0x00, 0x40, 0x80, 0x00, 0x80, 0x00, 0x80, 0x00, 0, 0, 0, 0]));
    let mut text = W::new();
    for v in [0x0001_0000u32, 0, 0, 0, 0x0001_0000, 0, 0, 0, 0x4000_0000] {
        text.u32(v);
    }
    g.bytes(&bx(b"text", &text.0));
    if timecode {
        // Display settings: 12 pt, black on white, "Lucida Grande".
        let mut tcmi = W::new();
        tcmi.u16(0).u16(0).u16(12).u16(0).zeros(6).bytes(&[0xff; 6]);
        let font = b"Lucida Grande";
        tcmi.u8(font.len() as u8).bytes(font);
        g.bytes(&bx(b"tmcd", &full(b"tcmi", 0, 0, &tcmi.0)));
    }
    bx(b"gmhd", &g.0)
}

/// `tmcd` sample description: non-drop-frame timecode at `fps`.
fn tmcd_stsd(fps: u32) -> Vec<u8> {
    let mut e = W::new();
    e.zeros(6).u16(1).u32(0).u32(0).u32(fps).u32(1).u8(fps.min(255) as u8).zeros(3);
    full(b"stsd", 0, 0, &[&1u32.to_be_bytes()[..], &bx(b"tmcd", &e.0)].concat())
}

/// QuickTime text sample description used for chapter tracks.
fn chapter_stsd() -> Vec<u8> {
    let mut e = W::new();
    e.zeros(6).u16(1);
    e.u32(1).u8(0).u8(0).u32(0); // display flags, justification, background
    e.zeros(8); // default text box
    e.u16(0).u16(0).u16(1).u8(0).u8(0).u32(0); // style record
    e.bytes(&bx(b"ftab", &[0, 1, 0, 1, 0])); // one font, id 1, no name
    full(b"stsd", 0, 0, &[&1u32.to_be_bytes()[..], &bx(b"text", &e.0)].concat())
}

/// One chapter title sample: length, UTF-8 text, encoding atom.
fn chapter_sample(title: &str) -> Vec<u8> {
    let t = crate::util::truncate_str(title, 1000).as_bytes();
    let mut v = W::new();
    v.u16(t.len() as u16).bytes(t).bytes(&bx(b"encd", &0x0000_0100u32.to_be_bytes()));
    v.0
}

/// Nero-style chapter list (read by VLC, mpv and other players).
fn chpl(chapters: &[(Ns, String)]) -> Vec<u8> {
    let mut w = W::new();
    w.u32(0).u8(chapters.len().min(255) as u8);
    for (t, title) in chapters.iter().take(255) {
        let name = &title.as_bytes()[..title.len().min(255)];
        w.u64((t.max(&0) / 100) as u64).u8(name.len() as u8).bytes(name);
    }
    full(b"chpl", 1, 0, &w.0)
}

fn moov(tracks: &[TrackInfo], brand: Brand, fragmented: bool) -> Vec<u8> {
    moov_with(tracks, brand, fragmented, &[], &[])
}

/// `udta/meta/ilst` with iTunes-style text tags.
fn ilst(tags: &[([u8; 4], String)]) -> Vec<u8> {
    let mut items = W::new();
    for (tag, value) in tags {
        let mut data = W::new();
        data.u32(1).u32(0).bytes(value.as_bytes()); // type 1 = UTF-8 text
        items.bytes(&bx(tag, &bx(b"data", &data.0)));
    }
    let mut h = W::new();
    h.u32(0).bytes(b"mdir").bytes(b"appl").zeros(8).u8(0);
    let hdlr = full(b"hdlr", 0, 0, &h.0);
    full(b"meta", 0, 0, &[hdlr, bx(b"ilst", &items.0)].concat())
}

fn moov_with(tracks: &[TrackInfo], brand: Brand, fragmented: bool, chapters: &[(Ns, String)], tags: &[([u8; 4], String)]) -> Vec<u8> {
    let dur = tracks.iter().map(|t| t.movie_end()).max().unwrap_or(0);
    let mut mv = W::new();
    mv.u32(0).u32(0).u32(MOVIE_TIMESCALE).u32(dur.min(u32::MAX as u64) as u32);
    mv.u32(0x0001_0000).u16(0x0100).zeros(10).matrix().zeros(24);
    mv.u32(tracks.iter().map(|t| t.id).max().unwrap_or(0) + 1);
    let mut m = W::new();
    m.bytes(&full(b"mvhd", 0, 0, &mv.0));
    for t in tracks {
        m.bytes(&trak(t, brand, fragmented));
    }
    if fragmented {
        let mut mvex = W::new();
        for t in tracks {
            let mut tr = W::new();
            tr.u32(t.id).u32(1).u32(0).u32(0).u32(0);
            mvex.bytes(&full(b"trex", 0, 0, &tr.0));
        }
        m.bytes(&bx(b"mvex", &mvex.0));
    }
    if !chapters.is_empty() || !tags.is_empty() {
        let mut u = W::new();
        if !chapters.is_empty() {
            u.bytes(&chpl(chapters));
        }
        if !tags.is_empty() {
            u.bytes(&ilst(tags));
        }
        m.bytes(&bx(b"udta", &u.0));
    }
    bx(b"moov", &m.0)
}

// ---- writer ---------------------------------------------------------------------

struct Pending {
    ticks: i64,
    data: Vec<u8>,
    sync: bool,
}

struct Track {
    info: TrackInfo,
    /// Nominal duration of one sample, used for the very last one.
    default_duration: u32,
    pending: Option<Pending>,
    frag: Vec<Sample>,
    frag_data: Vec<u8>,
    decode_time: u64,
    first_ticks: Option<i64>,
}

pub struct Mp4Writer {
    file: MirrorFile,
    brand: Brand,
    /// (fps, first frame number since midnight)
    timecode: Option<(u32, u32)>,
    chapters: Vec<(Ns, String)>,
    tracks: Vec<Track>,
    started: bool,
    pos: u64,
    patch_offsets: Vec<u64>,
    seq: u32,
    fragment_secs: f64,
    metadata: Vec<([u8; 4], String)>,
    major_brand: Option<[u8; 4]>,
}

fn ns_to_ticks(ns: Ns, timescale: u32) -> i64 {
    ((ns as i128 * timescale as i128 + 500_000_000) / 1_000_000_000) as i64
}

impl Mp4Writer {
    pub fn create(path: &Path, brand: Brand) -> Result<Self> {
        Self::create_with_backup(path, None, brand)
    }

    /// Write the file to `path` and, if given, a backup copy at `backup`.
    pub fn create_with_backup(path: &Path, backup: Option<&Path>, brand: Brand) -> Result<Self> {
        let file = MirrorFile::create(path, backup)?;
        Ok(Mp4Writer {
            file,
            brand,
            timecode: None,
            chapters: Vec::new(),
            tracks: Vec::new(),
            started: false,
            pos: 0,
            patch_offsets: Vec::new(),
            seq: 0,
            fragment_secs: 1.0,
            metadata: Vec::new(),
            major_brand: None,
        })
    }

    /// Add a video track. `fps` sets the duration of the final frame.
    pub fn add_video(&mut self, cfg: &VideoConfig, fps: u32) -> usize {
        let entry = video_sample_entry(cfg, self.brand);
        let stsd = full(b"stsd", 0, 0, &[&1u32.to_be_bytes()[..], &entry].concat());
        self.add_track(Handler::Video, VIDEO_TIMESCALE, cfg.width, cfg.height, stsd, 0, VIDEO_TIMESCALE / fps.max(1))
    }

    /// A problem with the backup copy since last asked.
    pub fn take_backup_error(&mut self) -> Option<String> {
        self.file.take_backup_error()
    }

    /// Add a timecode track starting at `start_frame` (frames since
    /// midnight at `fps`), so editors place the file by time of day.
    pub fn set_timecode(&mut self, fps: u32, start_frame: u32) {
        self.timecode = Some((fps.max(1), start_frame));
    }

    /// Chapter marks: start time (ns from the start of the file) and title.
    pub fn set_chapters(&mut self, mut chapters: Vec<(Ns, String)>) {
        chapters.sort_by_key(|c| c.0);
        self.chapters = chapters;
    }

    /// How long the track really is (the encoder pads its last frame).
    pub fn set_duration(&mut self, track: usize, ns: Ns) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.info.trim_to = Some((ns.max(0) as i128 * t.info.timescale as i128 / 1_000_000_000) as u64);
        }
    }

    pub fn add_audio(&mut self, cfg: &AudioConfig) -> usize {
        let entry = audio_sample_entry(cfg);
        let stsd = full(b"stsd", 0, 0, &[&1u32.to_be_bytes()[..], &entry].concat());
        self.add_track(Handler::Audio, cfg.rate, 0, 0, stsd, cfg.priming, super::AAC_FRAME)
    }

    /// Name a track and say whether it plays by default; tracks given the
    /// same `alt_group` are alternatives to each other.
    pub fn set_track_info(&mut self, track: usize, name: &str, enabled: bool, alt_group: u16) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.info.name = Some(name.to_string());
            t.info.enabled = enabled;
            t.info.alt_group = alt_group;
        }
    }

    /// iTunes-style tags (title, artist...) for players and podcast apps.
    pub fn set_metadata(&mut self, tags: Vec<([u8; 4], String)>) {
        self.metadata = tags;
    }

    /// Use a different main brand in the file type (e.g. `M4A ` for audio).
    pub fn set_major_brand(&mut self, brand: [u8; 4]) {
        self.major_brand = Some(brand);
    }

    #[allow(clippy::too_many_arguments)]
    fn add_track(&mut self, handler: Handler, timescale: u32, width: u32, height: u32, stsd: Vec<u8>, priming: u32, default_duration: u32) -> usize {
        assert!(!self.started, "tracks must be added before the first sample");
        let id = self.tracks.len() as u32 + 1;
        self.tracks.push(Track {
            info: TrackInfo::new(id, handler, timescale, width, height, stsd, priming),
            default_duration,
            pending: None,
            frag: Vec::new(),
            frag_data: Vec::new(),
            decode_time: 0,
            first_ticks: None,
        });
        self.tracks.len() - 1
    }

    fn start(&mut self) -> Result<()> {
        let infos: Vec<TrackInfo> = self.tracks.iter().map(|t| t.info.clone()).collect();
        let mut head = ftyp(self.brand);
        if let Some(b) = self.major_brand {
            head[8..12].copy_from_slice(&b);
        }
        let moov_at = head.len() as u64;
        head.extend_from_slice(&moov(&infos, self.brand, true));
        self.file.write_all(&head)?;
        self.patch_offsets.push(moov_at);
        self.pos = head.len() as u64;
        self.started = true;
        Ok(())
    }

    /// Add one sample. `time` is its decode time in ns from the start of the
    /// file; times must increase per track.
    pub fn write(&mut self, track: usize, time: Ns, data: &[u8], sync: bool) -> Result<()> {
        if !self.started {
            self.start()?;
        }
        let t = &mut self.tracks[track];
        let ticks = ns_to_ticks(time, t.info.timescale).max(0);
        let ticks = match &t.pending {
            Some(p) if ticks <= p.ticks => p.ticks + 1,
            _ => ticks,
        };
        if t.first_ticks.is_none() {
            // A track that starts late keeps its start time: as the decode
            // time of its first fragment, and as an edit in the final file.
            t.first_ticks = Some(ticks);
            t.decode_time = ticks as u64;
            t.info.start = ticks as u64;
        }
        if let Some(p) = t.pending.take() {
            let duration = (ticks - p.ticks).max(1) as u32;
            t.frag.push(Sample { size: p.data.len() as u32, duration, sync: p.sync });
            t.frag_data.extend_from_slice(&p.data);
        }
        t.pending = Some(Pending { ticks, data: data.to_vec(), sync });
        let longest = self
            .tracks
            .iter()
            .map(|t| t.frag.iter().map(|s| s.duration as f64).sum::<f64>() / t.info.timescale as f64)
            .fold(0.0, f64::max);
        if longest >= self.fragment_secs {
            self.flush_fragment()?;
        }
        Ok(())
    }

    fn flush_fragment(&mut self) -> Result<()> {
        if self.tracks.iter().all(|t| t.frag.is_empty()) {
            return Ok(());
        }
        self.seq += 1;
        // Build moof with data offsets filled in afterwards.
        let mut trafs = Vec::new();
        let mut offset_fields = Vec::new(); // (index in moof bytes, data offset within mdat payload)
        let mut data_len = 0u64;
        for t in &self.tracks {
            if t.frag.is_empty() {
                continue;
            }
            let mut tf = W::new();
            tf.bytes(&full(b"tfhd", 0, 0x02_0000, &t.info.id.to_be_bytes()));
            tf.bytes(&full(b"tfdt", 1, 0, &t.decode_time.to_be_bytes()));
            let mut tr = W::new();
            tr.u32(t.frag.len() as u32);
            let field_at = tr.0.len();
            tr.u32(0);
            for s in &t.frag {
                let flags = if s.sync { 0x0200_0000 } else { 0x0101_0000 };
                tr.u32(s.duration).u32(s.size).u32(flags);
            }
            let trun = full(b"trun", 0, 0x000701, &tr.0);
            let trun_at = tf.0.len();
            tf.bytes(&trun);
            trafs.push((bx(b"traf", &tf.0), trun_at + 12 + field_at, data_len));
            data_len += t.frag_data.len() as u64;
        }
        let mut moof_body = full(b"mfhd", 0, 0, &self.seq.to_be_bytes());
        for (traf, field, data_off) in &trafs {
            let at = moof_body.len() + 8 + field; // inside traf: 8-byte traf header
            offset_fields.push((at + 8, *data_off)); // + 8 for the moof header
            moof_body.extend_from_slice(traf);
        }
        let mut moof = bx(b"moof", &moof_body);
        let moof_size = moof.len() as u64;
        for (at, data_off) in &offset_fields {
            let v = (moof_size + 8 + data_off) as u32;
            moof[*at..*at + 4].copy_from_slice(&v.to_be_bytes());
        }
        let moof_pos = self.pos;
        let mdat_header = {
            let mut h = Vec::new();
            h.extend_from_slice(&((data_len + 8) as u32).to_be_bytes());
            h.extend_from_slice(b"mdat");
            h
        };
        let mut buf = Vec::with_capacity(moof.len() + 8 + data_len as usize);
        buf.extend_from_slice(&moof);
        buf.extend_from_slice(&mdat_header);
        let mut file_off = moof_pos + moof_size + 8;
        for t in &mut self.tracks {
            if t.frag.is_empty() {
                continue;
            }
            buf.extend_from_slice(&t.frag_data);
            t.info.chunks.push(Chunk { offset: file_off, samples: t.frag.len() as u32 });
            file_off += t.frag_data.len() as u64;
            t.decode_time += t.frag.iter().map(|s| s.duration as u64).sum::<u64>();
            t.info.samples.append(&mut t.frag);
            t.frag_data.clear();
        }
        self.file.write_all(&buf)?;
        self.patch_offsets.push(moof_pos);
        self.pos += buf.len() as u64;
        Ok(())
    }

    /// Finish the file: last samples, final fragment, then convert to a
    /// regular MP4. Returns the file size.
    pub fn finish(mut self) -> Result<u64> {
        if !self.started {
            // Nothing was ever written: leave an empty but valid file.
            self.start()?;
        }
        for t in &mut self.tracks {
            if let Some(p) = t.pending.take() {
                let duration = t.default_duration.max(1);
                t.frag.push(Sample { size: p.data.len() as u32, duration, sync: p.sync });
                t.frag_data.extend_from_slice(&p.data);
            }
        }
        self.flush_fragment()?;
        let mut infos: Vec<TrackInfo> = self.tracks.iter().map(|t| t.info.clone()).collect();

        // Timecode and chapter tracks: their few bytes of samples go in one
        // small mdat before the final moov.
        let movie_ms = infos.iter().map(|t| t.movie_end()).max().unwrap_or(0);
        let mut extra = Vec::new();
        let mut end = self.pos;
        let mdat_payload_at = end + 8;
        let mut next_id = infos.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        if let Some((fps, start_frame)) = self.timecode {
            let frames = (movie_ms * fps as u64).div_ceil(1000).max(1) as u32;
            let id = next_id;
            next_id += 1;
            infos.push(TrackInfo {
                samples: vec![Sample { size: 4, duration: frames, sync: true }],
                chunks: vec![Chunk { offset: mdat_payload_at + extra.len() as u64, samples: 1 }],
                ..TrackInfo::new(id, Handler::Timecode, fps, 0, 0, tmcd_stsd(fps), 0)
            });
            extra.extend_from_slice(&start_frame.to_be_bytes());
            if let Some(v) = infos.iter_mut().find(|t| t.handler == Handler::Video) {
                v.tref.push((*b"tmcd", id));
            }
        }
        let chapters: Vec<(Ns, String)> = self.chapters.iter().filter(|c| c.0 >= 0 && (c.0 / 1_000_000) < movie_ms as i64).cloned().collect();
        if !chapters.is_empty() {
            let id = next_id;
            let mut samples = Vec::new();
            let mut first_chunk_at = None;
            // The first chapter always starts at 0.
            let mut marks = chapters.clone();
            if marks[0].0 > 0 {
                marks.insert(0, (0, "Start".into()));
            }
            for (i, (t, title)) in marks.iter().enumerate() {
                let start_ms = (*t / 1_000_000) as u64;
                let next_ms = marks.get(i + 1).map(|n| (n.0 / 1_000_000) as u64).unwrap_or(movie_ms);
                let data = chapter_sample(title);
                first_chunk_at.get_or_insert(mdat_payload_at + extra.len() as u64);
                samples.push(Sample { size: data.len() as u32, duration: next_ms.saturating_sub(start_ms).max(1) as u32, sync: true });
                extra.extend_from_slice(&data);
            }
            infos.push(TrackInfo {
                chunks: vec![Chunk { offset: first_chunk_at.unwrap(), samples: samples.len() as u32 }],
                samples,
                ..TrackInfo::new(id, Handler::Chapters, 1000, 0, 0, chapter_stsd(), 0)
            });
            let owner = infos.iter().position(|t| t.handler == Handler::Audio && t.enabled).or_else(|| infos.iter().position(|t| t.handler == Handler::Video));
            if let Some(o) = owner {
                infos[o].tref.push((*b"chap", id));
            }
        }
        if !extra.is_empty() {
            self.file.seek(SeekFrom::Start(end))?;
            self.file.write_all(&((extra.len() + 8) as u32).to_be_bytes())?;
            self.file.write_all(b"mdat")?;
            self.file.write_all(&extra)?;
            end += 8 + extra.len() as u64;
        }
        finalize(&mut self.file, self.brand, &infos, &self.patch_offsets, end, &chapters, &self.metadata)
    }
}

/// Rename the given boxes to `free` and append a complete `moov`.
fn finalize(file: &mut impl Out, brand: Brand, tracks: &[TrackInfo], patch: &[u64], end: u64, chapters: &[(Ns, String)], tags: &[([u8; 4], String)]) -> Result<u64> {
    // Tracks that start late get an edit that delays them, so everything
    // stays aligned to 0 like the recording.
    file.set_len(end)?;
    file.seek(SeekFrom::Start(end))?;
    let mv = moov_with(tracks, brand, false, chapters, tags);
    file.write_all(&mv)?;
    file.sync_data()?;
    for at in patch {
        file.seek(SeekFrom::Start(at + 4))?;
        file.write_all(b"free")?;
    }
    file.sync_all()?;
    Ok(end + mv.len() as u64)
}

// ---- reading (crash recovery) -----------------------------------------------------

fn read_u32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_u64(b: &[u8], at: usize) -> Option<u64> {
    b.get(at..at + 8).map(|s| u64::from_be_bytes(s.try_into().unwrap()))
}

/// Child boxes of a box payload: (type, start of payload, end).
fn children(b: &[u8]) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 8 <= b.len() {
        let size = read_u32(b, i).unwrap() as usize;
        let kind: [u8; 4] = b[i + 4..i + 8].try_into().unwrap();
        let (hdr, size) = if size == 1 {
            match read_u64(b, i + 8) {
                Some(s) => (16, s as usize),
                None => break,
            }
        } else if size == 0 {
            (8, b.len() - i)
        } else {
            (8, size)
        };
        if size < hdr || i + size > b.len() {
            break;
        }
        out.push((kind, i + hdr, i + size));
        i += size;
    }
    out
}

fn find<'a>(b: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    children(b).into_iter().find(|c| &c.0 == kind).map(|(_, s, e)| &b[s..e])
}

/// Media time of the last non-empty edit (the encoder delay).
fn parse_elst_media_time(elst: &[u8]) -> u32 {
    let count = read_u32(elst, 4).unwrap_or(0) as usize;
    let mut out = 0;
    for i in 0..count {
        if let Some(m) = read_u32(elst, 8 + i * 12 + 4) {
            if m != u32::MAX {
                out = m;
            }
        }
    }
    out
}

/// What [`recover`] did.
#[derive(Debug, PartialEq)]
pub enum Recovery {
    /// The file was already complete.
    AlreadyComplete,
    /// Converted; this many seconds were saved.
    Recovered { seconds: f64 },
    /// Not an MP4 this app wrote.
    NotRecoverable,
}

/// Turn a fragmented MP4 left behind by a crash into a regular one.
pub fn recover(path: &Path) -> Result<Recovery> {
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let len = file.metadata()?.len();
    // Walk the top-level boxes.
    let mut pos = 0u64;
    let mut moov_box: Option<(u64, Vec<u8>)> = None;
    let mut moofs: Vec<(u64, Vec<u8>)> = Vec::new();
    // End of the last complete box (a fragment counts once its mdat is in).
    let mut end_ok = 0u64;
    let mut brand = Brand::Mp4;
    let mut head = [0u8; 16];
    while pos + 8 <= len {
        file.seek(SeekFrom::Start(pos))?;
        if file.read_exact(&mut head[..8]).is_err() {
            break;
        }
        let mut size = u32::from_be_bytes(head[..4].try_into().unwrap()) as u64;
        let kind: [u8; 4] = head[4..8].try_into().unwrap();
        let mut hdr = 8;
        if size == 1 {
            if file.read_exact(&mut head[8..16]).is_err() {
                break;
            }
            size = u64::from_be_bytes(head[8..16].try_into().unwrap());
            hdr = 16;
        }
        if size < hdr || pos + size > len {
            break; // truncated box: the end of what survived
        }
        let body_len = size - hdr;
        match &kind {
            b"ftyp" => {
                let mut b = vec![0u8; body_len.min(64) as usize];
                file.read_exact(&mut b)?;
                if b.starts_with(b"qt  ") {
                    brand = Brand::Mov;
                }
            }
            b"moov" | b"moof" => {
                let mut b = vec![0u8; body_len as usize];
                file.read_exact(&mut b)?;
                if &kind == b"moov" {
                    moov_box = Some((pos, b));
                } else {
                    moofs.push((pos, b));
                }
            }
            _ => {}
        }
        if &kind != b"moof" {
            end_ok = pos + size;
        }
        pos += size;
    }
    let Some((moov_at, moov_body)) = moov_box else { return Ok(Recovery::NotRecoverable) };
    if find(&moov_body, b"mvex").is_none() {
        // Finished, but if it was interrupted while renaming the fragment
        // headers, finish that too.
        for (at, _) in &moofs {
            file.seek(SeekFrom::Start(at + 4))?;
            file.write_all(b"free")?;
        }
        return Ok(Recovery::AlreadyComplete);
    }
    // Keep only fragments whose mdat finished.
    moofs.retain(|(at, _)| *at < end_ok);

    let mut tracks: Vec<TrackInfo> = Vec::new();
    for (kind, s, e) in children(&moov_body) {
        if &kind != b"trak" {
            continue;
        }
        let trak = &moov_body[s..e];
        let tkhd = find(trak, b"tkhd").context("tkhd")?;
        let id = read_u32(tkhd, 12).context("track id")?;
        let (width, height) = (read_u32(tkhd, 76).unwrap_or(0) >> 16, read_u32(tkhd, 80).unwrap_or(0) >> 16);
        let priming = find(trak, b"edts").and_then(|e| find(e, b"elst")).map(parse_elst_media_time).unwrap_or(0);
        let mdia = find(trak, b"mdia").context("mdia")?;
        let mdhd = find(mdia, b"mdhd").context("mdhd")?;
        let timescale = if mdhd[0] == 1 { read_u32(mdhd, 20) } else { read_u32(mdhd, 12) }.context("timescale")?;
        let hd = find(mdia, b"hdlr").context("hdlr")?;
        let handler = if &hd[8..12] == b"vide" { Handler::Video } else { Handler::Audio };
        let stbl = find(mdia, b"minf").and_then(|m| find(m, b"stbl")).context("stbl")?;
        let stsd_body = find(stbl, b"stsd").context("stsd")?;
        let stsd = bx(b"stsd", stsd_body);
        let mut info = TrackInfo::new(id, handler, timescale, width, height, stsd, priming);
        info.enabled = read_u32(tkhd, 0).unwrap_or(1) & 1 == 1;
        info.alt_group = tkhd.get(34..36).map(|b| u16::from_be_bytes([b[0], b[1]])).unwrap_or(0);
        info.name = find(trak, b"udta").and_then(|u| find(u, b"name")).map(|n| String::from_utf8_lossy(n).to_string());
        tracks.push(info);
    }
    if tracks.is_empty() {
        return Ok(Recovery::NotRecoverable);
    }
    for (moof_at, body) in &moofs {
        for (kind, s, e) in children(body) {
            if &kind != b"traf" {
                continue;
            }
            let traf = &body[s..e];
            let Some(tfhd) = find(traf, b"tfhd") else { continue };
            let Some(id) = read_u32(tfhd, 4) else { continue };
            let Some(t) = tracks.iter_mut().find(|t| t.id == id) else { continue };
            let Some(trun) = find(traf, b"trun") else { continue };
            if t.samples.is_empty() {
                t.start = find(traf, b"tfdt")
                    .map(|d| if d[0] == 1 { read_u64(d, 4).unwrap_or(0) } else { read_u32(d, 4).unwrap_or(0) as u64 })
                    .unwrap_or(0);
            }
            let flags = read_u32(trun, 0).unwrap_or(0) & 0xFF_FFFF;
            if flags != 0x000701 {
                bail!("unexpected fragment layout");
            }
            let count = read_u32(trun, 4).unwrap_or(0);
            let data_offset = read_u32(trun, 8).unwrap_or(0) as u64;
            let mut at = 12;
            for _ in 0..count {
                let (Some(d), Some(sz), Some(f)) = (read_u32(trun, at), read_u32(trun, at + 4), read_u32(trun, at + 8)) else { break };
                t.samples.push(Sample { duration: d, size: sz, sync: f & 0x0001_0000 == 0 });
                at += 12;
            }
            t.chunks.push(Chunk { offset: moof_at + data_offset, samples: count });
        }
    }
    let seconds = tracks
        .iter()
        .map(|t| t.samples.iter().map(|s| s.duration as f64).sum::<f64>() / t.timescale as f64)
        .fold(0.0, f64::max);
    let mut patch = vec![moov_at];
    patch.extend(moofs.iter().map(|m| m.0));
    finalize(&mut file, brand, &tracks, &patch, end_ok, &[], &[])?;
    Ok(Recovery::Recovered { seconds })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::media::h264;
    use std::path::PathBuf;
    use std::sync::Arc;

    /// A tiny but valid H.264 stream would need an encoder; for structure
    /// tests a fake config is enough (ffprobe reads headers fine).
    pub fn fake_video_config() -> VideoConfig {
        let sps = vec![0x67, 0x42, 0xC0, 0x1E, 0xDA, 0x02, 0x80, 0xBF, 0xE5, 0x84];
        let pps = vec![0x68, 0xCE, 0x3C, 0x80];
        VideoConfig { codec: VideoCodec::H264, width: 640, height: 360, record: h264::build_avcc(&sps, &pps), parameter_sets: vec![sps, pps] }
    }

    pub fn audio_config() -> AudioConfig {
        AudioConfig { rate: 48000, channels: 2, asc: h264::aac_asc(48000, 2), priming: 1024 }
    }

    fn write_sample_file(path: &Path, seconds: u32, finish: bool) -> Option<Mp4Writer> {
        let mut w = Mp4Writer::create(path, Brand::Mp4).unwrap();
        let v = w.add_video(&fake_video_config(), 30);
        let a = w.add_audio(&audio_config());
        let frames = seconds * 30;
        let mut audio_t = 0i64;
        for i in 0..frames {
            let t = i as i64 * 1_000_000_000 / 30;
            while audio_t <= t {
                w.write(a, audio_t, &[0x21, 0x10, 0x04, 0x60, 0x8C, 0x1C], true).unwrap();
                audio_t += 1_000_000_000 * 1024 / 48000;
            }
            let data = h264::to_avcc([&[0x65u8, 0x88, 0x84, i as u8][..]]);
            w.write(v, t, &data, i % 30 == 0).unwrap();
        }
        if finish {
            w.finish().unwrap();
            None
        } else {
            Some(w)
        }
    }

    fn probe(path: &Path) -> Option<String> {
        let out = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "stream=codec_name,nb_frames,duration:format=duration", "-of", "compact"])
            .arg(path)
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr))
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("cutaway-mp4-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join(name)
    }

    #[test]
    fn finished_file_is_regular_mp4() {
        let p = tmp("done.mp4");
        write_sample_file(&p, 3, true);
        let bytes = std::fs::read(&p).unwrap();
        // Top level: ftyp, free (old moov), free (moofs), mdat..., moov
        let kinds: Vec<[u8; 4]> = children(&bytes).iter().map(|c| c.0).collect();
        assert_eq!(&kinds[0], b"ftyp");
        assert_eq!(kinds.last().unwrap(), b"moov");
        assert!(!kinds.contains(b"moof"));
        let moov_body = {
            let c = children(&bytes);
            let m = c.last().unwrap();
            bytes[m.1..m.2].to_vec()
        };
        assert!(find(&moov_body, b"mvex").is_none());
        if let Some(out) = probe(&p) {
            assert!(out.contains("codec_name=h264|duration=3.000000|nb_frames=90"), "{out}");
            assert!(out.contains("codec_name=aac"), "{out}");
        }
    }

    #[test]
    fn crashed_file_recovers() {
        let p = tmp("crash.mp4");
        let w = write_sample_file(&p, 4, false).unwrap();
        // Simulate a crash: drop without finishing, then chop the tail.
        drop(w);
        let len = std::fs::metadata(&p).unwrap().len();
        let f = OpenOptions::new().write(true).open(&p).unwrap();
        f.set_len(len - 50).unwrap();
        drop(f);
        let r = recover(&p).unwrap();
        match r {
            Recovery::Recovered { seconds } => assert!(seconds >= 2.0 && seconds < 4.0, "{seconds}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(recover(&p).unwrap(), Recovery::AlreadyComplete);
        if let Some(out) = probe(&p) {
            assert!(out.contains("codec_name=h264"), "{out}");
        }
    }

    #[test]
    fn packets_type_is_arc() {
        // Compile-time check that packet data is shareable between files.
        let _p = crate::media::VideoPacket {
            pts: 0,
            dts: 0,
            keyframe: true,
            data: Arc::new(vec![]),
            config: Arc::new(fake_video_config()),
        };
    }
}
